//! An in-memory Wit.ai stand-in for tests and for local development.
//!
//! The same design note as [`crate::elevenlabs::fake`]: a fake that
//! answers every call cannot fail, so a test against it would be
//! self-certifying. This one holds the app token it will accept and
//! refuses anything else the way Wit.ai refuses it (401), and it can be
//! told to rate-limit, to stall, and to answer with silence — the three
//! behaviours read-aloud must survive loudly.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;

use super::{Voice, WitClient, WitError};
use crate::config::Secret;

/// The app token [`FakeWit::new`] accepts. Anything else is a 401.
pub const VALID_TOKEN: &str = "wit-fake-token-not-a-real-one";
/// The voices [`FakeWit::new`] lists.
pub const KNOWN_VOICES: &[&str] = &["wit$Remi", "wit$Clara"];

/// One synthesize request the fake was asked for, after the token check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recorded {
    /// The sentence-sized text.
    pub text: String,
    /// The voice it was asked in.
    pub voice: String,
    /// The pace it was asked for.
    pub speed_percent: Option<u32>,
}

/// Build a small valid WAV carrying `samples` (16-bit mono, 16 kHz).
#[must_use]
pub fn fake_wav(samples: &[i16]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut wav = Vec::with_capacity(44 + samples.len() * 2);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36_u32.saturating_add(data_len)).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt \x10\0\0\0\x01\0\x01\0");
    wav.extend_from_slice(&16_000_u32.to_le_bytes());
    wav.extend_from_slice(&32_000_u32.to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    wav
}

/// What the fake answers, when a test has chosen.
#[derive(Clone, Debug, PartialEq)]
enum Behaviour {
    /// A tone whose pitch follows the text, so tests can tell texts apart.
    Speak,
    /// 429 for the next `remaining` calls, then speak.
    RateLimited { remaining: u32 },
    /// Valid WAV in which every sample is zero.
    Silent,
    /// A successful status with no bytes at all.
    Empty,
    /// Wait, then speak.
    Slow(Duration),
}

/// In-memory Wit.ai.
#[derive(Debug)]
pub struct FakeWit {
    valid_token: String,
    state: Arc<Mutex<State>>,
}

#[derive(Debug)]
struct State {
    behaviour: Behaviour,
    requests: Vec<Recorded>,
    voices_calls: usize,
}

impl Default for FakeWit {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeWit {
    /// A fake holding [`VALID_TOKEN`] and [`KNOWN_VOICES`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            valid_token: VALID_TOKEN.to_owned(),
            state: Arc::new(Mutex::new(State {
                behaviour: Behaviour::Speak,
                requests: Vec::new(),
                voices_calls: 0,
            })),
        }
    }

    /// Answer 429 to the next `times` synthesize calls, then speak.
    pub fn rate_limit_next(&self, times: u32) {
        self.lock().behaviour = Behaviour::RateLimited { remaining: times };
    }

    /// Answer with silence: a valid WAV of zeros.
    pub fn be_silent(&self) {
        self.lock().behaviour = Behaviour::Silent;
    }

    /// Answer with a successful status and no audio bytes.
    pub fn be_empty(&self) {
        self.lock().behaviour = Behaviour::Empty;
    }

    /// Wait `delay` before answering every call.
    pub fn be_slow(&self, delay: Duration) {
        self.lock().behaviour = Behaviour::Slow(delay);
    }

    /// Speak normally again.
    pub fn speak(&self) {
        self.lock().behaviour = Behaviour::Speak;
    }

    /// Every synthesize the fake was asked for, in order.
    #[must_use]
    pub fn requests(&self) -> Vec<Recorded> {
        self.lock().requests.clone()
    }

    /// How many times the voices list was asked for.
    #[must_use]
    pub fn voices_calls(&self) -> usize {
        self.lock().voices_calls
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn check_token(&self, token: &Secret) -> Result<(), WitError> {
        if token.expose() == self.valid_token {
            return Ok(());
        }
        // Deliberately quoting the token back, as an upstream is free to:
        // redaction in `from_response` is what keeps it out of the error.
        Err(WitError::from_response(
            401,
            &format!("the token {} is not valid", token.expose()),
            token,
            1,
        ))
    }
}

#[async_trait]
impl WitClient for FakeWit {
    async fn synthesize(
        &self,
        token: &Secret,
        text: &str,
        voice: &str,
        speed_percent: Option<u32>,
    ) -> Result<Vec<u8>, WitError> {
        self.check_token(token)?;
        let behaviour = {
            let mut state = self.lock();
            state.requests.push(Recorded {
                text: text.to_owned(),
                voice: voice.to_owned(),
                speed_percent,
            });
            match &mut state.behaviour {
                Behaviour::RateLimited { remaining } if *remaining > 0 => {
                    *remaining -= 1;
                    return Err(WitError::from_response(429, "rate limited", token, 1));
                }
                other => other.clone(),
            }
        };
        match behaviour {
            Behaviour::Silent => Ok(fake_wav(&[0; 1600])),
            Behaviour::Empty => Ok(Vec::new()),
            Behaviour::Slow(delay) => {
                tokio::time::sleep(delay).await;
                Ok(fake_wav(&[900; 800]))
            }
            _ => {
                // Distinct per text, so a test can prove which sentence was read.
                let pitch = 400 + (text.len() as i16 % 40) * 10;
                Ok(fake_wav(&[pitch; 800]))
            }
        }
    }

    async fn voices(&self, token: &Secret) -> Result<Vec<Voice>, WitError> {
        self.check_token(token)?;
        self.lock().voices_calls += 1;
        Ok(KNOWN_VOICES
            .iter()
            .map(|name| Voice {
                name: (*name).to_owned(),
                locale: "en".to_owned(),
                gender: None,
                styles: vec!["default".to_owned()],
            })
            .collect())
    }
}
