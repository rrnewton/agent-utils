//! The document index: a fold over the cached messages. `#628 documents-index-server`.
//!
//! Design: `ai_docs/transient/2026-10-26-documents-view/README.md`, as decided by the
//! owner on 2026-10-28. Every message with a link recognised as a document registers
//! that document candidate (existence only); nothing is fetched eagerly. The index is
//! a pure function of the cached messages, maintained incrementally as messages are
//! cached (live, backfill, edit, delete) and rebuilt or loaded at startup. It caches
//! `document key -> mentions (when/where)`, never document contents; fetched contents
//! live in a separate bounded cache. The list path reads only the fold result.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, RwLock};

use serde::{Deserialize, Serialize};

/// The longest address recognised, in bytes.
pub const MAX_ADDRESS: usize = 2048;
/// The most bytes one fetched document may hold.
pub const MAX_DOCUMENT_BYTES: usize = 1024 * 1024;
/// Default ceiling on cached documents, by count.
pub const DEFAULT_MAX_CACHED: usize = 100;
/// Default ceiling on cached documents, by total content bytes.
pub const DEFAULT_MAX_CACHED_BYTES: usize = 16 * 1024 * 1024;
/// The most documents one fetch-recent call fetches.
pub const MAX_FETCH_RECENT: usize = 20;

/// What kind of document a link names. Extensible: a new source is a new kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocKind {
    /// A GitHub `blob/…/*.md` (or `.markdown`) page.
    GithubMarkdown,
    /// Any other GitHub `blob/…` file.
    GithubFile,
    /// A paste, through the configured command source.
    Paste,
    /// A Google Docs document. Recognised, not fetched yet.
    GoogleDoc,
}

impl DocKind {
    /// The wire name of this kind, also its list icon name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GithubMarkdown => "github_markdown",
            Self::GithubFile => "github_file",
            Self::Paste => "paste",
            Self::GoogleDoc => "google_doc",
        }
    }
}

/// One mention of a document in one message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mention {
    /// Channel the message was in.
    pub channel_id: String,
    /// Message that linked the document.
    pub message_id: String,
    /// Thread the message is a reply in, when it is one.
    pub thread_id: Option<String>,
    /// When the message was posted, in Unix milliseconds, when known.
    pub posted_ms: Option<i64>,
    /// Display name of the author.
    pub author: String,
    /// The link's label, when the writer gave one.
    pub label: Option<String>,
}

/// The parsed locator behind a document key: what a fetcher needs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Locator {
    /// The document's address as linked.
    pub url: String,
    /// GitHub owner, for GitHub kinds.
    pub owner: Option<String>,
    /// GitHub repository, for GitHub kinds.
    pub repo: Option<String>,
    /// GitHub ref as written (first segment after `blob/`; a branch name with a
    /// slash needs API resolution, which the fetcher owns).
    pub ref_name: Option<String>,
    /// Path inside the repository, or the paste/document id for other kinds.
    pub path: String,
}

/// One document in the fold: its mentions and the best name known without fetching.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Canonical document key, e.g. `github:owner/repo@main:docs/a.md`.
    pub key: String,
    /// What kind of document it is.
    pub kind: DocKind,
    /// What a fetcher needs to fetch it.
    pub locator: Locator,
    /// Last path segment, for the dimmed row.
    pub file_name: String,
    /// The first link label seen, when any mention carried one.
    pub label: Option<String>,
    /// Every mention, oldest first.
    pub mentions: Vec<Mention>,
}

impl Entry {
    /// When this document was first linked, in Unix milliseconds, when known.
    #[must_use]
    pub fn first_mentioned_ms(&self) -> Option<i64> {
        self.mentions.iter().filter_map(|m| m.posted_ms).min()
    }
    /// When this document was last linked, in Unix milliseconds, when known.
    #[must_use]
    pub fn last_mentioned_ms(&self) -> Option<i64> {
        self.mentions.iter().filter_map(|m| m.posted_ms).max()
    }
    /// The name to show before fetching: label, else file name.
    #[must_use]
    pub fn display_name(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.file_name)
    }
}

/// One recognised document link in a text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recognized {
    /// Canonical document key.
    pub key: String,
    /// What kind of document it is.
    pub kind: DocKind,
    /// What a fetcher needs.
    pub locator: Locator,
    /// Last path segment.
    pub file_name: String,
    /// The link's label, when given.
    pub label: Option<String>,
}

fn valid_name(part: &str) -> bool {
    !part.is_empty()
        && part.len() <= 100
        && part
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'+'))
}

fn dot_segment(segment: &str) -> bool {
    let mut rest = segment;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('.') {
            rest = after;
        } else if rest
            .get(..3)
            .is_some_and(|head| head.eq_ignore_ascii_case("%2e"))
        {
            rest = &rest[3..];
        } else {
            return false;
        }
    }
    !segment.is_empty()
}

