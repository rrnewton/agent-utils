//! Only the user's own explicit ask sends. `#511 voice-explicit-send-only`.
//!
//! The owner, driving, said a long sentence to the voice agent. The speech runtime cut it into
//! four turns, garbled two of them ("conflicting guidance" became "afflicting guidance"), and the
//! model posted the fragment "afflicting guidance and remind on" to a coding agent's channel, with
//! confirmation off. His rule, verbatim in the issue: the voice agent talks to a coding agent only
//! when he asks it to with an active verb — "send this to the coding agent", "ask the coding
//! agent" — whether or not confirmation is on. It never passes an utterance through.
//!
//! The prompt says so. This module is what holds when the model does not listen: `post_reply`
//! names, in `user_request`, the user's words that asked for the send, and [`SendIntent::check`]
//! looks for those words in what the user actually said in the last [`HEARD_WINDOW`] and for an
//! explicit directive in them. No match, no directive, or a directive already spent on an earlier
//! send, and nothing is proposed or posted: the model is told to answer or ask instead.
//!
//! # What the server hears
//!
//! The browser owns a call's socket ([`crate::voice_bridge`]), so the server hears the user only
//! from the voice page: `POST /api/v1/voice-heard` with each user turn's text as it grows
//! ([`SendIntent::hear`]). A spoken turn is filed in the transcript only when it settles, which on
//! the websocket backend is after the model's tool call, so the live report is what makes an ask
//! count while the turn that made it is still open. Filed turns are deliberately NOT heard: the page
//! files turns it saved before it was closed minutes or hours later, and an ask in one of those must
//! not come back to life. Never a page's echo of its own prompt (`#353 relay-echo-as-owner`), never
//! a chat message, never anything the model says: the user's words only.
//!
//! # What counts as an ask
//!
//! The quoted `user_request` must itself be an ask, and say more than small words; and the turn it
//! is found in must hold an ask too. [`directive`] is deliberately positional, not a list of words
//! to find. A send verb counts only where an imperative or a request stands — at the start of a
//! clause, after "please", "so", "and", "then", "yes", "no", after "can you", "could you", "would
//! you", "will you", or after "want you to" — and with no "don't", "not" or "never" in the few words
//! before it. So "did you send it", "the agent should post that", "I don't want you to send this"
//! and "don't tell it" are not asks. And it counts only when it reaches who or what the send is for:
//!
//! * "tell", "ask", "remind", "ping", "message", "have", "instruct", "inform", "notify" a named
//!   addressee: an agent noun ("the coding agent", "Claude", "the bot") or a channel this server
//!   knows by name — never "it" or "them", where a misheard fragment slips through ("so tell it to
//!   afflicting guidance"); "let" an addressee "know"; "have" not in a question ("have the agents
//!   finished?" asks the voice agent);
//! * "send", "post", "forward", "relay", "pass", "share", "submit" a message ("send this", "post
//!   that", "send it") or to an addressee ("send rebase to the coding agent"); "put" only to an
//!   addressee ("put it in the lead channel", never "put that on hold");
//! * "say", "write", "reply", "respond", "answer" to an addressee.
//!
//! A word that turns the sentence away — "me", "you", "what", "about", "whether", the end of a
//! sentence — stops the search, so "tell me what the agent said" is not an ask. A bare "yes" is
//! not one either: the model is told to ask the user to say "send it".
//!
//! # One ask, one send
//!
//! A direct send spends the ask that licensed it in the same step as the check, so one "tell the
//! coding agent to rebase" licenses one post, and two sends at once cannot share it; a send that
//! certainly posted nothing gives it back. The same words to the same place again are still
//! answered from the post gate's send memory, as a repeat that posts nothing, never refused as a
//! new send. Proposing spends nothing: a proposal posts nothing, and the owner's yes is what sends
//! it. What was heard and spent is kept for [`HEARD_WINDOW`] and no longer.
//!
//! # The log
//!
//! Content-free like every access line ([`crate::access::send_intent`]): the event, the reason,
//! how long the quoted request was, and how many utterances were heard. Never a word of either.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How far back an ask is heard. The owner's ask and the model's call are seconds apart; three
/// minutes covers a model that asked a follow-up question first.
pub const HEARD_WINDOW: Duration = Duration::from_secs(180);
/// How many user turns are kept, oldest dropped first.
pub const HEARD_MAX: usize = 32;
/// The most characters of one user turn kept. A longer one keeps its start and its end, where an
/// ask at the close of a long turn is.
pub const HEARD_TEXT_MAX: usize = 2_000;
/// How much of the start of a longer turn is kept; the rest of [`HEARD_TEXT_MAX`] is its end.
const HEARD_HEAD: usize = 500;
/// The longest key a page may name a turn by.
pub const HEARD_KEY_MAX: usize = 160;
/// How long a check waits for the page's report of the turn the model is answering, which can be
/// a beat behind the model's call.
pub const REPORT_GRACE: Duration = Duration::from_millis(2_500);
/// The largest `POST /api/v1/voice-heard` body accepted: a turn's text at [`HEARD_TEXT_MAX`]
/// characters of up to four bytes each, its key, and the JSON around them.
pub const MAX_BODY_BYTES: usize = 4 * HEARD_TEXT_MAX + 4 * HEARD_KEY_MAX + 256;
/// How many asks a direct send remembers having spent.
const SPENT_MAX: usize = 64;
/// Two turns closer than this are read together as well as apart, so "tell the coding agent" cut
/// from "to rebase" by a pause is still one ask.
const ADJACENT: Duration = Duration::from_secs(15);

