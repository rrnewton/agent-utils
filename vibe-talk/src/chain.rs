//! Read-aloud through an ordered chain of voice providers.
//!
//! `#563 read-aloud-fallback-chain`. The owner asked for two options on
//! text-to-speech so reading keeps working when one voice stack is down:
//! a deployment names an ordered list (for example local, then Wit.ai,
//! then the conversational voice stack), and a block that one provider
//! refuses — an explicit error, or audio that says nothing — is spoken
//! by the next. The reader is told which voice actually spoke, per block,
//! through [`SpeechStream::voice`] and the `x-read-aloud-voice` response
//! header.
//!
//! One rule keeps this honest: fallback happens BEFORE any audio reaches
//! the reader. A provider whose stream fails mid-block cannot be replaced
//! inaudibly, so that failure stays loud exactly as it is for a single
//! provider. Empty text is not a provider failure and never falls
//! through: there is nothing for the next voice to say either.

use std::sync::Arc;

use async_trait::async_trait;

use crate::speech::{Description, Speech, SpeechError, SpeechProvider, SpeechStream, VoiceInfo};

/// An ordered set of providers tried in turn for each block.
pub struct ChainSpeech {
    providers: Vec<Arc<dyn SpeechProvider>>,
}

impl ChainSpeech {
    /// Build a chain from at least one provider, in trying order.
    ///
    /// # Panics
    ///
    /// Panics when `providers` is empty: a chain with no voice is a
    /// configuration error the caller must not be able to build.
    #[must_use]
    pub fn new(providers: Vec<Arc<dyn SpeechProvider>>) -> Self {
        assert!(!providers.is_empty(), "a voice chain needs a provider");
        Self { providers }
    }

    /// The providers in trying order.
    #[must_use]
    pub fn providers(&self) -> &[Arc<dyn SpeechProvider>] {
        &self.providers
    }
}

#[async_trait]
impl SpeechProvider for ChainSpeech {
    fn describe(&self) -> Description {
        let first = self.providers[0].describe();
        Description {
            backend: first.backend,
            label: first.label.clone(),
            playback: first.playback,
            local_only: first.local_only,
            chain: self
                .providers
                .iter()
                .map(|provider| provider.describe().label)
                .collect(),
        }
    }

    async fn warm_up(&self) -> Result<(), SpeechError> {
        // Warm the first voice that will warm: preparation must not fail
        // the read because a fallback further down is asleep.
        let mut last: Option<SpeechError> = None;
        for provider in &self.providers {
            match provider.warm_up().await {
                Ok(()) => return Ok(()),
                Err(error) => last = Some(error),
            }
        }
        Err(last.expect("a chain has a provider"))
    }

    async fn speak(&self, text: &str, speed: Option<f64>) -> Result<Speech, SpeechError> {
        if text.trim().is_empty() {
            return Err(SpeechError::Empty);
        }
        let mut last: Option<SpeechError> = None;
        for provider in &self.providers {
            match provider.speak(text, speed).await {
                Ok(spoken) => return Ok(spoken),
                Err(SpeechError::Empty) => return Err(SpeechError::Empty),
                Err(error) => last = Some(error),
            }
        }
        Err(last.expect("a chain has a provider"))
    }

    async fn speak_stream_voiced(
        &self,
        text: &str,
        speed: Option<f64>,
        selection: Option<&str>,
        voice: Option<&str>,
    ) -> Result<SpeechStream, SpeechError> {
        if text.trim().is_empty() {
            return Err(SpeechError::Empty);
        }
        let mut last: Option<SpeechError> = None;
        for provider in &self.providers {
            match provider
                .speak_stream_voiced(text, speed, selection, voice)
                .await
            {
                Ok(mut stream) => {
                    // Name the voice that is about to speak this block.
                    if stream.voice.is_none() {
                        stream.voice = Some(provider.describe().label);
                    }
                    return Ok(stream);
                }
                Err(SpeechError::Empty) => return Err(SpeechError::Empty),
                Err(error) => last = Some(error),
            }
        }
        Err(last.expect("a chain has a provider"))
    }

