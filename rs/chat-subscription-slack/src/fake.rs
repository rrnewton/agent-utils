//! Deterministic in-memory [`SlackTransport`] for tests.
//!
//! A [`FakeTransport`] is a cheap clonable handle on shared state, so a test can give one clone
//! to the code under test and inspect another. Streams replay scripted tail lines through the
//! real [`crate::cli::parse_tail_line`] and [`crate::cli::classify_tail_stderr`] parsers, and
//! prove positions with the real [`PositionProof`], in script order: a script models the order
//! in which the plugin reads the client's two pipes, which need not be the order the client
//! wrote them in. Posts and reactions are recorded with the app
//! identity they used; injected failures model Slack errors and uncertain outcomes.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use crate::cli::{classify_tail_stderr, parse_tail_line, PositionProof};
use crate::ids::AppId;
use crate::transport::{
    EventStream, ListQuery, ListedMessage, PostMessage, SlackTransport, StreamCancel, StreamRecord,
    StreamWait, SubscribeSpec, TextFormat, TransportError,
};

/// One scripted stream step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FakeStep {
    /// A raw tail stdout line.
    Line(String),
    /// The stream reports nothing for one wait.
    Idle,
    /// The stream ends.
    Close(String),
    /// A line the client wrote to its standard error.
    Stderr(String),
    /// The stream fails to open: [`SlackTransport::open_stream`] returns this error.
    OpenFails(TransportError),
}

/// One recorded post.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FakePost {
    /// App the post acted as.
    pub app_id: String,
    /// Conversation ID.
    pub channel_id: String,
    /// Thread root, if any.
    pub thread_ts: Option<String>,
    /// Body.
    pub text: String,
    /// Body format.
    pub format: TextFormat,
    /// Assigned message ts.
    pub ts: String,
}

/// One recorded reaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FakeReaction {
    /// App the reaction acted as.
    pub app_id: String,
    /// Conversation ID.
    pub channel_id: String,
    /// Message ts.
    pub message_ts: String,
    /// Reaction name.
    pub name: String,
}

/// An injected outcome for the next post.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FakePostFailure {
    /// Fail without posting.
    Refuse(TransportError),
    /// Post, then report an unknown outcome (a lost response).
    PostedButLost,
    /// Do not post, and report an unknown outcome (a lost request).
    LostBeforePosting,
}

/// Everything the fake has observed.
#[derive(Clone, Debug, Default)]
pub struct FakeState {
    /// Scripts consumed one per [`SlackTransport::open_stream`].
    pub scripts: VecDeque<Vec<FakeStep>>,
    /// Every stream opened, in order.
    pub opened: Vec<SubscribeSpec>,
    /// Every successful post, including ones whose response was lost.
    pub posts: Vec<FakePost>,
    /// Every reaction added.
    pub reactions: Vec<FakeReaction>,
    /// Authoritative sender classifications; an absent sender is not confirmed human.
    pub sender_humanity: BTreeMap<String, bool>,
    /// Human-author lookups, as `(app_id, sender_id)`.
    pub humanity_queries: Vec<(String, String)>,
    /// Injected errors for the next human-author lookups.
    pub humanity_failures: VecDeque<TransportError>,
    /// Injected outcomes for the next posts.
    pub post_failures: VecDeque<FakePostFailure>,
    /// Number of listing calls.
    pub list_calls: usize,
    /// Next assigned message ts (seconds part).
    pub next_ts_seconds: u64,
}

/// Shared handle on a [`FakeState`].
#[derive(Clone, Debug)]
pub struct FakeTransport {
    state: Arc<Mutex<FakeState>>,
}

impl Default for FakeTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeTransport {
    /// An empty fake whose first post gets ts `1800000000.000100`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(FakeState {
                next_ts_seconds: 1_800_000_000,
                ..FakeState::default()
            })),
        }
    }

    /// Lock the shared state.
    pub fn state(&self) -> MutexGuard<'_, FakeState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Queue a script for the next stream.
    pub fn push_script(&self, steps: Vec<FakeStep>) {
        self.state().scripts.push_back(steps);
    }

    /// Queue an outcome for the next post.
    pub fn push_post_failure(&self, failure: FakePostFailure) {
        self.state().post_failures.push_back(failure);
    }
}

struct FakeCancel {
    flag: Arc<(Mutex<bool>, Condvar)>,
}

impl StreamCancel for FakeCancel {
    fn cancel(&self) {
        let (cancelled, changed) = &*self.flag;
        *cancelled
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        changed.notify_all();
    }
}

/// A scripted stream. When the script is exhausted it blocks until cancelled or the wait ends.
pub struct FakeStream {
    steps: VecDeque<FakeStep>,
    flag: Arc<(Mutex<bool>, Condvar)>,
    closed: Arc<AtomicBool>,
    proof: PositionProof,
    proven: Option<u64>,
}