/// Recognise one address, or `None` when it is not a document this knows.
///
/// Strict, as [`crate::link_labels`] is: no decoding, no dot segments, no
/// backslash, space or control character, so Slack's file-name links
/// (`http://validate.rs`) and code spans never become documents.
#[must_use]
pub fn recognize_url(
    address: &str,
    label: Option<&str>,
    paste_prefixes: &[String],
) -> Option<Recognized> {
    if address.len() > MAX_ADDRESS
        || address
            .bytes()
            .any(|b| b == b'\\' || b == b' ' || b.is_ascii_control())
    {
        return None;
    }
    let rest = crate::link_labels::strip_scheme(address)?;
    let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (host, tail) = rest.split_at(host_end);
    let tail = tail.split('#').next().unwrap_or("");
    let path = tail.split('?').next().unwrap_or("");
    let path = path.strip_prefix('/').unwrap_or(path);
    let path = path.strip_suffix('/').unwrap_or(path);
    let segments: Vec<&str> = if path.is_empty() {
        Vec::new()
    } else {
        path.split('/').collect()
    };
    if segments.iter().any(|s| s.is_empty() || dot_segment(s)) {
        return None;
    }
    let label = label.map(str::to_owned).filter(|l| !l.trim().is_empty());
    let mk = |kind: DocKind, key: String, locator: Locator, file_name: String| Recognized {
        key,
        kind,
        locator,
        file_name,
        label: label.clone(),
    };
    if host.eq_ignore_ascii_case("github.com") || host.eq_ignore_ascii_case("www.github.com") {
        let [owner, repo, blob, r#ref, rest @ ..] = segments.as_slice() else {
            return None;
        };
        if *blob != "blob" || !valid_name(owner) || !valid_name(repo) || rest.is_empty() {
            return None;
        }
        let file_path = rest.join("/");
        if file_path
            .split('/')
            .any(|s| !valid_name(s) && !s.contains('.'))
        {
            // Path segments are file names: dots are ordinary there.
        }
        let lower = file_path.to_ascii_lowercase();
        let kind = if lower.ends_with(".md") || lower.ends_with(".markdown") {
            DocKind::GithubMarkdown
        } else {
            DocKind::GithubFile
        };
        let file_name = rest.last().copied().unwrap_or("").to_owned();
        return Some(mk(
            kind,
            format!("github:{owner}/{repo}@{ref}:{file_path}"),
            Locator {
                url: address.to_owned(),
                owner: Some((*owner).to_owned()),
                repo: Some((*repo).to_owned()),
                ref_name: Some((*r#ref).to_owned()),
                path: file_path,
            },
            file_name,
        ));
    }
    if host.eq_ignore_ascii_case("raw.githubusercontent.com") {
        let [owner, repo, r#ref, rest @ ..] = segments.as_slice() else {
            return None;
        };
        if !valid_name(owner) || !valid_name(repo) || rest.is_empty() {
            return None;
        }
        let file_path = rest.join("/");
        let lower = file_path.to_ascii_lowercase();
        let kind = if lower.ends_with(".md") || lower.ends_with(".markdown") {
            DocKind::GithubMarkdown
        } else {
            DocKind::GithubFile
        };
        return Some(mk(
            kind,
            format!("github:{owner}/{repo}@{ref}:{file_path}"),
            Locator {
                url: address.to_owned(),
                owner: Some((*owner).to_owned()),
                repo: Some((*repo).to_owned()),
                ref_name: Some((*r#ref).to_owned()),
                path: file_path,
            },
            rest.last().copied().unwrap_or("").to_owned(),
        ));
    }
    if host.eq_ignore_ascii_case("docs.google.com") {
        if let [doc, d, id, ..] = segments.as_slice() {
            if *doc == "document" && *d == "d" && valid_name(id) {
                return Some(mk(
                    DocKind::GoogleDoc,
                    format!("gdoc:{id}"),
                    Locator {
                        url: address.to_owned(),
                        owner: None,
                        repo: None,
                        ref_name: None,
                        path: (*id).to_owned(),
                    },
                    (*id).to_owned(),
                ));
            }
        }
        return None;
    }
    for prefix in paste_prefixes {
        if let Some(after) = address.strip_prefix(prefix.as_str()) {
            let id = after.split(['/', '?', '#']).next().unwrap_or("").to_owned();
            let ok = id.len() <= 64
                && id
                    .bytes()
                    .enumerate()
                    .all(|(i, b)| b.is_ascii_alphanumeric() || (i > 0 && matches!(b, b'_' | b'-')))
                && !id.is_empty();
            if ok {
                return Some(mk(
                    DocKind::Paste,
                    format!("paste:{id}"),
                    Locator {
                        url: address.to_owned(),
                        owner: None,
                        repo: None,
                        ref_name: None,
                        path: id.clone(),
                    },
                    id,
                ));
            }
        }
    }
    None
}

/// Every document link in `text`: Markdown links, Slack `<url|label>` links and
/// bare addresses, deduplicated by key (first label wins).
#[must_use]
pub fn recognize(text: &str, paste_prefixes: &[String]) -> Vec<Recognized> {
    let mut found: Vec<Recognized> = Vec::new();
    let push = |rec: Recognized, found: &mut Vec<Recognized>| {
        if let Some(old) = found.iter_mut().find(|r| r.key == rec.key) {
            if old.label.is_none() {
                old.label = rec.label;
            }
        } else {
            found.push(rec);
        }
    };
    // Markdown [label](url) and Slack <url|label>.
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            if let Some(close) = text[i..].find("](") {
                let label = &text[i + 1..i + close];
                let start = i + close + 2;
                if let Some(end) = text[start..].find([')', ' ', '\n']) {
                    let url = &text[start..start + end];
                    if let Some(rec) = recognize_url(url, Some(label), paste_prefixes) {
                        push(rec, &mut found);
                    }
                    i = start + end;
                    continue;
                }
            }
        }
        if bytes[i] == b'<' {
            if let Some(close) = text[i..].find('>') {
                let inner = &text[i + 1..i + close];
                let (url, label) = match inner.split_once('|') {
                    Some((u, l)) => (u, Some(l)),
                    None => (inner, None),
                };
                if let Some(rec) = recognize_url(url.trim(), label, paste_prefixes) {
                    push(rec, &mut found);
                }
                i += close + 1;
                continue;
            }
        }
        i += 1;
    }
    // Bare addresses.
    for word in text.split(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'' | '`'))
    {
        let word = word.trim_matches(|c| matches!(c, '(' | '[' | '{'));
        let word = word.trim_end_matches(|c| matches!(c, ')' | ']' | '}' | '.' | ',' | ';' | ':'));
        if word.contains("://") {
            if let Some(rec) = recognize_url(word, None, paste_prefixes) {
                push(rec, &mut found);
            }
        }
    }
    found
}

/// One change in the fold, for `since_revision` sync. Local state only ever
/// gains information: a change enriches or removes a record, never rewrites
/// history below the revision a reader already holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    /// Fold revision at which this change landed.
    pub revision: u64,
    /// Document key that changed.
    pub key: String,
    /// Whether the document left the fold at this revision.
    pub deleted: bool,
}