/// Why a send was refused. Each has a [`Refusal::code`] for the log and a
/// [`Refusal::sentence`] for the model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// `user_request` was missing or empty.
    Missing,
    /// The user said those words, but they ask for no send.
    NoDirective,
    /// Nothing the user said in the last three minutes matches `user_request`.
    NotHeard,
    /// The ask was already spent on an earlier send.
    Spent,
}

impl Refusal {
    /// A short code for the access log and the REST error.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Missing => "missing_request",
            Self::NoDirective => "no_directive",
            Self::NotHeard => "not_heard",
            Self::Spent => "spent",
        }
    }

    /// What the model is told: nothing was sent, why, and what to do instead.
    #[must_use]
    pub fn sentence(self) -> &'static str {
        match self {
            Self::Missing => {
                "NOT SENT. Nothing was posted or proposed: post_reply needs user_request, the \
                 user's own words asking you to send this. Send only when the user explicitly \
                 asks you to, in words such as \"send this to the coding agent\" or \"tell the \
                 coding agent to ...\"."
            }
            Self::NoDirective => {
                "NOT SENT. Nothing was posted or proposed: the user's words you quoted do not ask \
                 you to send anything. Send to the chat or the coding agent only when the user \
                 explicitly asks you to, with words such as \"send this to the coding agent\", \
                 \"tell the coding agent to ...\" or \"ask Claude ...\". Never pass on what the \
                 user said as a message. Answer the user yourself, or, if you think they want it \
                 sent, ask them to say \"send it\"."
            }
            Self::NotHeard => {
                "NOT SENT. Nothing was posted or proposed: the user did not say the words you \
                 quoted in the last few minutes, as far as the voice page heard them. Speech can \
                 be cut short or misheard. Do not send it; tell the user you did not catch a \
                 request to send, and ask them to say it again if they want it sent."
            }
            Self::Spent => {
                "NOT SENT. Nothing was posted or proposed: the ask you quoted was already used for \
                 a message you sent, and each message needs its own ask. Do not send it unless the \
                 user explicitly asks again."
            }
        }
    }
}

/// The utterances that licensed a send: what [`SendIntent::spend`] marks used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct License {
    keys: Vec<String>,
}

/// One user turn as the page last reported it.
#[derive(Clone, Debug)]
struct Heard {
    key: String,
    text: String,
    at: Instant,
}

/// An ask a direct send has used, by the key of the turn it was heard in.
#[derive(Clone, Debug)]
struct Spent {
    key: String,
    at: Instant,
}

#[derive(Debug, Default)]
struct Inner {
    /// Oldest first.
    heard: VecDeque<Heard>,
    /// Oldest first.
    spent: VecDeque<Spent>,
}

impl Inner {
    /// Drop what is older than [`HEARD_WINDOW`]: it can license nothing, and is kept no longer
    /// than it is useful.
    fn prune(&mut self, now: Instant) {
        self.heard
            .retain(|h| now.saturating_duration_since(h.at) <= HEARD_WINDOW);
        self.spent
            .retain(|s| now.saturating_duration_since(s.at) <= HEARD_WINDOW);
    }
}

/// What the user said lately, and which of their asks were spent. In memory: an ask does not
/// outlive the call it was made in, and a restart ends the call.
#[derive(Debug, Default)]
pub struct SendIntent {
    inner: Mutex<Inner>,
    heard_changed: tokio::sync::Notify,
}

/// `text` within [`HEARD_TEXT_MAX`] characters: whole when it fits, otherwise its first sentences
/// and its end, where an ask at the close of a long turn is.
fn bounded(text: &str) -> String {
    let count = text.chars().count();
    if count <= HEARD_TEXT_MAX {
        return text.to_owned();
    }
    let head: String = text.chars().take(HEARD_HEAD).collect();
    let tail: String = text
        .chars()
        .skip(count - (HEARD_TEXT_MAX - HEARD_HEAD - 3))
        .collect();
    format!("{head} … {tail}")
}

