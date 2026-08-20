//! The shipped default: no model, no network, no cost.
//!
//! It reuses [`crate::summary::condense`] rather than reimplementing the code-and-link flattening
//! that already exists there, so the digest line and the summary line cannot drift into two
//! different ideas of what "speakable" means.
//!
//! # Why cache a deterministic function
//!
//! It looks like ceremony, and the honest answer is that the cache is not here for this backend.
//! It is here because the version key is expensive to retrofit and cheap to ship, and because
//! shipping an extractive default keeps the deployment runnable with no new secret. What the
//! cache buys today is that the wiring — the key, the invalidation, the sweep — is exercised by
//! the tests and by every real run, rather than being written for the first time on the day a
//! model is configured.

use async_trait::async_trait;

use super::{Summarizer, SummaryError, SummaryRequest};

/// Condensation, behind the [`Summarizer`] trait.
#[derive(Clone, Copy, Debug, Default)]
pub struct ExtractiveSummarizer;

/// The name this backend contributes to [`super::policy_version`].
pub const BACKEND: &str = "extractive";

#[async_trait]
impl Summarizer for ExtractiveSummarizer {
    fn describe(&self) -> &'static str {
        "extractive (truncation, no model, no network, no cost)"
    }

    async fn summarize(&self, request: &SummaryRequest<'_>) -> Result<String, SummaryError> {
        // The context is deliberately unused: truncation cannot make use of it, and pretending
        // otherwise by concatenating neighbours would produce a summary of the wrong message.
        // A model backend is what the context is there for.
        Ok(crate::summary::condense(
            &request.target.content,
            request.target_chars,
        ))
    }
}