/// The most changes kept for `since_revision` sync.
const MAX_CHANGES: usize = 4096;

/// The fold state: document key -> entry, plus which keys each message holds.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Fold {
    docs: HashMap<String, Entry>,
    by_message: HashMap<String, Vec<String>>,
    revision: u64,
    #[serde(default)]
    changes: VecDeque<Change>,
}

fn message_slot(channel_id: &str, message_id: &str) -> String {
    format!("{channel_id}\u{1f}{message_id}")
}

/// The incremental document index. Share it; every method takes a short lock.
pub struct Index {
    paste_prefixes: Vec<String>,
    fold: RwLock<Fold>,
    persist_path: Option<std::path::PathBuf>,
}

impl std::fmt::Debug for Index {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Index")
            .field("documents", &self.len())
            .finish_non_exhaustive()
    }
}

impl Index {
    /// An empty index recognising pastes under `paste_prefixes`.
    #[must_use]
    pub fn new(paste_prefixes: Vec<String>) -> Self {
        Self {
            paste_prefixes,
            fold: RwLock::new(Fold::default()),
            persist_path: None,
        }
    }

    /// An index that persists its fold as JSON at `path`, loading it when present.
    ///
    /// # Errors
    ///
    /// Returns the I/O or JSON error when an existing file cannot be read.
    pub fn load(paste_prefixes: Vec<String>, path: std::path::PathBuf) -> Result<Self, String> {
        let fold = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| format!("documents index {}: {e}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Fold::default(),
            Err(e) => return Err(format!("documents index {}: {e}", path.display())),
        };
        Ok(Self {
            paste_prefixes,
            fold: RwLock::new(fold),
            persist_path: Some(path),
        })
    }

    /// The pure fold: build an index from messages from scratch.
    #[must_use]
    pub fn from_messages<I>(paste_prefixes: Vec<String>, messages: I) -> Self
    where
        I: IntoIterator<Item = (String, String, Option<i64>, String, Option<String>, String)>,
    {
        let index = Self::new(paste_prefixes);
        for (channel, message, posted_ms, author, thread, text) in messages {
            index.apply_message(
                &channel,
                &message,
                posted_ms,
                &author,
                thread.as_deref(),
                &text,
            );
        }
        index
    }