impl EventStream for FakeStream {
    fn next(&mut self, wait: Duration) -> StreamWait {
        let (cancelled, changed) = &*self.flag;
        let guard = cancelled
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *guard {
            return StreamWait::Cancelled;
        }
        if let Some(position) = self.proven.take() {
            return StreamWait::Record(StreamRecord::Position(position));
        }
        match self.steps.pop_front() {
            Some(FakeStep::Line(line)) => {
                let record = parse_tail_line(&line);
                self.proven = self.proof.stdout(&record);
                StreamWait::Record(record)
            }
            Some(FakeStep::Idle) => StreamWait::Idle,
            Some(FakeStep::Close(reason)) => StreamWait::Closed(reason),
            Some(FakeStep::Stderr(line)) => {
                self.proven = self.proof.stderr(&line);
                StreamWait::Record(classify_tail_stderr(&line))
            }
            Some(FakeStep::OpenFails(_)) => StreamWait::Idle,
            None => {
                let (guard, _) = changed
                    .wait_timeout_while(guard, wait, |cancelled| !*cancelled)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if *guard {
                    StreamWait::Cancelled
                } else {
                    StreamWait::Idle
                }
            }
        }
    }

    fn canceller(&self) -> Arc<dyn StreamCancel> {
        Arc::new(FakeCancel {
            flag: Arc::clone(&self.flag),
        })
    }

    fn close(&mut self) -> Result<(), String> {
        self.closed.store(true, Ordering::SeqCst);
        Ok(())
    }
}

impl SlackTransport for FakeTransport {
    fn open_stream(
        &mut self,
        spec: &SubscribeSpec,
    ) -> Result<Box<dyn EventStream>, TransportError> {
        let mut state = self.state();
        state.opened.push(spec.clone());
        let steps = state.scripts.pop_front().unwrap_or_default();
        if let Some(FakeStep::OpenFails(error)) = steps.first() {
            return Err(error.clone());
        }
        Ok(Box::new(FakeStream {
            steps: steps.into(),
            flag: Arc::new((Mutex::new(false), Condvar::new())),
            closed: Arc::new(AtomicBool::new(false)),
            proof: PositionProof::new(spec, None),
            proven: None,
        }))
    }

    fn post_message(&mut self, post: &PostMessage<'_>) -> Result<String, TransportError> {
        let mut state = self.state();
        let failure = state.post_failures.pop_front();
        match failure {
            Some(FakePostFailure::Refuse(error)) => return Err(error),
            Some(FakePostFailure::LostBeforePosting) => {
                return Err(TransportError::unknown(
                    "transport_timeout",
                    "fake lost request",
                ));
            }
            _ => {}
        }
        state.next_ts_seconds += 1;
        let ts = format!("{}.000100", state.next_ts_seconds);
        state.posts.push(FakePost {
            app_id: post.app_id.as_str().to_owned(),
            channel_id: post.channel_id.to_owned(),
            thread_ts: post.thread_ts.map(str::to_owned),
            text: post.text.to_owned(),
            format: post.format,
            ts: ts.clone(),
        });
        if failure == Some(FakePostFailure::PostedButLost) {
            return Err(TransportError::unknown(
                "transport_timeout",
                "fake lost response",
            ));
        }
        Ok(ts)
    }

    fn sender_is_human(&mut self, app_id: &AppId, sender_id: &str) -> Result<bool, TransportError> {
        let mut state = self.state();
        state
            .humanity_queries
            .push((app_id.as_str().to_owned(), sender_id.to_owned()));
        if let Some(error) = state.humanity_failures.pop_front() {
            return Err(error);
        }
        Ok(state
            .sender_humanity
            .get(sender_id)
            .copied()
            .unwrap_or(false))
    }

    fn add_reaction(
        &mut self,
        app_id: &AppId,
        channel_id: &str,
        message_ts: &str,
        name: &str,
    ) -> Result<bool, TransportError> {
        let mut state = self.state();
        let reaction = FakeReaction {
            app_id: app_id.as_str().to_owned(),
            channel_id: channel_id.to_owned(),
            message_ts: message_ts.to_owned(),
            name: name.to_owned(),
        };
        if state.reactions.contains(&reaction) {
            return Ok(true);
        }
        state.reactions.push(reaction);
        Ok(false)
    }

    fn list_messages(
        &mut self,
        query: &ListQuery<'_>,
    ) -> Result<Vec<ListedMessage>, TransportError> {
        let mut state = self.state();
        state.list_calls += 1;
        let oldest = crate::ids::ts_micros(query.oldest).unwrap_or(0);
        Ok(state
            .posts
            .iter()
            .filter(|post| {
                post.channel_id == query.channel_id
                    && post.thread_ts.as_deref() == query.thread_ts
                    && crate::ids::ts_micros(&post.ts).unwrap_or(0) > oldest
            })
            .map(|post| ListedMessage {
                ts: post.ts.clone(),
                thread_ts: post.thread_ts.clone(),
                author_user_id: Some("U0FAKEBOT01".to_owned()),
                is_author_bot: true,
                content: post.text.clone(),
            })
            .collect())
    }
}