impl SendIntent {
    /// An empty record.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The page heard the user say `text` in the turn it calls `key`: the turn's whole text so
    /// far, which replaces what the same key said before. Empty text is ignored.
    pub fn hear(&self, key: &str, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let key: String = key.chars().take(HEARD_KEY_MAX).collect();
        let text = bounded(text);
        let now = Instant::now();
        let mut inner = self.lock();
        inner.prune(now);
        inner.heard.retain(|h| h.key != key);
        while inner.heard.len() >= HEARD_MAX {
            inner.heard.pop_front();
        }
        inner.heard.push_back(Heard { key, text, at: now });
        drop(inner);
        self.heard_changed.notify_waiters();
    }

    /// Whether `request` names an explicit ask the user made lately and has not spent; with
    /// `spend`, the ask is spent in the same step, so two sends at once cannot both use it
    /// ([`Self::release`] gives it back when nothing was posted).
    ///
    /// `targets` are the names of the channels this server knows, which count as addressees.
    /// When the ask is not heard yet, waits up to [`REPORT_GRACE`] for the page's report of the
    /// turn the model is answering before refusing.
    ///
    /// # Errors
    ///
    /// The [`Refusal`] that says why not.
    pub async fn check(
        &self,
        request: Option<&str>,
        targets: &[String],
        spend: bool,
    ) -> Result<License, Refusal> {
        let request = request.map(str::trim).unwrap_or_default();
        if request.is_empty() {
            return Err(Refusal::Missing);
        }
        let targets: Vec<Vec<String>> = targets
            .iter()
            .map(|t| words(&tokens(t)))
            .filter(|t| !t.is_empty())
            .collect();
        let wanted = tokens(request);
        // The quote must itself be the ask, and must say something beyond small words: a quote
        // of "the" matches every turn.
        if !directive(&wanted, &targets) || content(&wanted).is_empty() {
            return Err(Refusal::NoDirective);
        }
        let deadline = tokio::time::Instant::now() + REPORT_GRACE;
        loop {
            // Registered before looking, so a report that lands between the look and the wait
            // still wakes it.
            let changed = self.heard_changed.notified();
            let verdict = self.judge(&wanted, &targets, Instant::now(), spend);
            match verdict {
                Err(Refusal::NotHeard | Refusal::NoDirective)
                    if tokio::time::Instant::now() < deadline =>
                {
                    if tokio::time::timeout_at(deadline, changed).await.is_err() {
                        return self.judge(&wanted, &targets, Instant::now(), spend);
                    }
                }
                other => return other,
            }
        }
    }

