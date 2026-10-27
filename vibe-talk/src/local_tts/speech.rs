//! Adapts the local sidecar client to the shared read-aloud interface.
//!
//! The same enforcement as [`crate::wit::speech`], around a local client:
//! sentence-sized requests (the shared chunking), at most two at a time,
//! a small cache, and per-sentence validation before anything is cached
//! or handed to the player — silence and malformed audio stop the read.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{Mutex, Semaphore};

use super::{LocalError, LocalTtsClient};
use crate::config::LocalTtsConfig;
use crate::speech::{Description, Playback, Speech, SpeechError, SpeechProvider, SpeechStream};
use crate::wit::sentence_chunks;
use crate::wit::speech::{is_silent, merge_wavs};

/// How many synthesize calls may be in flight at once. Two, as in
/// [`crate::wit::speech`]: the page's prefetch can overlap the current
/// block's tail, and a CPU model gains nothing from a deeper queue.
pub const MAX_IN_FLIGHT: usize = 2;

/// How many synthesized sentence results the cache holds.
pub const CACHE_ENTRIES: usize = 128;

type CacheKey = (String, String, Option<u32>);

/// Local read-aloud with its configuration held behind the provider boundary.
pub struct LocalSpeech {
    client: Arc<dyn LocalTtsClient>,
    config: LocalTtsConfig,
    cache: Mutex<HashMap<CacheKey, Vec<u8>>>,
    cache_order: Mutex<VecDeque<CacheKey>>,
    in_flight: Semaphore,
}

impl LocalSpeech {
    /// Use the given sidecar client and the deployment's local settings.
    #[must_use]
    pub fn new(client: Arc<dyn LocalTtsClient>, config: LocalTtsConfig) -> Self {
        Self {
            client,
            config,
            cache: Mutex::new(HashMap::new()),
            cache_order: Mutex::new(VecDeque::new()),
            in_flight: Semaphore::new(MAX_IN_FLIGHT),
        }
    }

    fn voice(&self, chosen: Option<&str>) -> Result<String, SpeechError> {
        chosen
            .map(str::trim)
            .filter(|voice| !voice.is_empty())
            .or_else(|| {
                self.config
                    .voice
                    .as_deref()
                    .map(str::trim)
                    .filter(|voice| !voice.is_empty())
            })
            .map(str::to_owned)
            .ok_or_else(|| SpeechError::NotConfigured {
                code: "local_tts_not_configured",
                detail: "local_tts.voice is not configured and no voice was chosen in \
                         Settings; name one in the configuration, for example \
                         voice = \"af_heart\""
                    .to_owned(),
            })
    }

    async fn cached(&self, key: &CacheKey) -> Option<Vec<u8>> {
        self.cache.lock().await.get(key).cloned()
    }

    async fn store(&self, key: CacheKey, audio: Vec<u8>) {
        let mut cache = self.cache.lock().await;
        let mut order = self.cache_order.lock().await;
        if !cache.contains_key(&key) {
            order.push_back(key.clone());
            while order.len() > CACHE_ENTRIES {
                if let Some(oldest) = order.pop_front() {
                    cache.remove(&oldest);
                }
            }
        }
        cache.insert(key, audio);
    }