    /// The current fold revision. Every mutation bumps it by one.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.fold.read().unwrap_or_else(|p| p.into_inner()).revision
    }

    /// What changed since `revision`: changed entries, deleted keys, whether
    /// the change log no longer reaches back that far, and the current revision.
    #[must_use]
    pub fn changes_since(&self, revision: u64) -> (Vec<Entry>, Vec<String>, bool, u64) {
        let fold = self.fold.read().unwrap_or_else(|p| p.into_inner());
        let truncated = fold
            .changes
            .front()
            .is_some_and(|c| c.revision > revision + 1);
        let mut latest: HashMap<String, bool> = HashMap::new();
        let mut order: Vec<String> = Vec::new();
        for change in fold.changes.iter().filter(|c| c.revision > revision) {
            if !latest.contains_key(&change.key) {
                order.push(change.key.clone());
            }
            latest.insert(change.key.clone(), change.deleted);
        }
        let mut changed = Vec::new();
        let mut deleted = Vec::new();
        for key in order {
            if latest[&key] {
                deleted.push(key);
            } else if let Some(entry) = fold.docs.get(&key) {
                changed.push(entry.clone());
            }
        }
        (changed, deleted, truncated, fold.revision)
    }

    fn bump(fold: &mut Fold, affected: &[String]) {
        if affected.is_empty() {
            return;
        }
        fold.revision += 1;
        let revision = fold.revision;
        for key in affected {
            let deleted = !fold.docs.contains_key(key);
            fold.changes.push_back(Change {
                revision,
                key: key.clone(),
                deleted,
            });
        }
        while fold.changes.len() > MAX_CHANGES {
            fold.changes.pop_front();
        }
    }

    fn persist(&self, fold: &Fold) {
        if let Some(path) = &self.persist_path {
            if let Ok(text) = serde_json::to_string(fold) {
                let _ = std::fs::write(path, text);
            }
        }
    }

    /// Fold one message in, replacing whatever it mentioned before (an edit).
    pub fn apply_message(
        &self,
        channel_id: &str,
        message_id: &str,
        posted_ms: Option<i64>,
        author: &str,
        thread_id: Option<&str>,
        text: &str,
    ) {
        let recognised = recognize(text, &self.paste_prefixes);
        let slot = message_slot(channel_id, message_id);
        let mut fold = self.fold.write().unwrap_or_else(|p| p.into_inner());
        let mut affected: Vec<String> = fold.by_message.get(&slot).cloned().unwrap_or_default();
        self.remove_slot(&mut fold, &slot);
        let mut keys = Vec::new();
        for rec in recognised {
            keys.push(rec.key.clone());
            let entry = fold.docs.entry(rec.key.clone()).or_insert_with(|| Entry {
                key: rec.key.clone(),
                kind: rec.kind,
                locator: rec.locator.clone(),
                file_name: rec.file_name.clone(),
                label: rec.label.clone(),
                mentions: Vec::new(),
            });
            if entry.label.is_none() {
                entry.label = rec.label.clone();
            }
            entry.mentions.push(Mention {
                channel_id: channel_id.to_owned(),
                message_id: message_id.to_owned(),
                thread_id: thread_id.map(str::to_owned),
                posted_ms,
                author: author.to_owned(),
                label: rec.label.clone(),
            });
            entry
                .mentions
                .sort_by_key(|m| m.posted_ms.unwrap_or(i64::MIN));
        }
        if !keys.is_empty() {
            fold.by_message.insert(slot, keys.clone());
        }
        for key in keys {
            if !affected.contains(&key) {
                affected.push(key);
            }
        }
        Self::bump(&mut fold, &affected);
        self.persist(&fold);
    }

    fn remove_slot(&self, fold: &mut Fold, slot: &str) {
        if let Some(keys) = fold.by_message.remove(slot) {
            for key in keys {
                let empty = fold.docs.get_mut(&key).is_some_and(|entry| {
                    entry
                        .mentions
                        .retain(|m| message_slot(&m.channel_id, &m.message_id) != slot);
                    entry.mentions.is_empty()
                });
                if empty {
                    fold.docs.remove(&key);
                }
            }
        }
    }

    /// Fold one message out (a deletion).
    pub fn remove_message(&self, channel_id: &str, message_id: &str) {
        let slot = message_slot(channel_id, message_id);
        let mut fold = self.fold.write().unwrap_or_else(|p| p.into_inner());
        let affected = fold.by_message.get(&slot).cloned().unwrap_or_default();
        self.remove_slot(&mut fold, &slot);
        Self::bump(&mut fold, &affected);
        self.persist(&fold);
    }

    /// Fold every message of one channel out.
    pub fn remove_channel(&self, channel_id: &str) {
        let mut fold = self.fold.write().unwrap_or_else(|p| p.into_inner());
        let slots: Vec<String> = fold
            .by_message
            .keys()
            .filter(|s| s.split('\u{1f}').next() == Some(channel_id))
            .cloned()
            .collect();
        let mut affected = Vec::new();
        for slot in slots {
            if let Some(keys) = fold.by_message.get(&slot) {
                affected.extend(keys.iter().cloned());
            }
            self.remove_slot(&mut fold, &slot);
        }
        Self::bump(&mut fold, &affected);
        self.persist(&fold);
    }

    /// Forget everything (a storage purge).
    pub fn clear(&self) {
        let mut fold = self.fold.write().unwrap_or_else(|p| p.into_inner());
        let affected: Vec<String> = fold.docs.keys().cloned().collect();
        let revision = fold.revision;
        *fold = Fold::default();
        fold.revision = revision;
        Self::bump(&mut fold, &affected);
        self.persist(&fold);
    }

    /// Every document, in mention order: by last mention, oldest first, so the
    /// newest mention is at the bottom of the list, as in a message feed.
    #[must_use]
    pub fn list(&self) -> Vec<Entry> {
        let fold = self.fold.read().unwrap_or_else(|p| p.into_inner());
        let mut entries: Vec<Entry> = fold.docs.values().cloned().collect();
        entries.sort_by(|a, b| {
            a.last_mentioned_ms()
                .unwrap_or(i64::MIN)
                .cmp(&b.last_mentioned_ms().unwrap_or(i64::MIN))
                .then_with(|| a.key.cmp(&b.key))
        });
        entries
    }

    /// One document by key.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<Entry> {
        self.fold
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .docs
            .get(key)
            .cloned()
    }

    /// How many documents the fold holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.fold
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .docs
            .len()
    }

    /// Whether the fold holds no documents.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The paste prefixes this index recognises.
    #[must_use]
    pub fn paste_prefixes(&self) -> &[String] {
        &self.paste_prefixes
    }
}