    /// [`Self::check`]'s verdict as of `now`, without waiting.
    fn judge(
        &self,
        wanted: &[Token],
        targets: &[Vec<String>],
        now: Instant,
        spend: bool,
    ) -> Result<License, Refusal> {
        let mut inner = self.lock();
        inner.prune(now);
        let recent: Vec<&Heard> = inner.heard.iter().collect();
        // Each turn alone, newest first, then each adjacent pair read together.
        let mut candidates: Vec<Vec<&Heard>> = recent.iter().rev().map(|h| vec![*h]).collect();
        for pair in recent.windows(2).rev() {
            if pair[1].at.saturating_duration_since(pair[0].at) <= ADJACENT {
                candidates.push(vec![pair[0], pair[1]]);
            }
        }
        let mut refusal = Refusal::NotHeard;
        let mut found = None;
        for candidate in candidates {
            let text = candidate
                .iter()
                .map(|h| h.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            let said = tokens(&text);
            if !quotes(wanted, &said) {
                continue;
            }
            if !directive(&said, targets) {
                if refusal == Refusal::NotHeard {
                    refusal = Refusal::NoDirective;
                }
                continue;
            }
            if candidate
                .iter()
                .any(|h| inner.spent.iter().any(|s| s.key == h.key))
            {
                refusal = Refusal::Spent;
                continue;
            }
            found = Some(License {
                keys: candidate.iter().map(|h| h.key.clone()).collect(),
            });
            break;
        }
        let Some(license) = found else {
            return Err(refusal);
        };
        if spend {
            for key in &license.keys {
                while inner.spent.len() >= SPENT_MAX {
                    inner.spent.pop_front();
                }
                inner.spent.push_back(Spent {
                    key: key.clone(),
                    at: now,
                });
            }
        }
        Ok(license)
    }

    /// Whether `request` names the user's own explicit ask, made lately, to mark messages read:
    /// "mark that read", "mark those as done", "archive it", "dismiss them", "clear those". The
    /// quote must itself be such an ask and must be among what the user said in the last
    /// [`HEARD_WINDOW`]. Nothing is spent: marking twice changes nothing the second time. `#649
    /// voice-agent-tools` item 7. Waits up to [`REPORT_GRACE`] for the page's report, as
    /// [`Self::check`] does.
    ///
    /// # Errors
    ///
    /// The [`Refusal`] that says why not ([`Refusal::Spent`] is never one).
    pub async fn check_read(&self, request: Option<&str>) -> Result<(), Refusal> {
        let request = request.map(str::trim).unwrap_or_default();
        if request.is_empty() {
            return Err(Refusal::Missing);
        }
        let wanted = tokens(request);
        if !read_directive(&wanted) || content(&wanted).is_empty() {
            return Err(Refusal::NoDirective);
        }
        let deadline = tokio::time::Instant::now() + REPORT_GRACE;
        loop {
            let changed = self.heard_changed.notified();
            let verdict = self.judge_read(&wanted, Instant::now());
            match verdict {
                Err(_) if tokio::time::Instant::now() < deadline => {
                    if tokio::time::timeout_at(deadline, changed).await.is_err() {
                        return self.judge_read(&wanted, Instant::now());
                    }
                }
                other => return other,
            }
        }
    }

    /// [`Self::check_read`]'s verdict as of `now`, without waiting.
    fn judge_read(&self, wanted: &[Token], now: Instant) -> Result<(), Refusal> {
        let mut inner = self.lock();
        inner.prune(now);
        let recent: Vec<&Heard> = inner.heard.iter().collect();
        let mut candidates: Vec<Vec<&Heard>> = recent.iter().rev().map(|h| vec![*h]).collect();
        for pair in recent.windows(2).rev() {
            if pair[1].at.saturating_duration_since(pair[0].at) <= ADJACENT {
                candidates.push(vec![pair[0], pair[1]]);
            }
        }
        let mut refusal = Refusal::NotHeard;
        for candidate in candidates {
            let text = candidate
                .iter()
                .map(|h| h.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            let said = tokens(&text);
            if !quotes(wanted, &said) {
                continue;
            }
            if read_directive(&said) {
                return Ok(());
            }
            refusal = Refusal::NoDirective;
        }
        Err(refusal)
    }

    /// The send `license` was spent on certainly posted nothing: its ask is good again.
    pub fn release(&self, license: &License) {
        self.lock().spent.retain(|s| !license.keys.contains(&s.key));
    }

    /// Forget everything heard and spent: the owner erased the record it is a copy of.
    pub fn forget(&self) {
        let mut inner = self.lock();
        inner.heard.clear();
        inner.spent.clear();
    }

    /// How many user turns are held, for the log.
    #[must_use]
    pub fn heard_count(&self) -> usize {
        let mut inner = self.lock();
        inner.prune(Instant::now());
        inner.heard.len()
    }
}

/// A token of speech: a word, or the end of a sentence, which says whether it was a question.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    Word(String),
    Stop { question: bool },
}

/// `text` as lowercase words and sentence ends. Apostrophes are dropped ("don't" is "dont"),
/// every other mark splits words, and `.`, `!`, `?`, `;` and `:` end a sentence.
fn tokens(text: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut Vec<Token>| {
        if !word.is_empty() {
            out.push(Token::Word(std::mem::take(word)));
        }
    };
    for c in text.chars() {
        if c.is_alphanumeric() {
            word.extend(c.to_lowercase());
        } else if c == '\'' || c == '\u{2019}' {
            // Inside a word: "don't" is one word.
        } else {
            flush(&mut word, &mut out);
            if matches!(c, '.' | '!' | '?' | ';' | ':') {
                let question = c == '?';
                match out.last_mut() {
                    Some(Token::Stop { question: q }) => *q |= question,
                    _ => out.push(Token::Stop { question }),
                }
            }
        }
    }
    flush(&mut word, &mut out);
    out
}

fn words(tokens: &[Token]) -> Vec<String> {
    tokens
        .iter()
        .filter_map(|t| match t {
            Token::Word(w) => Some(w.clone()),
            Token::Stop { .. } => None,
        })
        .collect()
}

/// Words that carry no content, left out when a quoted request is matched against speech.
const FILLER: &[&str] = &[
    "a", "an", "and", "the", "to", "of", "in", "on", "it", "is", "that", "this", "for", "please",
    "so", "can", "could", "you", "me", "i", "my", "our", "with", "at", "be", "just", "also",
    "then", "now", "um", "uh", "okay", "ok",
];

/// The words of `tokens` that carry content: everything but [`FILLER`].
fn content(tokens: &[Token]) -> Vec<String> {
    words(tokens)
        .into_iter()
        .filter(|w| !FILLER.contains(&w.as_str()))
        .collect()
}

/// Whether `said` holds what `wanted` quotes: at least half of the quote's content words, each
/// as said or within a slip of the ear of it, and at least one. A quote of small words alone
/// quotes nothing.
fn quotes(wanted: &[Token], said: &[Token]) -> bool {
    let content = content(wanted);
    let said = words(said);
    if content.is_empty() {
        return false;
    }
    let found = content
        .iter()
        .filter(|w| said.iter().any(|s| close(w, s)))
        .count();
    found >= 1 && found * 2 >= content.len()
}

/// The same word, or one a transcriber could have heard for it: one edit apart in a word of four
/// letters or more, two in one of eight or more.
fn close(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let (la, lb) = (a.chars().count(), b.chars().count());
    let shorter = la.min(lb);
    let allowed = if shorter >= 8 {
        2
    } else if shorter >= 4 {
        1
    } else {
        0
    };
    allowed > 0 && la.abs_diff(lb) <= allowed && edits(a, b) <= allowed
}

/// Levenshtein distance.
fn edits(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let substituted = previous + usize::from(ca != *cb);
            previous = row[j + 1];
            row[j + 1] = substituted.min(row[j] + 1).min(row[j + 1] + 1);
        }
    }
    row[b.len()]
}

