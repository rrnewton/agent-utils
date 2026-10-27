//! Small, content-free dependency checks, independent of browser activity.
//!
//! Every ten minutes, mint a conversation URL without opening a billed call and synthesize
//! "OK." through the configured ElevenLabs read-aloud path. Never log URLs, keys or audio.

use std::time::Duration;

use futures_util::StreamExt as _;

use crate::state::AppState;

/// The dependency check cadence; `/healthz` remains process liveness only.
pub const INTERVAL: Duration = Duration::from_secs(600);
const DEADLINE: Duration = Duration::from_secs(30);

async fn speech(state: &AppState) -> Result<(), &'static str> {
    let mut stream = state
        .speech
        .speak_stream("OK.", None)
        .await
        .map_err(|error| error.code())?;
    // The browser's incremental decoder accepts PCM WAV, not an arbitrary encoded audio body.
    let header = crate::speech::wav_header(u32::MAX, 24_000);
    if stream.content_type != "audio/wav" || stream.preamble.as_deref() != Some(header.as_slice()) {
        return Err("unexpected_audio_format");
    }
    let mut bytes = 0;
    let mut nonzero = false;
    while let Some(chunk) = stream.chunks.next().await {
        let chunk = chunk.map_err(|error| error.code())?;
        bytes += chunk.len();
        nonzero |= chunk.iter().any(|byte| *byte != 0);
        if bytes > 2_000_000 {
            return Err("audio_too_large");
        }
    }
    if bytes == 0 || bytes % 2 != 0 || !nonzero {
        return Err("empty_or_silent_audio");
    }
    Ok(())
}

async fn bounded(
    check: impl std::future::Future<Output = Result<(), &'static str>>,
) -> Result<(), &'static str> {
    tokio::time::timeout(DEADLINE, check)
        .await
        .unwrap_or(Err("timeout"))
}

/// Check configured ElevenLabs capabilities once. `None` means that capability is not selected.
pub async fn check(state: &AppState) -> [Option<Result<(), &'static str>>; 2] {
    let auth = async {
        state.config.elevenlabs.agent_id.as_ref()?;
        Some(
            bounded(async {
                state
                    .elevenlabs
                    .signed_url(&state.config.elevenlabs)
                    .await
                    .map(|_| ())
                    .map_err(|error| error.code())
            })
            .await,
        )
    };
    let audio = async {
        if state.speech.describe().backend != "elevenlabs" {
            return None;
        }
        Some(bounded(speech(state)).await)
    };
    let (auth, audio) = tokio::join!(auth, audio);
    [auth, audio]
}

/// Probe immediately, then every ten minutes. Failures are WARN lines with safe error codes.
/// This detects dependency failures, not a phone's speaker, microphone or WebSocket path.
pub async fn run(state: AppState) {
    let mut every = tokio::time::interval(INTERVAL);
    every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        every.tick().await;
        for (component, result) in ["elevenlabs_auth", "read_aloud"]
            .into_iter()
            .zip(check(&state).await)
        {
            match result {
                Some(Ok(())) => tracing::info!(component, "voice dependency check passed"),
                Some(Err(code)) => {
                    tracing::warn!(component, code, "voice dependency check FAILED")
                }
                None => tracing::debug!(component, "voice dependency check not selected"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speech::{Description, Playback, Speech, SpeechError, SpeechProvider, SpeechStream};
    use std::sync::Arc;

    struct Audio {
        mime: &'static str,
        samples: &'static [u8],
    }

    #[async_trait::async_trait]
    impl SpeechProvider for Audio {
        fn describe(&self) -> Description {
            Description {
                backend: "elevenlabs",
                label: "test".to_owned(),
                playback: Playback::Audio,
                local_only: false,
                chain: Vec::new(),
            }
        }

        async fn speak(&self, _: &str, _: Option<f64>) -> Result<Speech, SpeechError> {
            unreachable!("the check must exercise the streaming path")
        }

        async fn speak_stream(
            &self,
            text: &str,
            _: Option<f64>,
        ) -> Result<SpeechStream, SpeechError> {
            assert_eq!(text, "OK.");
            Ok(SpeechStream {
                content_type: self.mime.to_owned(),
                preamble: Some(crate::speech::wav_header(u32::MAX, 24_000).into()),
                session: None,
                voice: None,
                chunks: futures_util::stream::iter([Ok(bytes::Bytes::from_static(self.samples))])
                    .boxed(),
            })
        }
    }

    #[tokio::test]
    async fn success_requires_decodable_nonsilent_audio_and_auth_is_independent() {
        let (mut state, _, vendor) = crate::testing::state_parts();
        for (mime, samples, expected) in [
            ("audio/ogg", &[1, 0][..], Err("unexpected_audio_format")),
            ("audio/wav", &[][..], Err("empty_or_silent_audio")),
            ("audio/wav", &[0, 0][..], Err("empty_or_silent_audio")),
            ("audio/wav", &[1][..], Err("empty_or_silent_audio")),
            ("audio/wav", &[1, 0][..], Ok(())),
        ] {
            state.speech = Arc::new(Audio { mime, samples });
            assert_eq!(check(&state).await, [Some(Ok(())), Some(expected)]);
        }
        vendor.fail_next("unreachable");
        let results = check(&state).await;
        assert!(results[0].expect("auth selected").is_err());
        assert_eq!(results[1], Some(Ok(())));
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_dependency_has_a_deadline() {
        let started = tokio::time::Instant::now();
        assert_eq!(bounded(std::future::pending()).await, Err("timeout"));
        assert_eq!(started.elapsed(), DEADLINE);
    }
}
