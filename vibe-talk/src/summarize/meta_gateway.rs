//! A summariser that is ONE plain HTTP call to an AI gateway: no agent harness, no
//! conversation, and no tools. `#1027 vibe-talk-spark-summarizer`.
//!
//! # Why a second HTTP summariser
//!
//! [`super::messages_api`] speaks the Messages API in its Anthropic and Vertex dialects. The
//! gateway this backend calls — the same upstream the Muse on the host uses — speaks
//! neither: it answers an OpenAI-compatible chat-completions shape (`choices[0].message.content`)
//! at `POST {base_url}/chat/completions`, authenticated by mutual TLS and nothing else. That is
//! a different request type, a different response type and a different credential, so it is a
//! backend of its own (`backend = "meta_gateway"` under `[summaries.llm]`) rather than a third
//! dialect squeezed into a Messages-shaped body.
//!
//! Everything that is NOT the wire shape is shared with [`super::messages_api`] and imported
//! from it, so the two cannot drift: the HTTP client (extra CA roots, mTLS identity re-read when
//! it rotates on disk, no redirects), the one [`RateLimiter`] discipline (shared by summaries
//! and thread names, paused for everyone on a 429), the retry-on-429/overload policy under one
//! deadline, bounded response reads, and failure reporting that repeats nothing this server
//! sent. Credentials are file paths in the configuration, never values in it, and never logged.
//!
//! # No tools, structurally
//!
//! [`ChatRequest`] HAS NO `tools`, `tool_choice` OR `temperature` FIELD, for the reason
//! [`super::messages_api::MessagesRequest`] has none: a model offered no tool cannot call one,
//! whatever the fenced channel text asks of it. A test asserts the serialized body carries none
//! of those keys.
//!
//! # Reasoning is part of the answer's budget
//!
//! The models this gateway serves reason before they write, and the reasoning is charged
//! against `max_tokens`. Two consequences are handled here rather than left to surprise an
//! operator: the request carries an explicit `reasoning_effort` (default
//! [`crate::config::ReasoningEffort::DEFAULT`], folded into the cache key because it changes
//! what a summary says), and the output budget defaults far above the Messages budget
//! ([`crate::config::DEFAULT_META_GATEWAY_MAX_TOKENS`]). An answer that stops at the budget
//! (`finish_reason: "length"`) with no text at all — the whole allowance spent thinking — is a
//! [`SummaryError::Shape`], never an empty summary; one stopped with text is marked as cut, as a
//! Messages `max_tokens` stop is.

use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::time::Instant;

use super::agent::{plain_uncut, PLAIN_TEXT_RULE};
use super::messages_api::{
    build_client, causes, describe_failure, error_code, marked_as_cut, read_bounded, retry_after,
    stamp, unusable_file, BACKGROUND_MAX_WAIT, MAX_ATTEMPTS, MAX_PAUSE,
};
use super::rate_limit::{Priority, RateLimiter};
use super::threads::{parse_thread_answer, ThreadNaming, ThreadRequest, THREAD_RULE, THREAD_STYLE};
use super::{Summarizer, Summary, SummaryError, SummaryRequest, PROMPT};
use crate::config::LlmConfig;

/// The slug this backend contributes to [`super::policy_version`].
pub const BACKEND: &str = "meta-gateway";

/// The whole request body. THERE IS NO `tools`, `tool_choice` OR `temperature` FIELD, and there
/// must never be: see the module doc.
///
/// The budget is sent as `max_completion_tokens`, the one spelling both gateway families this
/// backend has been measured against accept: current Azure-hosted reasoning deployments refuse
/// `max_tokens` outright ("use max_completion_tokens instead"), while the gateway Spark is
/// served from accepts either. The operator's setting keeps its `max_tokens` name — it is the
/// budget either way — and only the wire spelling follows the gateways. There is deliberately
/// no per-model branch: switching models stays a `base_url` and `model` change in
/// configuration, never a code path.
#[derive(Debug, Serialize)]
pub struct ChatRequest<'a> {
    /// The model to ask, exactly as configured.
    pub model: &'a str,
    /// The output budget ([`LlmConfig::max_tokens`]), reasoning included: see the module doc.
    pub max_completion_tokens: u32,
    /// How hard the model reasons before writing, as the gateway spells it. Absent when the
    /// section says `reasoning_effort = "off"`, for a model that refuses the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<&'a str>,
    /// Exactly two turns: the instruction, then the fenced channel text.
    pub messages: [ChatTurn<'a>; 2],
}