/// A fetched document, as the bounded cache holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Fetched {
    /// Document key.
    pub key: String,
    /// What kind of document it is.
    pub kind: DocKind,
    /// Real title: front matter, else first heading, else label/file name.
    pub title: String,
    /// Raw Markdown source.
    pub content: String,
    /// Content version: SHA-256 of the content, hex.
    pub version: String,
    /// When it was fetched, in Unix milliseconds.
    pub fetched_ms: i64,
    /// Headings, in order, for the outline.
    pub outline: Vec<String>,
}

/// Why a fetch or freshness check failed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FetchError {
    /// No fetcher serves this kind yet.
    #[error("no fetcher for {0} documents yet")]
    Unsupported(&'static str),
    /// The source refused or failed.
    #[error("fetch failed: {0}")]
    Failed(String),
}

/// SHA-256 of `content`, hex: the version a fetched document is cached under.
#[must_use]
pub fn version_of(content: &str) -> String {
    use ring::digest::{digest, SHA256};
    let hash = digest(&SHA256, content.as_bytes());
    hash.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// The title and outline of a Markdown document.
#[must_use]
pub fn title_and_outline(content: &str, fallback: &str) -> (String, Vec<String>) {
    let mut outline = Vec::new();
    let mut title = None;
    let mut lines = content.lines().peekable();
    // Front matter title.
    if lines.peek().is_some_and(|l| l.trim() == "---") {
        lines.next();
        for line in lines.by_ref() {
            if line.trim() == "---" {
                break;
            }
            if let Some(value) = line.trim().strip_prefix("title:") {
                title = Some(value.trim().trim_matches('"').to_owned());
            }
        }
    }
    for line in content.lines() {
        let line = line.trim_end();
        if let Some(heading) = line.strip_prefix("# ") {
            outline.push(heading.trim().to_owned());
            if title.is_none() {
                title = Some(heading.trim().to_owned());
            }
        } else if let Some(heading) = line.strip_prefix("## ") {
            outline.push(heading.trim().to_owned());
        }
    }
    (title.unwrap_or_else(|| fallback.to_owned()), outline)
}

/// One source document, as a fetcher returns it before titling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchedRaw {
    /// Raw Markdown source.
    pub content: String,
    /// The source's own version (an ETag or blob id), when it gives one.
    pub version: Option<String>,
}

/// Fetches one document kind. Implemented by the GitHub and paste sources,
/// and by fakes in tests.
#[async_trait::async_trait]
pub trait Fetcher: Send + Sync {
    /// Fetch the document `entry` names.
    async fn fetch(&self, entry: &Entry) -> Result<FetchedRaw, FetchError>;
    /// The version the source now holds for `entry`, without content when the
    /// source can say cheaply; `None` when it cannot say without fetching.
    async fn current_version(&self, entry: &Entry) -> Result<Option<String>, FetchError>;
}

/// The GitHub fetcher: raw content over HTTPS, with an optional file token.
pub struct GithubFetcher {
    client: reqwest::Client,
    token: Option<String>,
    raw_base: String,
}

impl GithubFetcher {
    /// A fetcher against `raw_base` (default `https://raw.githubusercontent.com`).
    #[must_use]
    pub fn new(token: Option<String>, raw_base: Option<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            token,
            raw_base: raw_base
                .unwrap_or_else(|| "https://raw.githubusercontent.com".to_owned())
                .trim_end_matches('/')
                .to_owned(),
        }
    }

    fn url(&self, entry: &Entry) -> Result<String, FetchError> {
        let (owner, repo, r#ref) = (
            entry.locator.owner.as_deref(),
            entry.locator.repo.as_deref(),
            entry.locator.ref_name.as_deref(),
        );
        match (owner, repo, r#ref) {
            (Some(o), Some(r), Some(f)) => Ok(format!(
                "{}/{}/{}/{}/{}",
                self.raw_base, o, r, f, entry.locator.path
            )),
            _ => Err(FetchError::Failed("not a GitHub document".to_owned())),
        }
    }

    async fn get(&self, entry: &Entry) -> Result<FetchedRaw, FetchError> {
        let url = self.url(entry)?;
        let mut request = self.client.get(&url).header("user-agent", "vibe-talk");
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .await
            .map_err(|e| FetchError::Failed(e.to_string()))?;
        if !response.status().is_success() {
            return Err(FetchError::Failed(format!(
                "GitHub answered {}",
                response.status()
            )));
        }
        let etag = response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let content = response
            .text()
            .await
            .map_err(|e| FetchError::Failed(e.to_string()))?;
        if content.len() > MAX_DOCUMENT_BYTES {
            return Err(FetchError::Failed("document over 1 MiB".to_owned()));
        }
        Ok(FetchedRaw {
            version: etag.or_else(|| Some(version_of(&content))),
            content,
        })
    }
}