/// Verbs whose ask names an addressee: "tell the coding agent", "ask Claude".
const ADDRESS_VERBS: &[&str] = &[
    "tell", "ask", "remind", "ping", "message", "have", "instruct", "inform", "notify",
];
/// Verbs whose ask names a message or a destination: "send this", "post to the channel".
const SEND_VERBS: &[&str] = &[
    "send", "post", "forward", "relay", "pass", "share", "submit",
];
/// Verbs whose ask names a destination only: "say to the agent", "reply to Claude".
const TO_VERBS: &[&str] = &["say", "write", "reply", "respond", "answer"];

/// Who a send can be for, besides the channels this server knows by name. Not "assistant": that
/// is the voice agent itself.
const ADDRESSEES: &[&str] = &[
    "agent",
    "agents",
    "claude",
    "codex",
    "muse",
    "gemini",
    "gpt",
    "bot",
    "bots",
    "coder",
    "team",
    "channel",
    "thread",
    "chat",
    "coordinator",
    "worker",
    "lead",
];
/// What "send", "post" and the like can take as the message itself.
const MESSAGES: &[&str] = &[
    "message",
    "messages",
    "note",
    "reply",
    "response",
    "answer",
    "text",
    "this",
    "that",
    "it",
    "these",
    "those",
    "them",
    "question",
    "request",
    "update",
    "instructions",
    "comment",
    "word",
];
/// What "reply", "respond" and "answer" can be to, besides an addressee.
const REPLY_TO: &[&str] = &["message", "thread", "post", "it", "that", "this"];
/// Words that may stand between a verb and what it reaches: "send THE message", "tell THE CODING
/// agent".
const BETWEEN: &[&str] = &[
    "a", "an", "the", "my", "our", "your", "that", "this", "to", "coding", "code", "other", "same",
    "main", "please", "again", "also", "back", "over", "along", "on", "in", "into", "through",
];
/// Words that turn a sentence away from an ask before it reaches an addressee.
const BLOCKERS: &[&str] = &[
    "me",
    "us",
    "you",
    "myself",
    "yourself",
    "what",
    "how",
    "why",
    "when",
    "where",
    "who",
    "which",
    "whether",
    "if",
    "about",
    "something",
    "anything",
    "nothing",
];
/// Words after which a verb stands as an imperative: the start of a request.
const LEADS: &[&str] = &[
    "please", "so", "and", "then", "just", "now", "okay", "ok", "also", "yes", "yeah", "yep", "no",
    "right", "alright", "sure", "actually", "first", "next", "oh", "hey", "um", "uh", "well",
    "but", "or", "instead", "maybe", "quickly", "lets", "wanna", "gonna", "kindly", "again",
];
/// Words before "you" that make "you <verb>" a request: "can you tell", "will you send".
const ASKING: &[&str] = &[
    "can", "could", "would", "will", "please", "and", "so", "then",
];
/// Words before "to" that make "to <verb>" a request: "want you to tell", "need to send".
const WANTING: &[&str] = &["you", "want", "need", "like"];
/// Words that, a few words before a verb in the same sentence, turn it into its opposite: "I don't
/// want you to send this", "never tell it".
const NEGATIONS: &[&str] = &[
    "dont", "not", "never", "doesnt", "didnt", "wont", "cant", "cannot", "shouldnt", "mustnt",
    "without",
];
/// How many words back a negation reaches.
const NEGATION_REACH: usize = 4;

/// Whether `said` holds an explicit ask to send: a send verb in imperative or request position
/// that reaches an addressee or a message. See the module documentation for the rules; `targets`
/// are channel names, as word lists, that count as addressees.
fn directive(said: &[Token], targets: &[Vec<String>]) -> bool {
    (0..said.len()).any(|at| directive_at(said, at, targets))
}

/// Whether `said` asks for messages to be marked read here, in imperative or request position and
/// not negated: "mark" followed within a few words by "read", "done" or "seen"; or "archive",
/// "dismiss" or "clear". "Did you mark it read?" and "don't mark it read" are not asks. `#649
/// voice-agent-tools` item 7.
fn read_directive(said: &[Token]) -> bool {
    (0..said.len()).any(|at| {
        let verb = match word(said, at) {
            Some(verb) => verb,
            None => return false,
        };
        let asks = imperative(said, at) && !in_question(said, at);
        match verb {
            "mark" => {
                asks && said[at + 1..]
                    .iter()
                    .take(6)
                    .map_while(|t| match t {
                        Token::Word(w) => Some(w.as_str()),
                        Token::Stop { .. } => None,
                    })
                    .any(|w| matches!(w, "read" | "done" | "seen"))
            }
            "archive" | "dismiss" | "clear" => asks,
            _ => false,
        }
    })
}