/// One turn of the two this backend ever sends.
#[derive(Debug, Serialize)]
pub struct ChatTurn<'a> {
    /// `"system"` for the instruction, `"user"` for the prompt.
    pub role: &'static str,
    /// Trusted text in the system turn; the prompt, fenced, in the user turn.
    pub content: &'a str,
}

impl<'a> ChatRequest<'a> {
    /// The body `config` sends for one instruction and one prompt.
    #[must_use]
    pub fn new(config: &'a LlmConfig, system: &'a str, prompt: &'a str) -> Self {
        Self {
            model: &config.model,
            max_completion_tokens: config.max_tokens,
            reasoning_effort: config.reasoning_effort.sent(),
            messages: [
                ChatTurn {
                    role: "system",
                    content: system,
                },
                ChatTurn {
                    role: "user",
                    content: prompt,
                },
            ],
        }
    }
}

/// Where `config` sends a request: the gateway's chat-completions path under its base URL.
#[must_use]
pub fn endpoint(config: &LlmConfig) -> String {
    format!("{}/chat/completions", config.base_url)
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    #[serde(default)]
    choices: Vec<ChatChoice>,
    #[serde(default)]
    usage: Option<ChatUsage>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    #[serde(default)]
    message: ChatMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ChatMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    refusal: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ChatUsage {
    #[serde(default)]
    prompt_tokens: Option<u64>,
    #[serde(default)]
    completion_tokens: Option<u64>,
}

/// What this backend has done, for a test to assert on and an operator to read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GatewayStats {
    /// HTTP requests sent, retries included.
    pub requests: usize,
    /// Answers that produced text.
    pub answered: usize,
    /// Times a 429 or an overload was waited out and the call tried again.
    pub retried: usize,
    /// Times the HTTP client was built: once, then again whenever the client identity or the CA
    /// bundle changed on disk.
    pub client_builds: usize,
}

/// Summaries, and thread names, from one gateway call apiece.
pub struct MetaGatewaySummarizer {
    config: LlmConfig,
    policy_input: String,
    limiter: RateLimiter,
    client: Mutex<Option<(Fingerprint, reqwest::Client)>>,
    stats: Mutex<GatewayStats>,
}

impl std::fmt::Debug for MetaGatewaySummarizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // As the Messages summariser: the model is what a debug line needs, never a path.
        f.debug_struct("MetaGatewaySummarizer")
            .field("model", &self.config.model)
            .field("stats", &self.stats())
            .finish_non_exhaustive()
    }
}

/// Which client identity and CA bundle the cached client was built from: each file's
/// modification time and length, or `None` when it is not configured.
type Fingerprint = (Option<(SystemTime, u64)>, Option<(SystemTime, u64)>);

/// One attempt's result.
enum Attempt {
    Answered(String),
    Retry {
        after: Option<Duration>,
        detail: String,
    },
    Failed(SummaryError),
}

impl MetaGatewaySummarizer {
    /// Build a summariser for `config`. Reads no credential and opens no connection: both happen
    /// per call, so a deployment whose identity file is not there yet still starts, and says so.
    #[must_use]
    pub fn new(config: LlmConfig) -> Self {
        // What decides what a summary SAYS, and nothing else: see `LlmConfig`. Both instructions
        // are in, because one cache version covers summaries and thread names alike.
        let policy_input = format!(
            "{PROMPT}\n{PLAIN_TEXT_RULE}\n{THREAD_RULE}\nbackend={BACKEND}\nmodel={}\n\
             max_tokens={}\nreasoning_effort={}",
            config.model,
            config.max_tokens,
            config.reasoning_effort.as_str()
        );
        let limiter = RateLimiter::new(config.requests_per_minute, config.burst);
        Self {
            config,
            policy_input,
            limiter,
            client: Mutex::new(None),
            stats: Mutex::new(GatewayStats::default()),
        }
    }

    /// The configuration this summariser was built from.
    #[must_use]
    pub fn config(&self) -> &LlmConfig {
        &self.config
    }