#[async_trait::async_trait]
impl Fetcher for GithubFetcher {
    async fn fetch(&self, entry: &Entry) -> Result<FetchedRaw, FetchError> {
        match entry.kind {
            DocKind::GithubMarkdown | DocKind::GithubFile => self.get(entry).await,
            kind => Err(FetchError::Unsupported(kind.as_str())),
        }
    }

    async fn current_version(&self, entry: &Entry) -> Result<Option<String>, FetchError> {
        Ok(self.fetch(entry).await?.version)
    }
}

/// The paste fetcher: a configured command, run without a shell, after `--`.
pub struct PasteFetcher {
    argv: Vec<String>,
}

impl PasteFetcher {
    /// A fetcher running `argv` with the paste id in place of `{}`.
    #[must_use]
    pub fn new(argv: Vec<String>) -> Self {
        Self { argv }
    }
}

#[async_trait::async_trait]
impl Fetcher for PasteFetcher {
    async fn fetch(&self, entry: &Entry) -> Result<FetchedRaw, FetchError> {
        if entry.kind != DocKind::Paste {
            return Err(FetchError::Unsupported(entry.kind.as_str()));
        }
        if self.argv.is_empty() {
            return Err(FetchError::Failed("no paste command configured".to_owned()));
        }
        let argv: Vec<String> = self
            .argv
            .iter()
            .map(|a| a.replace("{}", &entry.locator.path))
            .collect();
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            tokio::process::Command::new(&argv[0])
                .args(&argv[1..])
                .output(),
        )
        .await
        .map_err(|_| FetchError::Failed("paste command timed out".to_owned()))?
        .map_err(|e| FetchError::Failed(e.to_string()))?;
        if !output.status.success() {
            return Err(FetchError::Failed(format!(
                "paste command answered {}",
                output.status
            )));
        }
        if output.stdout.len() > MAX_DOCUMENT_BYTES {
            return Err(FetchError::Failed("document over 1 MiB".to_owned()));
        }
        let content = String::from_utf8_lossy(&output.stdout).into_owned();
        let version = Some(version_of(&content));
        Ok(FetchedRaw { content, version })
    }

    async fn current_version(&self, entry: &Entry) -> Result<Option<String>, FetchError> {
        Ok(self.fetch(entry).await?.version)
    }
}

/// Fetchers by kind, tried in order. The service's one fetcher.
pub struct CompositeFetcher {
    fetchers: Vec<Arc<dyn Fetcher>>,
}

impl CompositeFetcher {
    /// Try each fetcher in order; the first that does not say Unsupported wins.
    #[must_use]
    pub fn new(fetchers: Vec<Arc<dyn Fetcher>>) -> Self {
        Self { fetchers }
    }
}

#[async_trait::async_trait]
impl Fetcher for CompositeFetcher {
    async fn fetch(&self, entry: &Entry) -> Result<FetchedRaw, FetchError> {
        let mut unsupported = FetchError::Unsupported(entry.kind.as_str());
        for fetcher in &self.fetchers {
            match fetcher.fetch(entry).await {
                Err(e @ FetchError::Unsupported(_)) => unsupported = e,
                other => return other,
            }
        }
        Err(unsupported)
    }

    async fn current_version(&self, entry: &Entry) -> Result<Option<String>, FetchError> {
        let mut unsupported = FetchError::Unsupported(entry.kind.as_str());
        for fetcher in &self.fetchers {
            match fetcher.current_version(entry).await {
                Err(e @ FetchError::Unsupported(_)) => unsupported = e,
                other => return other,
            }
        }
        Err(unsupported)
    }
}

struct CacheState {
    docs: HashMap<String, Fetched>,
    order: VecDeque<String>,
    bytes: usize,
    max_entries: usize,
    max_bytes: usize,
}

/// Build the service from `[documents]` configuration: load the persisted
/// fold when a path is configured, and wire the GitHub and paste fetchers.
#[must_use]
pub fn service_from_config(config: &crate::config::DocumentsConfig) -> Arc<Service> {
    let index = match &config.index_path {
        Some(path) => Arc::new(
            Index::load(config.paste_url_prefixes.clone(), path.clone())
                .unwrap_or_else(|_| Index::new(config.paste_url_prefixes.clone())),
        ),
        None => Arc::new(Index::new(config.paste_url_prefixes.clone())),
    };
    let token = config
        .github_token_file
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty());
    let mut fetchers: Vec<Arc<dyn Fetcher>> = vec![Arc::new(GithubFetcher::new(
        token,
        config.github_raw_base.clone(),
    ))];
    if !config.paste_command.is_empty() {
        fetchers.push(Arc::new(PasteFetcher::new(config.paste_command.clone())));
    }
    Arc::new(Service::new(
        index,
        Arc::new(CompositeFetcher::new(fetchers)),
        config.max_cached_documents,
        config.max_cached_bytes,
    ))
}