fn word(said: &[Token], at: usize) -> Option<&str> {
    match said.get(at) {
        Some(Token::Word(w)) => Some(w.as_str()),
        _ => None,
    }
}

/// The word before `at`, or `None` at the start of a sentence.
fn before(said: &[Token], at: usize) -> Option<&str> {
    at.checked_sub(1).and_then(|i| word(said, i))
}

/// Whether the sentence holding `at` ends in a question mark.
fn in_question(said: &[Token], at: usize) -> bool {
    said[at..]
        .iter()
        .find_map(|t| match t {
            Token::Stop { question } => Some(*question),
            Token::Word(_) => None,
        })
        .unwrap_or(false)
}

/// Whether a negation stands a few words before `at` in its sentence.
fn negated(said: &[Token], at: usize) -> bool {
    said[at.saturating_sub(NEGATION_REACH)..at]
        .iter()
        .rev()
        .map_while(|t| match t {
            Token::Word(w) => Some(w.as_str()),
            Token::Stop { .. } => None,
        })
        .any(|w| NEGATIONS.contains(&w))
}

/// Whether the verb at `at` stands where an imperative or a request does.
fn imperative(said: &[Token], at: usize) -> bool {
    if negated(said, at) {
        return false;
    }
    match before(said, at) {
        None => true,
        Some(p) if LEADS.contains(&p) => true,
        Some("you") => at
            .checked_sub(1)
            .is_some_and(|i| before(said, i).is_none_or(|p2| ASKING.contains(&p2))),
        Some("to") => at
            .checked_sub(1)
            .and_then(|i| before(said, i))
            .is_some_and(|p2| WANTING.contains(&p2)),
        Some(_) => false,
    }
}

/// The length of the addressee starting at `at`, if one does: a channel name, or an agent noun.
fn addressee(said: &[Token], at: usize, targets: &[Vec<String>]) -> Option<usize> {
    for target in targets {
        if target
            .iter()
            .enumerate()
            .all(|(i, t)| word(said, at + i) == Some(t.as_str()))
        {
            return Some(target.len());
        }
    }
    word(said, at).filter(|w| ADDRESSEES.contains(w)).map(|_| 1)
}

/// Whether, within a few words after the verb at `at`, the clause reaches something `found`
/// accepts before a blocker. `free` is how many words that are neither [`BETWEEN`] nor accepted
/// may be passed over (a misheard adjective: "tell the coating agent").
fn reaches(said: &[Token], at: usize, free: usize, found: impl Fn(usize) -> bool) -> bool {
    let mut passed = 0;
    for i in at + 1..(at + 10).min(said.len()) {
        let Some(w) = word(said, i) else {
            return false;
        };
        if found(i) {
            return true;
        }
        if BLOCKERS.contains(&w) {
            return false;
        }
        if !BETWEEN.contains(&w) {
            passed += 1;
            if passed > free {
                return false;
            }
        }
    }
    false
}