    /// What this backend has done so far.
    #[must_use]
    pub fn stats(&self) -> GatewayStats {
        *self
            .stats
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn note(&self, change: impl FnOnce(&mut GatewayStats)) {
        change(
            &mut self
                .stats
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
    }

    /// The HTTP client, rebuilt whenever the client identity or the CA bundle changed on disk.
    /// The identity a gateway accepts is short-lived and rotates under a running server, so the
    /// reload is a guarantee, exactly as on the Messages backend, whose [`build_client`] this
    /// shares.
    ///
    /// # Errors
    ///
    /// [`SummaryError::NotConfigured`] naming the file that cannot be read or does not hold what
    /// it should.
    pub fn client(&self) -> Result<reqwest::Client, SummaryError> {
        let fingerprint = (
            stamp(
                self.config.client_identity_file.as_deref(),
                "summaries.llm.client_identity_file",
            )?,
            stamp(
                self.config.ca_bundle_file.as_deref(),
                "summaries.llm.ca_bundle_file",
            )?,
        );
        let mut slot = self
            .client
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((held, client)) = slot.as_ref() {
            if *held == fingerprint {
                return Ok(client.clone());
            }
        }
        let client = build_client(&self.config)?;
        *slot = Some((fingerprint, client.clone()));
        drop(slot);
        self.note(|s| s.client_builds += 1);
        if self.config.client_identity_file.is_some() {
            tracing::info!(
                setting = "summaries.llm.client_identity_file",
                "loaded the summariser's client identity (again, if it changed on disk)"
            );
        }
        Ok(client)
    }

    /// One call: wait for a slot, send, and retry what the gateway asked to be retried. One
    /// deadline covers every wait, retries included, as on the Messages backend.
    async fn complete(
        &self,
        priority: Priority,
        system: &str,
        prompt: &str,
    ) -> Result<String, SummaryError> {
        let max_wait = match priority {
            Priority::Interactive => Duration::from_secs(self.config.timeout_secs),
            Priority::Background => BACKGROUND_MAX_WAIT,
        };
        let began = Instant::now();
        let deadline = began + max_wait;
        let mut last = String::new();
        for attempt in 0..MAX_ATTEMPTS {
            let patience = deadline.saturating_duration_since(Instant::now());
            if let Err(wait) = self.limiter.acquire(priority, patience).await {
                return Err(SummaryError::Transport(format!(
                    "the summariser's rate limit ({} a minute, burst {}) has no free call for \
                     another {}s{}",
                    self.config.requests_per_minute,
                    self.config.burst,
                    wait.as_secs(),
                    if last.is_empty() {
                        String::new()
                    } else {
                        format!("; the gateway last said {last}")
                    }
                )));
            }
            match self.send(system, prompt).await {
                Attempt::Answered(text) => {
                    tracing::debug!(
                        attempts = attempt + 1,
                        elapsed_ms = began.elapsed().as_millis(),
                        "the AI gateway answered"
                    );
                    return Ok(text);
                }
                Attempt::Failed(error) => return Err(error),
                Attempt::Retry { after, detail } => {
                    // The gateway's own wait when it gave one, else 1s, 2s, 4s.
                    let wait = after
                        .unwrap_or_else(|| Duration::from_secs(1 << attempt))
                        .min(MAX_PAUSE);
                    tracing::warn!(
                        backend = BACKEND,
                        wait_ms = wait.as_millis(),
                        attempt = attempt + 1,
                        %detail,
                        "the AI gateway asked us to slow down; every summariser call now waits"
                    );
                    self.limiter.pause_for(wait);
                    self.note(|s| s.retried += 1);
                    last = detail;
                }
            }
        }
        Err(SummaryError::Transport(format!(
            "the AI gateway was still refusing after {MAX_ATTEMPTS} attempts: {last}"
        )))
    }

    /// One HTTP request and its interpretation. The only credential is the client certificate
    /// the shared [`build_client`] put on the connection: no key, no bearer token, and the
    /// loader refuses a section that configures one for this backend.
    async fn send(&self, system: &str, prompt: &str) -> Attempt {
        let client = match self.client() {
            Ok(client) => client,
            Err(error) => return Attempt::Failed(error),
        };
        let body = ChatRequest::new(&self.config, system, prompt);
        let mut request = client
            .post(endpoint(&self.config))
            .timeout(Duration::from_secs(self.config.timeout_secs))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .json(&body);
        for (name, value) in &self.config.extra_headers {
            request = request.header(name.as_str(), value.as_str());
        }

        self.note(|s| s.requests += 1);
        let started = Instant::now();
        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                return Attempt::Failed(SummaryError::Transport(if error.is_timeout() {
                    format!(
                        "the AI gateway did not answer within {}s",
                        self.config.timeout_secs
                    )
                } else {
                    // The whole chain, as on the Messages backend: the reason — refused,
                    // unresolvable, a TLS handshake the gateway rejected — is under reqwest's
                    // own line.
                    format!("the AI gateway could not be reached: {}", causes(&error))
                }));
            }
        };
        let status = response.status();
        let retry = retry_after(response.headers());
        let code = error_code(response.headers());
        let body = match read_bounded(response).await {
            Ok(body) => body,
            Err(error) => return Attempt::Failed(error),
        };
        if status.is_success() {
            return self.interpret(&body, started.elapsed());
        }
        let detail = describe_failure(status, code.as_deref(), &body);
        match status.as_u16() {
            // 529 is an overload answer some gateways keep from the Messages world; 503 is a
            // gateway's own. Both mean "later", as does 429.
            429 | 503 | 529 => Attempt::Retry {
                after: retry,
                detail,
            },
            400..=499 => Attempt::Failed(SummaryError::Refused(detail)),
            _ => Attempt::Failed(SummaryError::Transport(detail)),
        }
    }