/// The documents service: the fold index plus the bounded fetched cache.
pub struct Service {
    /// The fold index.
    pub index: Arc<Index>,
    cache: Mutex<CacheState>,
    fetcher: Arc<dyn Fetcher>,
}

impl Service {
    /// A service over `index`, fetching through `fetcher`, with cache bounds.
    #[must_use]
    pub fn new(
        index: Arc<Index>,
        fetcher: Arc<dyn Fetcher>,
        max_entries: usize,
        max_bytes: usize,
    ) -> Self {
        Self {
            index,
            cache: Mutex::new(CacheState {
                docs: HashMap::new(),
                order: VecDeque::new(),
                bytes: 0,
                max_entries: max_entries.max(1),
                max_bytes: max_bytes.max(1),
            }),
            fetcher,
        }
    }

    /// A service with the default cache bounds and no fetcher that can fetch.
    #[must_use]
    pub fn unfetchable(index: Arc<Index>) -> Self {
        struct NoneFetcher;
        #[async_trait::async_trait]
        impl Fetcher for NoneFetcher {
            async fn fetch(&self, entry: &Entry) -> Result<FetchedRaw, FetchError> {
                Err(FetchError::Unsupported(entry.kind.as_str()))
            }
            async fn current_version(&self, entry: &Entry) -> Result<Option<String>, FetchError> {
                Err(FetchError::Unsupported(entry.kind.as_str()))
            }
        }
        Self::new(
            index,
            Arc::new(NoneFetcher),
            DEFAULT_MAX_CACHED,
            DEFAULT_MAX_CACHED_BYTES,
        )
    }

    /// The fetched copy of `key`, when cached.
    #[must_use]
    pub fn cached(&self, key: &str) -> Option<Fetched> {
        self.cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .docs
            .get(key)
            .cloned()
    }

    /// How many documents are cached, and their total content bytes.
    #[must_use]
    pub fn cache_stats(&self) -> (usize, usize) {
        let cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
        (cache.docs.len(), cache.bytes)
    }

    fn put(&self, fetched: Fetched) {
        let mut cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(old) = cache.docs.remove(&fetched.key) {
            cache.bytes = cache.bytes.saturating_sub(old.content.len());
            cache.order.retain(|k| k != &fetched.key);
        }
        cache.bytes += fetched.content.len();
        cache.order.push_back(fetched.key.clone());
        cache.docs.insert(fetched.key.clone(), fetched);
        while cache.docs.len() > cache.max_entries || cache.bytes > cache.max_bytes {
            let Some(key) = cache.order.pop_front() else {
                break;
            };
            if let Some(old) = cache.docs.remove(&key) {
                cache.bytes = cache.bytes.saturating_sub(old.content.len());
            }
        }
    }

    /// Fetch one document and cache it: the "sync this item" operation.
    /// Idempotent: a cached copy is returned as it stands; use
    /// [`Service::fetch_one_refresh`] to replace it with the newest version.
    ///
    /// # Errors
    ///
    /// [`FetchError`] when no fetcher serves its kind or the source fails.
    pub async fn fetch_one(&self, key: &str) -> Result<Fetched, FetchError> {
        if let Some(cached) = self.cached(key) {
            return Ok(cached);
        }
        self.fetch_one_refresh(key).await
    }

    /// Fetch one document now, replacing any cached copy.
    ///
    /// # Errors
    ///
    /// [`FetchError`] when no fetcher serves its kind or the source fails.
    pub async fn fetch_one_refresh(&self, key: &str) -> Result<Fetched, FetchError> {
        let entry = self
            .index
            .get(key)
            .ok_or_else(|| FetchError::Failed(format!("unknown document {key}")))?;
        let raw = self.fetcher.fetch(&entry).await?;
        if raw.content.len() > MAX_DOCUMENT_BYTES {
            return Err(FetchError::Failed("document over 1 MiB".to_owned()));
        }
        let (title, outline) = title_and_outline(&raw.content, entry.display_name());
        let fetched = Fetched {
            key: entry.key.clone(),
            kind: entry.kind,
            title,
            version: raw.version.unwrap_or_else(|| version_of(&raw.content)),
            fetched_ms: crate::cache::wall_ms(),
            outline,
            content: raw.content,
        };
        self.put(fetched.clone());
        Ok(fetched)
    }