fn directive_at(said: &[Token], at: usize, targets: &[Vec<String>]) -> bool {
    let Some(verb) = word(said, at) else {
        return false;
    };
    if !imperative(said, at) {
        return false;
    }
    // "Have the agents finished?" asks the voice agent something; "have the agent run the tests"
    // is an ask to send.
    if verb == "have" && in_question(said, at) {
        return false;
    }
    if verb == "put" {
        // "put it in the lead channel"; never "put that on hold".
        return reaches(said, at, 8, |i| addressee(said, i, targets).is_some());
    }
    let to_addressee = |free| reaches(said, at, free, |i| addressee(said, i, targets).is_some());
    if verb == "let" {
        // "let the coding agent know": an addressee, then "know".
        return (at + 1..(at + 6).min(said.len())).any(|i| {
            addressee(said, i, targets).is_some_and(|len| {
                (i + len..(i + len + 3).min(said.len())).any(|k| word(said, k) == Some("know"))
            })
        });
    }
    if ADDRESS_VERBS.contains(&verb) {
        // Named, never "it" or "them": a pronoun is where a misheard fragment slips through
        // ("so tell it to afflicting guidance"), and the owner's own examples name who.
        return to_addressee(3);
    }
    if SEND_VERBS.contains(&verb) {
        // The message straight after the verb, past at most two small words, or a destination
        // anywhere in the clause.
        let mut i = at + 1;
        let mut skipped = 0;
        while let Some(w) = word(said, i) {
            if MESSAGES.contains(&w) {
                return true;
            }
            if skipped >= 2 || !matches!(w, "a" | "an" | "the" | "my" | "our" | "your") {
                break;
            }
            skipped += 1;
            i += 1;
        }
        return to_addressee(8);
    }
    if TO_VERBS.contains(&verb) {
        if matches!(verb, "reply" | "respond" | "answer") {
            return reaches(said, at, 3, |i| {
                addressee(said, i, targets).is_some()
                    || word(said, i).is_some_and(|w| REPLY_TO.contains(&w))
            });
        }
        // "say to the agent", "say hi to Claude": the addressee comes after a "to".
        let mut to = false;
        for i in at + 1..(at + 10).min(said.len()) {
            let Some(w) = word(said, i) else {
                return false;
            };
            if to && addressee(said, i, targets).is_some() {
                return true;
            }
            if BLOCKERS.contains(&w) {
                return false;
            }
            to |= w == "to";
        }
        return false;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asks(text: &str) -> bool {
        directive(&tokens(text), &[vec!["ops".to_owned(), "room".to_owned()]])
    }

    /// The incident, word for word as the speech runtime transcribed it on 2026-10-25.
    #[test]
    fn the_fragments_of_the_incident_ask_for_nothing() {
        for said in [
            "Find the agent that flagged a conflict between Claude dot MD and the newer agents \
             dot MD that referenced a soft green landing protocol.",
            "I do want the sock all.",
            "And also remind on.",
            "afflicting guidance and remind on",
            "Hold on. Let me let me interject some responses here and then you can continue with \
             your summary.",
            "Hold on a second.",
            "Read the last batch of messages on each of the channels.",
            "Ignore the previous message. That was",
        ] {
            assert!(!asks(said), "{said:?} is not an ask");
        }
    }

    #[test]
    fn real_asks_are_asks() {
        for said in [
            "Tell the coding agent to rebase onto main.",
            "Send this to the coding agent.",
            "Ask Claude whether the tests pass.",
            "Can you ask the agent what is left?",
            "Please tell Claude to stop.",
            "So tell the agent to resolve the conflicting guidance.",
            "Send that to ops room.",
            "Relay this to the lead team channel.",
            "Message the coding agent and say stop.",
            "Let the coding agent know the build is green.",
            "Reply to Claude: looks good.",
            "I want you to tell the coding agent to stop.",
            "Go ahead and send it.",
            "Yes, send it.",
            "send it",
            "Could you post in the channel that the build is green?",
            "Ping the agent about the flaky test.",
            "Have the coding agent run the tests again.",
            "Tell the coating agent to rebase.",
            "Forward the message to Codex.",
            "Say hi to the agent.",
            "OK, remind the agent to land it.",
        ] {
            assert!(asks(said), "{said:?} is an ask");
        }
    }

    #[test]
    fn sends_described_negated_or_asked_about_are_not_asks() {
        for said in [
            "What did the coding agent say?",
            "Tell me what the agent is doing.",
            "Did you send it?",
            "Don't send that.",
            "Do not tell the agent.",
            "The agent should post the results.",
            "Should I tell the agent?",
            "Find the agent that will send updates to the channel.",
            "Ask yourself whether that is right.",
            "Tell me about the coding agent.",
            "Let it run.",
            "Let me think.",
            "Yes.",
            "Send.",
            "Can you summarize what the agent posted?",
            "Don't tell. The agent said it was fine.",
            "I told the agent yesterday.",
        ] {
            assert!(!asks(said), "{said:?} is not an ask");
        }
    }

    #[test]
    fn a_quote_matches_speech_through_a_slip_of_the_ear() {
        let said = tokens("Tell the coating agent to rebase onto main.");
        assert!(quotes(
            &tokens("tell the coding agent to rebase onto main"),
            &said
        ));
        assert!(quotes(&tokens("rebase onto main"), &said));
        assert!(!quotes(&tokens("delete the branch"), &said));
        assert!(!quotes(&tokens(""), &said));
    }

    fn targets() -> Vec<String> {
        vec!["ops-room".to_owned()]
    }

    #[test]
    fn more_things_that_are_not_asks() {
        // The adversarial review's cases.
        for said in [
            "Have the agents finished?",
            "I don't want you to send this to the coding agent yet.",
            "I don't need to tell the agent.",
            "Put that on hold.",
            "Put it on my calendar.",
            "Never tell it that.",
            "Please tell it to stop.",
            "So tell it to afflicting guidance.",
            "OK, remind them to land it.",
            "Let it know.",
        ] {
            assert!(!asks(said), "{said:?} is not an ask");
        }
        for said in [
            "Put it in the lead channel.",
            "Have the agent run the tests.",
            "No, tell the agent to stop.",
        ] {
            assert!(
                directive(&tokens(said), &[vec!["lead".to_owned()]]),
                "{said:?} is an ask"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn the_incident_is_refused() {
        let intent = SendIntent::new();
        intent.hear(
            "c:9:you",
            "Find the agent that flagged a conflict between Claude dot MD and the newer agents \
             dot MD that referenced a soft green landing protocol.",
        );
        // What the model posted, quoted as the ask: not an ask at all.
        assert_eq!(
            intent
                .check(Some("afflicting guidance and remind on"), &targets(), true)
                .await,
            Err(Refusal::NoDirective)
        );
        // Quoting the question asked of the voice agent is no better.
        assert_eq!(
            intent
                .check(
                    Some("Find the agent that flagged a conflict"),
                    &targets(),
                    true
                )
                .await,
            Err(Refusal::NoDirective)
        );
        // An ask the user never made, as far as the page heard.
        assert_eq!(
            intent
                .check(
                    Some("tell the coding agent to resolve the conflicting guidance"),
                    &targets(),
                    true
                )
                .await,
            Err(Refusal::NotHeard)
        );
        assert_eq!(
            intent.check(None, &targets(), true).await,
            Err(Refusal::Missing)
        );
        assert_eq!(
            intent.check(Some("  "), &targets(), true).await,
            Err(Refusal::Missing)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_quote_of_small_words_licenses_nothing() {
        let intent = SendIntent::new();
        intent.hear("c:3:you", "Ask Claude whether the tests pass.");
        assert_eq!(
            intent.check(Some("the"), &targets(), true).await,
            Err(Refusal::NoDirective)
        );
        // A quote that is no ask, even of words that are in the ask's turn.
        assert_eq!(
            intent.check(Some("the tests pass"), &targets(), true).await,
            Err(Refusal::NoDirective)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_ask_licenses_one_direct_send() {
        let intent = SendIntent::new();
        intent.hear("c:3:you", "Tell the coding agent to rebase onto main.");
        // Checking without spending — proposing — leaves the ask good.
        assert!(intent
            .check(
                Some("tell the coding agent to rebase onto main"),
                &targets(),
                false
            )
            .await
            .is_ok());
        let license = intent
            .check(Some("tell the coding agent to rebase"), &targets(), true)
            .await
            .expect("an ask the user made");
        // Spent in the same step: a second send at once cannot use it.
        assert_eq!(
            intent
                .check(Some("tell the coding agent to rebase"), &targets(), true)
                .await,
            Err(Refusal::Spent)
        );
        // A send that certainly posted nothing gives it back.
        intent.release(&license);
        assert!(intent
            .check(Some("tell the coding agent to rebase"), &targets(), true)
            .await
            .is_ok());
        // The same words said again later are a new ask.
        intent.hear("c:7:you", "Tell the coding agent to rebase onto main.");
        assert!(intent
            .check(Some("tell the coding agent to rebase"), &targets(), true)
            .await
            .is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn an_ask_split_across_two_turns_is_one_ask() {
        let intent = SendIntent::new();
        intent.hear("c:3:you", "Send a message");
        intent.hear("c:4:you", "to ops room saying the build is green.");
        assert!(intent
            .check(
                Some("send a message to ops-room saying the build is green"),
                &targets(),
                true
            )
            .await
            .is_ok());
    }

    #[test]
    fn an_ask_older_than_the_window_licenses_nothing() {
        let intent = SendIntent::new();
        intent.hear("c:3:you", "Tell the coding agent to rebase.");
        let wanted = tokens("tell the coding agent to rebase");
        let targets = vec![vec!["ops".to_owned(), "room".to_owned()]];
        assert!(intent
            .judge(&wanted, &targets, Instant::now(), false)
            .is_ok());
        // `Instant` is the standard library's, which a paused tokio clock does not move: judge as
        // of a time past the window instead.
        let later = Instant::now() + HEARD_WINDOW + Duration::from_secs(1);
        assert_eq!(
            intent.judge(&wanted, &targets, later, false),
            Err(Refusal::NotHeard)
        );
        assert_eq!(intent.lock().heard.len(), 0, "kept past the window");
    }

    #[tokio::test(start_paused = true)]
    async fn a_report_that_lands_during_the_grace_counts() {
        let intent = std::sync::Arc::new(SendIntent::new());
        let late = std::sync::Arc::clone(&intent);
        let report = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(400)).await;
            late.hear("c:3:you", "Ask Claude whether the tests pass.");
        });
        assert!(intent
            .check(Some("ask Claude whether the tests pass"), &targets(), true)
            .await
            .is_ok());
        report.await.expect("the report task finishes");
    }

    #[test]
    fn heard_text_is_bounded_and_keeps_the_end_of_a_long_turn() {
        let intent = SendIntent::new();
        for n in 0..HEARD_MAX + 5 {
            intent.hear(&format!("k{n}"), "words");
        }
        assert_eq!(intent.heard_count(), HEARD_MAX);
        let long = format!(
            "{} so send all that to the coding agent",
            "blah ".repeat(1_000)
        );
        intent.hear(&"k".repeat(HEARD_KEY_MAX * 2), &long);
        let inner = intent.lock();
        let last = inner.heard.back().expect("one heard");
        assert_eq!(last.key.chars().count(), HEARD_KEY_MAX);
        assert_eq!(last.text.chars().count(), HEARD_TEXT_MAX);
        assert!(last.text.ends_with("send all that to the coding agent"));
    }
}
