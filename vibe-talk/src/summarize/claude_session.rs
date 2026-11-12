//! A warm persistent Claude session as a summariser. `#1331 claude-session-summarizer`.
//!
//! A pool of 1-2 [`claude_stream_session::session::Session`] processes stays started, so a summary
//! pays no cold startup: measured cold per call at 11.8-14.2 s, warm turns at 1.2-2.1 s
//! (liaison measurement on the filing of #1331). Tools are off (the driver never enables
//! them), the model is chosen per summary kind through `[summaries.llm.models]`
//! (`#1324 per-kind-summarizer-model`), and sessions recycle after a configured number of
//! turns or tokens and restart on death. Requests queue fairly: the pool mutex is held
//! only to take or return a session, and slots are bounded by a FIFO semaphore.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use claude_stream_session::session::{Error as SessionError, Session, SessionConfig};
use tokio::sync::{Mutex, Semaphore};

use crate::config::{LlmConfig, SummaryKind};

use super::threads::{parse_thread_answer, ThreadNaming, ThreadRequest, THREAD_RULE, THREAD_STYLE};
use super::{Summarizer, Summary, SummaryError, SummaryRequest, PROMPT};

/// The slug this backend contributes to the cache key.
pub const BACKEND: &str = "claude_session";

struct Pool {
    config: LlmConfig,
    slots: Semaphore,
    idle: Mutex<HashMap<String, Vec<Session>>>,
}

impl Pool {
    fn session_config(&self, model: &str) -> SessionConfig {
        SessionConfig {
            program: self.config.claude_program.clone(),
            model: Some(model.to_owned()),
            effort: self.config.effort.clone(),
            arguments: Vec::new(),
        }
    }

    async fn complete(&self, model: &str, prompt: &str) -> Result<String, SummaryError> {
        let _permit = self
            .slots
            .acquire()
            .await
            .map_err(|_| SummaryError::Transport("claude session pool closed".to_owned()))?;
        let pooled = {
            let mut idle = self.idle.lock().await;
            idle.get_mut(model).and_then(Vec::pop)
        };
        let config = self.session_config(model);
        let prompt = prompt.to_owned();
        let (session, result) = tokio::task::spawn_blocking(move || {
            let mut session = match pooled {
                Some(session) => session,
                None => match Session::spawn(config) {
                    Ok(session) => session,
                    Err(error) => return (None, Err(map_error(error))),
                },
            };
            let result = session.complete(&prompt).map(|turn| turn.text);
            (Some(session), result.map_err(map_error))
        })
        .await
        .map_err(|error| SummaryError::Transport(format!("claude session task failed: {error}")))?;
        let text = match result {
            Ok(text) => text,
            Err(error) => return Err(error),
        };
        if let Some(session) = session {
            let recycle = session.turns() >= self.config.recycle_turns
                || session.tokens() >= self.config.recycle_tokens;
            if !recycle {
                self.idle
                    .lock()
                    .await
                    .entry(model.to_owned())
                    .or_default()
                    .push(session);
            }
        }
        Ok(text)
    }
}

fn map_error(error: SessionError) -> SummaryError {
    match error {
        SessionError::Spawn(error) => SummaryError::NotConfigured(Box::leak(
            format!("summaries.llm.claude_program ({error})").into_boxed_str(),
        )),
        SessionError::Died => SummaryError::Transport("claude session ended mid-turn".to_owned()),
        SessionError::Refused(message) => SummaryError::Refused(message),
        SessionError::Shape(message) => SummaryError::Shape(message),
    }
}

/// The `claude_session` summariser: a warm pool behind the [`Summarizer`] trait.
pub struct ClaudeSessionSummarizer {
    config: LlmConfig,
    pool: Arc<Pool>,
    policy: String,
}

impl ClaudeSessionSummarizer {
    /// Build the summariser; processes start lazily on first use per model.
    #[must_use]
    pub fn new(config: LlmConfig) -> Self {
        let policy = format!(
            "{PROMPT}\nbackend={BACKEND}\nmodel={}\neffort={}",
            config.model,
            config.effort.as_deref().unwrap_or("")
        );
        let pool = Arc::new(Pool {
            slots: Semaphore::new(config.pool_size as usize),
            idle: Mutex::new(HashMap::new()),
            config: config.clone(),
        });
        Self {
            config,
            pool,
            policy,
        }
    }

    async fn complete(&self, kind: SummaryKind, prompt: &str) -> Result<String, SummaryError> {
        let model = self.config.model_for(kind).to_owned();
        self.pool.complete(&model, prompt).await
    }
}

#[async_trait]
impl Summarizer for ClaudeSessionSummarizer {
    fn describe(&self) -> &'static str {
        "Claude session"
    }

    fn backend(&self) -> &'static str {
        BACKEND
    }

    fn policy_input(&self) -> &str {
        &self.policy
    }

    fn names_threads(&self) -> bool {
        true
    }

    async fn name_thread(&self, request: &ThreadRequest<'_>) -> Result<ThreadNaming, SummaryError> {
        let prompt = format!(
            "{THREAD_RULE}\n{THREAD_STYLE}\nKeep the sentence under {} characters.\n\n{}",
            request.sentence_chars,
            request.prompt()
        );
        let reply = self.complete(SummaryKind::ThreadName, &prompt).await?;
        parse_thread_answer(&reply, request.sentence_chars)
    }

    async fn summarize(&self, request: &SummaryRequest<'_>) -> Result<Summary, SummaryError> {
        let text = self
            .complete(SummaryKind::Message, request.prompt())
            .await?;
        let (text, cut_at) = super::agent::plain_uncut(&text, request.target_chars);
        Ok(Summary { text, cut_at })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(program: String) -> LlmConfig {
        LlmConfig {
            backend: crate::config::LlmBackend::ClaudeSession,
            claude_program: program,
            model: "claude-haiku-5-5".to_owned(),
            ..super::super::tests::llm()
        }
    }

    #[tokio::test]
    async fn missing_program_names_the_setting() {
        let summarizer = ClaudeSessionSummarizer::new(config("/nonexistent/claude".to_owned()));
        let message = crate::model::Message {
            edited_timestamp: None,
            thread: None,
            id: crate::model::MessageId("1".to_owned()),
            channel_id: crate::model::ChannelId("100".to_owned()),
            author: "a".to_owned(),
            author_id: crate::model::UserId("2".to_owned()),
            author_is_bot: false,
            content: "a long message that needs a summary".to_owned(),
            spoken_content: String::new(),
            content_html: String::new(),
            noise: false,
            timestamp: "2026-01-01T00:00:00Z".to_owned(),
            spoken_time: "midnight".to_owned(),
            reply_to: None,
            reactions: None,
            heading: None,
            mentions: Vec::new(),
            upstream_url: None,
            upstream_app_url: None,
            op_key: None,
            observed_ms: None,
        };
        let error = summarizer
            .summarize(&SummaryRequest::new(&message, &[], 160))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "summarizer_not_configured");
    }
}