    async fn speak_with(
        &self,
        text: &str,
        speed: Option<f64>,
        chosen_voice: Option<&str>,
    ) -> Result<Speech, SpeechError> {
        if text.trim().is_empty() {
            return Err(SpeechError::Empty);
        }
        if self
            .config
            .endpoint
            .as_deref()
            .map(str::trim)
            .is_none_or(|endpoint| endpoint.is_empty())
        {
            return Err(SpeechError::NotConfigured {
                code: "local_tts_not_configured",
                detail: "local_tts.endpoint is not configured; this server cannot read \
                         messages aloud with the local voice"
                    .to_owned(),
            });
        }
        let voice = self.voice(chosen_voice)?;
        let pace = crate::speech::clamp_speed(speed.or(self.config.speed));
        let pace_key = pace.map(|value| (value * 100.0).round() as u32);
        let mut pieces = Vec::new();
        for chunk in sentence_chunks(text) {
            let key: CacheKey = (chunk.clone(), voice.clone(), pace_key);
            if let Some(audio) = self.cached(&key).await {
                pieces.push(audio);
                continue;
            }
            let _permit = self
                .in_flight
                .acquire()
                .await
                .map_err(|_| SpeechError::Backend {
                    code: "local_tts_error",
                    detail: "the local speech request queue closed".to_owned(),
                })?;
            let audio = self
                .client
                .synthesize(&chunk, &voice, pace)
                .await
                .map_err(shared_error)?;
            // Validate BEFORE the cache, per sentence, exactly as the
            // Wit.ai adapter does: silence or malformed audio is this
            // read's failure, never a replayed result.
            crate::wit::speech::parse_wav_for_merge(&audio)
                .map_err(|error| shared_error(LocalError::Shape(error.to_string())))?;
            if is_silent(&audio) {
                return Err(SpeechError::Backend {
                    code: "local_tts_silent_audio",
                    detail: "the local voice returned only silence for this message; \
                             nothing was played"
                        .to_owned(),
                });
            }
            self.store(key, audio.clone()).await;
            pieces.push(audio);
        }
        let audio = merge_wavs(&pieces)
            .map_err(|error| shared_error(LocalError::Shape(error.to_string())))?;
        Ok(Speech {
            audio,
            content_type: crate::wit::AUDIO_CONTENT_TYPE.to_owned(),
        })
    }
}

fn shared_error(error: LocalError) -> SpeechError {
    SpeechError::Backend {
        code: error.code(),
        detail: error.to_string(),
    }
}

#[async_trait]
impl SpeechProvider for LocalSpeech {
    fn describe(&self) -> Description {
        Description {
            backend: "local",
            label: "Local voice".to_owned(),
            playback: Playback::Audio,
            local_only: false,
            chain: Vec::new(),
        }
    }

    async fn warm_up(&self) -> Result<(), SpeechError> {
        // Listing the voices proves the sidecar is up before the reader
        // taps, without synthesizing anything.
        self.client.voices().await.map_err(shared_error)?;
        Ok(())
    }

    async fn speak(&self, text: &str, speed: Option<f64>) -> Result<Speech, SpeechError> {
        self.speak_with(text, speed, None).await
    }

    async fn speak_stream_voiced(
        &self,
        text: &str,
        speed: Option<f64>,
        _selection: Option<&str>,
        voice: Option<&str>,
    ) -> Result<SpeechStream, SpeechError> {
        let spoken = self.speak_with(text, speed, voice).await?;
        Ok(SpeechStream {
            content_type: spoken.content_type,
            preamble: None,
            voice: None,
            session: None,
            chunks: Box::pin(futures_util::stream::once(async move {
                Ok(bytes::Bytes::from(spoken.audio))
            })),
        })
    }

    async fn voices(&self) -> Result<Vec<crate::speech::VoiceInfo>, SpeechError> {
        self.client.voices().await.map_err(shared_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_tts::fake::FakeLocalTts;

    fn config() -> LocalTtsConfig {
        LocalTtsConfig {
            endpoint: Some("http://127.0.0.1:18771".to_owned()),
            voice: Some("af_heart".to_owned()),
            speed: None,
        }
    }

    #[tokio::test]
    async fn a_silent_sidecar_stops_the_read_and_is_never_cached() {
        let fake = Arc::new(FakeLocalTts::new());
        let speech = LocalSpeech::new(fake.clone(), config());
        fake.be_silent();
        let error = speech.speak("hello there", None).await.expect_err("silent");
        assert_eq!(error.code(), "local_tts_silent_audio");
        fake.speak();
        speech
            .speak("hello there", None)
            .await
            .expect("asked again");
        assert_eq!(fake.requests().len(), 2);
    }

    #[tokio::test]
    async fn a_refusing_sidecar_is_loud() {
        let fake = Arc::new(FakeLocalTts::new());
        fake.refuse();
        let speech = LocalSpeech::new(fake, config());
        let error = speech
            .speak("hello there", None)
            .await
            .expect_err("refused");
        assert_eq!(error.code(), "local_tts_error");
    }

    #[tokio::test]
    async fn no_endpoint_is_not_configured() {
        let fake = Arc::new(FakeLocalTts::new());
        let mut bare = config();
        bare.endpoint = None;
        let speech = LocalSpeech::new(fake, bare);
        let error = speech.speak("hello there", None).await.expect_err("bare");
        assert_eq!(error.code(), "local_tts_not_configured");
        assert!(error.to_string().contains("local_tts.endpoint"));
    }
}
