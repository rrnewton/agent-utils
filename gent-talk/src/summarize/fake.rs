//! A [`Summarizer`] for tests that counts, records, and can refuse.
//!
//! The three assertions #49 turns on are all about how OFTEN the summariser is called — never for
//! a short message, once for a repeated request, again after the policy or the content changes —
//! and none of them can be made against a backend that does not count. This one does, and it can
//! be made to fail, which is what supplies the controls.

use std::sync::Mutex;

use async_trait::async_trait;

use super::{Summarizer, SummaryError, SummaryRequest};

/// What one call was asked to summarise.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedRequest {
    /// The text of the message it was given.
    pub target: String,
    /// How many context messages rode along.
    pub context: usize,
    /// The width it was asked for.
    pub target_chars: usize,
    /// EXACTLY what a model backend would have sent — [`super::SummaryRequest::prompt`], captured
    /// rather than rebuilt. A test that constructs the prompt itself proves only that the test
    /// can build a fence; this proves the summariser was handed one.
    pub prompt: String,
}

/// A summariser that answers with a fixed shape and remembers being asked.
#[derive(Debug, Default)]
pub struct FakeSummarizer {
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    requests: Vec<RecordedRequest>,
    fail_next: Option<String>,
}

impl FakeSummarizer {
    /// A fresh one.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many times it has actually been asked to produce a summary.
    #[must_use]
    pub fn calls(&self) -> usize {
        self.lock().requests.len()
    }

    /// Everything it was asked, in order.
    #[must_use]
    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.lock().requests.clone()
    }

    /// Make the next call fail. One-shot, so recovery is testable too.
    pub fn fail_next(&self, why: &str) {
        self.lock().fail_next = Some(why.to_owned());
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[async_trait]
impl Summarizer for FakeSummarizer {
    fn describe(&self) -> &'static str {
        "an in-memory summariser that counts its calls"
    }

    // Deliberately the SHIPPED slug rather than one of its own: the cache tests substitute this
    // for the real summariser, and a "fake" slug would move every key they exercise off the
    // production prefix while leaving all of them green.
    //
    // It borrows the slug and not the whole key: this fake takes the DEFAULT `policy_input`, so
    // the hash after the prefix is not the one a deployment writes. What the shipped key really
    // is, is pinned by `policy_version_for`'s own tests and by the startup sweep test, not here.
    fn backend(&self) -> &'static str {
        super::agent::BACKEND
    }

    async fn summarize(&self, request: &SummaryRequest<'_>) -> Result<String, SummaryError> {
        let mut state = self.lock();
        if let Some(why) = state.fail_next.take() {
            return Err(SummaryError::Transport(why));
        }
        state.requests.push(RecordedRequest {
            target: request.target.content.clone(),
            context: request.context.len(),
            target_chars: request.target_chars,
            prompt: request.prompt().to_owned(),
        });
        // Deliberately NOT the message text: a fake that echoed its input would let a caller that
        // returned the raw message pass as one that summarised it.
        Ok(format!("summary #{}", state.requests.len()))
    }
}
