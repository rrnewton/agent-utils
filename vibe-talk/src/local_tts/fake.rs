//! An in-memory local-sidecar stand-in for tests.
//!
//! The same design note as [`crate::wit::fake`]: it can answer with
//! silence and can stall, because a fake that can only say yes would
//! certify nothing about the loud-failure behaviour.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;

use super::{LocalError, LocalTtsClient, Voice};

/// The voices [`FakeLocalTts::new`] lists.
pub const KNOWN_VOICES: &[&str] = &["af_heart", "am_michael"];

/// One synthesize request the fake was asked for.
#[derive(Clone, Debug, PartialEq)]
pub struct Recorded {
    /// The sentence-sized text.
    pub text: String,
    /// The voice it was asked in.
    pub voice: String,
    /// The pace it was asked for.
    pub speed: Option<f64>,
}

/// What the fake answers, when a test has chosen.
#[derive(Clone, Debug, PartialEq)]
enum Behaviour {
    Speak,
    Silent,
    Empty,
    Slow(Duration),
    Refuse,
}

/// In-memory local sidecar.
#[derive(Debug)]
pub struct FakeLocalTts {
    state: Arc<Mutex<State>>,
}

#[derive(Debug)]
struct State {
    behaviour: Behaviour,
    requests: Vec<Recorded>,
    voices_calls: usize,
}

impl Default for FakeLocalTts {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeLocalTts {
    /// A fake listing [`KNOWN_VOICES`] and speaking at once.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                behaviour: Behaviour::Speak,
                requests: Vec::new(),
                voices_calls: 0,
            })),
        }
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

    /// Refuse every call with a 500, as a crashed model would surface.
    pub fn refuse(&self) {
        self.lock().behaviour = Behaviour::Refuse;
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
}

#[async_trait]
impl LocalTtsClient for FakeLocalTts {
    async fn synthesize(
        &self,
        text: &str,
        voice: &str,
        speed: Option<f64>,
    ) -> Result<Vec<u8>, LocalError> {
        let behaviour = {
            let mut state = self.lock();
            state.requests.push(Recorded {
                text: text.to_owned(),
                voice: voice.to_owned(),
                speed,
            });
            state.behaviour.clone()
        };
        match behaviour {
            Behaviour::Refuse => Err(LocalError::from_response(500, "model crashed")),
            Behaviour::Silent => Ok(crate::wit::fake::fake_wav(&[0; 1600])),
            Behaviour::Empty => Ok(Vec::new()),
            Behaviour::Slow(delay) => {
                tokio::time::sleep(delay).await;
                Ok(crate::wit::fake::fake_wav(&[900; 800]))
            }
            Behaviour::Speak => Ok(crate::wit::fake::fake_wav(&[700; 800])),
        }
    }

    async fn voices(&self) -> Result<Vec<Voice>, LocalError> {
        self.lock().voices_calls += 1;
        Ok(KNOWN_VOICES
            .iter()
            .map(|name| Voice {
                name: (*name).to_owned(),
                locale: "en".to_owned(),
                gender: None,
                styles: Vec::new(),
            })
            .collect())
    }
}