    async fn voices(&self) -> Result<Vec<VoiceInfo>, SpeechError> {
        // The first catalogue wins: that is the voice the reader will
        // most likely hear, and its names are the ones a pick must use.
        let mut last: Option<SpeechError> = None;
        for provider in &self.providers {
            match provider.voices().await {
                Ok(voices) if !voices.is_empty() => return Ok(voices),
                Ok(_) => {}
                Err(error) => last = Some(error),
            }
        }
        match last {
            Some(error) => Err(error),
            None => Ok(Vec::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speech::{Description, Playback};
    use std::sync::Mutex;

    struct Stub {
        label: &'static str,
        result: Mutex<Option<Result<Speech, SpeechError>>>,
        calls: Mutex<usize>,
    }

    impl Stub {
        fn ok(label: &'static str) -> Arc<Self> {
            Arc::new(Self {
                label,
                result: Mutex::new(Some(Ok(Speech {
                    audio: vec![1],
                    content_type: "audio/wav".to_owned(),
                }))),
                calls: Mutex::new(0),
            })
        }
        fn fails(label: &'static str, error: SpeechError) -> Arc<Self> {
            Arc::new(Self {
                label,
                result: Mutex::new(Some(Err(error))),
                calls: Mutex::new(0),
            })
        }
        fn calls(&self) -> usize {
            *self.calls.lock().expect("calls")
        }
    }

    #[async_trait]
    impl SpeechProvider for Stub {
        fn describe(&self) -> Description {
            Description {
                backend: "stub",
                label: self.label.to_owned(),
                playback: Playback::Audio,
                local_only: false,
                chain: Vec::new(),
            }
        }

        async fn speak(&self, _text: &str, _speed: Option<f64>) -> Result<Speech, SpeechError> {
            *self.calls.lock().expect("calls") += 1;
            self.result
                .lock()
                .expect("result")
                .take()
                .expect("one result per stub")
        }
    }

    fn backend_error(code: &'static str) -> SpeechError {
        SpeechError::Backend {
            code,
            detail: format!("{code} happened"),
        }
    }

    #[tokio::test]
    async fn an_error_falls_through_to_the_next_voice() {
        let first = Stub::fails("First", backend_error("local_tts_unreachable"));
        let second = Stub::ok("Second");
        let chain = ChainSpeech::new(vec![first.clone(), second.clone()]);
        chain.speak("hello", None).await.expect("second speaks");
        assert_eq!(first.calls(), 1);
        assert_eq!(second.calls(), 1);
        assert_eq!(chain.describe().chain, vec!["First", "Second"]);
    }

    #[tokio::test]
    async fn silence_falls_through_like_any_other_refusal() {
        let first = Stub::fails("First", backend_error("wit_silent_audio"));
        let second = Stub::ok("Second");
        let chain = ChainSpeech::new(vec![first, second]);
        chain.speak("hello", None).await.expect("second speaks");
    }

    #[tokio::test]
    async fn when_every_voice_fails_the_last_error_is_the_answer() {
        let chain = ChainSpeech::new(vec![
            Stub::fails("First", backend_error("local_tts_unreachable")),
            Stub::fails("Second", backend_error("wit_rate_limited")),
        ]);
        let error = chain.speak("hello", None).await.expect_err("all failed");
        assert_eq!(error.code(), "wit_rate_limited");
    }

    #[tokio::test]
    async fn empty_text_never_reaches_a_provider() {
        let first = Stub::ok("First");
        let chain = ChainSpeech::new(vec![first.clone()]);
        let error = chain.speak("   ", None).await.expect_err("empty");
        assert_eq!(error.code(), "nothing_to_read");
        assert_eq!(first.calls(), 0);
    }

    #[tokio::test]
    async fn the_stream_names_the_voice_that_spoke() {
        let first = Stub::fails("First", backend_error("local_tts_error"));
        let second = Stub::ok("Second voice");
        let chain = ChainSpeech::new(vec![first, second]);
        let stream = chain
            .speak_stream_voiced("hello", None, None, None)
            .await
            .expect("stream");
        assert_eq!(stream.voice.as_deref(), Some("Second voice"));
    }
}