    /// Turn a successful response into text, or say why it is not one.
    fn interpret(&self, body: &[u8], elapsed: Duration) -> Attempt {
        let Ok(parsed) = serde_json::from_slice::<ChatResponse>(body) else {
            return Attempt::Failed(SummaryError::Shape(
                "the AI gateway answered 2xx with something that is not a chat-completions \
                 response"
                    .to_owned(),
            ));
        };
        let usage = parsed.usage.unwrap_or_default();
        let Some(choice) = parsed.choices.into_iter().next() else {
            return Attempt::Failed(SummaryError::Shape(
                "the AI gateway answered with no choices".to_owned(),
            ));
        };
        tracing::info!(
            backend = BACKEND,
            model = %self.config.model,
            answer_ms = elapsed.as_millis(),
            finish_reason = choice.finish_reason.as_deref().unwrap_or("(none)"),
            prompt_tokens = usage.prompt_tokens,
            completion_tokens = usage.completion_tokens,
            "the AI gateway answered one summariser call"
        );
        if choice
            .message
            .refusal
            .as_deref()
            .is_some_and(|refusal| !refusal.trim().is_empty())
        {
            return Attempt::Failed(SummaryError::Refused(
                "the model declined to answer".to_owned(),
            ));
        }
        let text = choice.message.content.unwrap_or_default();
        if text.trim().is_empty() {
            // Includes the reasoning-spent-the-budget answer: `finish_reason: "length"` and no
            // text at all. Naming the remedy is the difference between an operator fixing
            // `max_tokens` and one staring at empty rows.
            return Attempt::Failed(SummaryError::Shape(format!(
                "the model answered with no text (finish_reason {}); if it spent its whole \
                 output budget reasoning, raise summaries.llm.max_tokens or lower \
                 summaries.llm.reasoning_effort",
                choice.finish_reason.as_deref().unwrap_or("absent")
            )));
        }
        self.note(|s| s.answered += 1);
        if choice.finish_reason.as_deref() == Some("length") {
            return Attempt::Answered(marked_as_cut(&text));
        }
        Attempt::Answered(text)
    }
}

#[async_trait]
impl Summarizer for MetaGatewaySummarizer {
    fn describe(&self) -> &'static str {
        "one AI gateway call per summary, with no tools (meta_gateway backend)"
    }

    fn backend(&self) -> &'static str {
        BACKEND
    }

    fn policy_input(&self) -> &str {
        &self.policy_input
    }

    fn unavailable_reason(&self) -> Option<String> {
        unusable_file(&self.config).map(|(setting, why)| format!("{setting} {why}"))
    }

    fn names_threads(&self) -> bool {
        true
    }

    async fn summarize(&self, request: &SummaryRequest<'_>) -> Result<Summary, SummaryError> {
        // The same instruction the Messages backend sends, so a deployment that switches
        // backend changes the model and nothing about what it is asked for.
        let system = format!(
            "{PLAIN_TEXT_RULE}\nKeep the answer under {} characters.",
            request.target_chars
        );
        let reply = self
            .complete(Priority::Interactive, &system, request.prompt())
            .await?;
        // Asked for `target_chars`, stored uncut and cut only well past it when served: see
        // `plain_uncut`. `#257 summary-truncation`, `#208 persistent-cache`. A `length` stop
        // was already marked by `marked_as_cut`; it is the gateway's cut, not ours, so it
        // leaves `cut_at` unset.
        let (summary, cut_at) = plain_uncut(&reply, request.target_chars);
        if summary.trim().is_empty() || summary == crate::summary::NO_TEXT {
            return Err(SummaryError::Shape(
                "the model answered with no text at all".to_owned(),
            ));
        }
        Ok(Summary {
            text: summary,
            cut_at,
        })
    }

    async fn name_thread(&self, request: &ThreadRequest<'_>) -> Result<ThreadNaming, SummaryError> {
        // `#541 thread-summary-concise`: the style rides with the rule, outside the policy input.
        let system = format!(
            "{THREAD_RULE}\n{THREAD_STYLE}\nKeep the sentence under {} characters.",
            request.sentence_chars
        );
        let reply = self
            .complete(Priority::Background, &system, request.prompt())
            .await?;
        parse_thread_answer(&reply, request.sentence_chars)
    }
}

#[cfg(test)]
mod tests;