    /// Fetch the most recent `n` unfetched documents, newest mention first in
    /// the attempt order. Returns what was fetched; failures are skipped.
    pub async fn fetch_recent(&self, n: usize) -> Vec<Fetched> {
        let n = n.clamp(1, MAX_FETCH_RECENT);
        let mut candidates: Vec<Entry> = self
            .index
            .list()
            .into_iter()
            .filter(|e| self.cached(&e.key).is_none())
            .collect();
        candidates.reverse();
        let mut fetched = Vec::new();
        for entry in candidates.into_iter().take(n) {
            if let Ok(doc) = self.fetch_one(&entry.key).await {
                fetched.push(doc);
            }
        }
        fetched
    }

    /// Whether the source now holds a newer version than the cached one.
    /// Best effort: an unfetched document, or one whose source cannot say,
    /// reports no update.
    ///
    /// # Errors
    ///
    /// [`FetchError`] when the freshness check itself fails.
    pub async fn freshness(&self, key: &str) -> Result<Freshness, FetchError> {
        let entry = self
            .index
            .get(key)
            .ok_or_else(|| FetchError::Failed(format!("unknown document {key}")))?;
        let Some(cached) = self.cached(key) else {
            return Ok(Freshness {
                key: key.to_owned(),
                fetched: false,
                version: None,
                latest_version: None,
                update_available: false,
            });
        };
        let latest = self.fetcher.current_version(&entry).await?;
        Ok(Freshness {
            key: key.to_owned(),
            fetched: true,
            update_available: latest.as_deref().is_some_and(|v| v != cached.version),
            version: Some(cached.version),
            latest_version: latest,
        })
    }
}

/// The answer of a freshness check.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Freshness {
    /// Document key.
    pub key: String,
    /// Whether a fetched copy is cached.
    pub fetched: bool,
    /// The cached version, when fetched.
    pub version: Option<String>,
    /// The version the source now holds, when it could say.
    pub latest_version: Option<String>,
    /// Whether the source holds a different version than the cached one.
    pub update_available: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefixes() -> Vec<String> {
        vec!["https://paste.example/".to_owned()]
    }

    #[test]
    fn recognises_github_markdown_and_files() {
        let rec = recognize(
            "see [the report](https://github.com/octo/widgets/blob/main/docs/report.md) and https://github.com/octo/widgets/blob/main/data/x.json",
            &prefixes(),
        );
        assert_eq!(rec.len(), 2);
        assert_eq!(rec[0].kind, DocKind::GithubMarkdown);
        assert_eq!(rec[0].key, "github:octo/widgets@main:docs/report.md");
        assert_eq!(rec[0].label.as_deref(), Some("the report"));
        assert_eq!(rec[1].kind, DocKind::GithubFile);
    }

    #[test]
    fn ignores_file_name_links_and_dot_segments() {
        assert!(recognize("open http://validate.rs now", &prefixes()).is_empty());
        assert!(recognize_url(
            "https://github.com/octo/widgets/blob/main/../../other/x.md",
            None,
            &prefixes()
        )
        .is_none());
        assert!(recognize_url(
            "https://github.com/octo/widgets/tree/main/docs",
            None,
            &prefixes()
        )
        .is_none());
    }

    #[test]
    fn recognises_pastes_and_google_docs_as_kinds() {
        let rec = recognize(
            "<https://paste.example/Ab12|a paste> https://docs.google.com/document/d/DocId123/edit",
            &prefixes(),
        );
        assert_eq!(rec.len(), 2);
        assert_eq!(rec[0].kind, DocKind::Paste);
        assert_eq!(rec[0].key, "paste:Ab12");
        assert_eq!(rec[1].kind, DocKind::GoogleDoc);
    }

    #[test]
    fn fold_replaces_on_edit_and_removes_on_delete() {
        let index = Index::new(prefixes());
        index.apply_message(
            "c1",
            "m1",
            Some(10),
            "agent",
            None,
            "[a](https://github.com/o/r/blob/main/a.md)",
        );
        index.apply_message(
            "c1",
            "m2",
            Some(20),
            "agent",
            None,
            "https://github.com/o/r/blob/main/a.md again",
        );
        assert_eq!(index.len(), 1);
        assert_eq!(index.list()[0].mentions.len(), 2);
        // Edit m1 to link b instead: a keeps only m2, b appears.
        index.apply_message(
            "c1",
            "m1",
            Some(10),
            "agent",
            None,
            "[b](https://github.com/o/r/blob/main/b.md)",
        );
        assert_eq!(index.len(), 2);
        index.remove_message("c1", "m2");
        index.remove_message("c1", "m1");
        assert!(index.is_empty());
    }

    #[test]
    fn list_is_in_mention_order_newest_last() {
        let index = Index::from_messages(
            prefixes(),
            vec![
                (
                    "c".into(),
                    "m1".into(),
                    Some(1),
                    "a".into(),
                    None,
                    "https://github.com/o/r/blob/main/a.md".into(),
                ),
                (
                    "c".into(),
                    "m2".into(),
                    Some(2),
                    "a".into(),
                    None,
                    "https://github.com/o/r/blob/main/b.md".into(),
                ),
            ],
        );
        let listed = index.list();
        let keys: Vec<&str> = listed.iter().map(|e| e.key.as_str()).collect();
        assert!(keys[0].ends_with("a.md") && keys[1].ends_with("b.md"));
    }
}
