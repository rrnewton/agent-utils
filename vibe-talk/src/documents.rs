//! The document index: a fold over the cached messages. `#628 documents-index-server`.
//!
//! Design: `ai_docs/transient/2026-10-26-documents-view/README.md`, as decided by the
//! owner on 2026-10-28. Every message with a link recognised as a document registers
//! that document candidate (existence only); nothing is fetched eagerly. The index is
//! a pure function of the cached messages, maintained incrementally as messages are
//! cached (live, backfill, edit, delete) and rebuilt or loaded at startup. It caches
//! `document key -> mentions (when/where)`, never document contents; fetched contents
//! live in a separate bounded cache, persisted beside the fold
//! (`documents-fetched.json`) so a restart does not lose what was downloaded.
//! The list path reads only the fold result and that cache.
//!
//! One admission sits beside the fold (`#1081 documents-relative-links`): a
//! write-scope fetch naming a canonical key no message has mentioned registers
//! a mention-less entry ([`Index::register_key`]), so a link followed from one
//! document to another can open. Canonicalisation is [`parse_key`], which goes
//! through [`recognize_url`] under the live configuration — never a free-form
//! address fetch.

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
    /// A minibeads issue: a GitHub `blob/…/.minibeads/issues/<prefix>-<n>.md` page.
    /// `#1176 doc-kind-buckets`.
    GithubIssue,
    /// A GitHub gist: `gist.github.com/<owner>/<id>` (the Pastes bucket's GitHub half).
    /// `#1176 doc-kind-buckets`.
    GithubGist,
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
            Self::GithubIssue => "github_issue",
            Self::GithubGist => "github_gist",
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

/// The issue number of a minibeads issue path (`.minibeads/issues/<prefix>-<n>.md`),
/// or `None` when `path` is not exactly that shape. `#1176 doc-kind-buckets`.
#[must_use]
pub fn issue_number_of(path: &str) -> Option<u64> {
    let mut parts = path.split('/');
    if parts.next() != Some(".minibeads") || parts.next() != Some("issues") {
        return None;
    }
    let file = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let stem = file.strip_suffix(".md")?;
    let (prefix, digits) = stem.rsplit_once('-')?;
    if !valid_name(prefix) || digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// The `status:` of a document's YAML front matter, when it opens with one.
/// `#1176 doc-kind-buckets`: an issue's status (open, in_progress, closed).
#[must_use]
pub fn issue_status_of(content: &str) -> Option<String> {
    let mut lines = content.lines();
    if lines.next().map(str::trim) != Some("---") {
        return None;
    }
    for line in lines {
        if line.trim() == "---" {
            return None;
        }
        if let Some(value) = line.trim().strip_prefix("status:") {
            let value = value.trim().trim_matches('"').trim_matches('\'');
            return (!value.is_empty()).then(|| value.to_owned());
        }
    }
    None
}

/// The body of an issue document after its YAML front matter, or the whole
/// content where it does not open with a complete `---` front-matter block.
/// `#2004 vibe-talk-read-highlight-missing`: the reader draws (and cuts for
/// reading) only this body — its header carries the front matter instead
/// (`splitIssueHeader` in the page) — so the server's speech cut must be
/// taken from the same body, or every block index shifts by the front
/// matter's block and the reading light never lands.
#[must_use]
pub fn issue_body(content: &str) -> &str {
    let mut offset = 0;
    let mut first = true;
    for line in content.split_inclusive('\n') {
        let text = line.trim();
        if first {
            first = false;
            if text != "---" {
                return content;
            }
            offset += line.len();
            continue;
        }
        if text == "---" {
            return &content[offset + line.len()..];
        }
        offset += line.len();
    }
    content
}

fn valid_gist_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
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
        let kind = if issue_number_of(&file_path).is_some() {
            DocKind::GithubIssue
        } else if lower.ends_with(".md") || lower.ends_with(".markdown") {
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
        let kind = if issue_number_of(&file_path).is_some() {
            DocKind::GithubIssue
        } else if lower.ends_with(".md") || lower.ends_with(".markdown") {
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
    if host.eq_ignore_ascii_case("gist.github.com") {
        // `gist.github.com/<owner>/<id>[/<revision>]`, or the bare `/<id>` form.
        let (owner, id, revision) = match segments.as_slice() {
            [id] => (None, *id, None),
            [owner, id] => (Some(*owner), *id, None),
            [owner, id, revision] => (Some(*owner), *id, Some(*revision)),
            _ => return None,
        };
        if !valid_gist_id(id) || owner.is_some_and(|o| !valid_name(o)) {
            return None;
        }
        return Some(mk(
            DocKind::GithubGist,
            format!("gist:{id}"),
            Locator {
                url: address.to_owned(),
                owner: owner.map(str::to_owned),
                repo: None,
                ref_name: revision.map(str::to_owned),
                path: id.to_owned(),
            },
            id.to_owned(),
        ));
    }
    if host.eq_ignore_ascii_case("gist.githubusercontent.com") {
        // `gist.githubusercontent.com/<owner>/<id>/raw[/<revision>/<file>…]`.
        let [owner, id, raw, ..] = segments.as_slice() else {
            return None;
        };
        if *raw != "raw" || !valid_name(owner) || !valid_gist_id(id) {
            return None;
        }
        return Some(mk(
            DocKind::GithubGist,
            format!("gist:{id}"),
            Locator {
                url: address.to_owned(),
                owner: Some((*owner).to_owned()),
                repo: None,
                ref_name: None,
                path: (*id).to_owned(),
            },
            (*id).to_owned(),
        ));
    }
    if host.eq_ignore_ascii_case("docs.google.com") {
        // A reserved kind, not yet recognised: Google Docs joins the kind set
        // when its fetcher does, so no permanently unfetchable rows appear.
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

/// Canonicalise a document key: what [`recognize_url`] would mint, read back.
///
/// The inverse of key minting, and deliberately no more permissive: the key's
/// parts are put back into the address form `recognize_url` accepts (a GitHub
/// blob address; a paste address under each configured `paste_prefixes`), and
/// the key is admitted only when that recognition mints the very same key.
/// So an unparseable key, a key of a kind this deployment does not recognise
/// (a paste with no paste source configured), and a caller-supplied address
/// wearing a key's clothes are all refused by the one check. Used only by the
/// write-scope fetch admission ([`Index::register_key`]); never a fetch of an
/// arbitrary caller address.
#[must_use]
pub fn parse_key(key: &str, paste_prefixes: &[String]) -> Option<Recognized> {
    if key.len() > MAX_ADDRESS
        || key
            .bytes()
            .any(|b| b == b'\\' || b == b' ' || b.is_ascii_control())
    {
        return None;
    }
    if let Some(rest) = key.strip_prefix("github:") {
        let (owner, rest) = rest.split_once('/')?;
        let (repo, rest) = rest.split_once('@')?;
        let (r#ref, path) = rest.split_once(':')?;
        if owner.is_empty()
            || repo.is_empty()
            || r#ref.is_empty()
            || path.is_empty()
            || r#ref.contains('/')
        {
            return None;
        }
        let url = format!("https://github.com/{owner}/{repo}/blob/{ref}/{path}");
        let rec = recognize_url(&url, None, paste_prefixes)?;
        return (rec.key == key).then_some(rec);
    }
    if let Some(id) = key.strip_prefix("gist:") {
        if !valid_gist_id(id) {
            return None;
        }
        let rec = recognize_url(
            &format!("https://gist.github.com/{id}"),
            None,
            paste_prefixes,
        )?;
        return (rec.key == key).then_some(rec);
    }
    if let Some(id) = key.strip_prefix("paste:") {
        if id.is_empty() {
            return None;
        }
        for prefix in paste_prefixes {
            if let Some(rec) = recognize_url(&format!("{prefix}{id}"), None, paste_prefixes) {
                if rec.key == key {
                    return Some(rec);
                }
            }
        }
        return None;
    }
    None
}

/// `text` with fenced and inline code removed: code is never a document link.
fn without_code(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut fenced = false;
    for line in text.split_inclusive('\n') {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if !fenced {
            out.push_str(line);
        }
    }
    let mut cleaned = String::with_capacity(out.len());
    let mut inline = false;
    for ch in out.chars() {
        if ch == '`' {
            inline = !inline;
        } else if !inline {
            cleaned.push(ch);
        }
    }
    cleaned
}

/// Every document link in `text`: Markdown links, Slack `<url|label>` links and
/// bare addresses, deduplicated by key (first label wins).
#[must_use]
pub fn recognize(text: &str, paste_prefixes: &[String]) -> Vec<Recognized> {
    let owned = without_code(text);
    let text = owned.as_str();
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
                // A label is one bracketed span: it cannot itself hold brackets or a
                // line break. Without this, a message that opens with a bracketed
                // word (an author tag, a citation) lends the whole message prefix
                // to the document it later links.
                if label.contains(['[', ']', '\n', '\r']) {
                    i += 1;
                    continue;
                }
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
        let word = word.trim_end_matches([')', ']', '}', '.', ',', ';', ':']);
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
        let mut fold = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| format!("documents index {}: {e}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Fold::default(),
            Err(e) => return Err(format!("documents index {}: {e}", path.display())),
        };
        // Rows folded before `#1176 doc-kind-buckets` hold issue paths as ordinary
        // markdown: re-classify them on read, with no migration.
        for entry in fold.docs.values_mut() {
            if entry.kind == DocKind::GithubMarkdown
                && issue_number_of(&entry.locator.path).is_some()
            {
                entry.kind = DocKind::GithubIssue;
            }
        }
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
        // A caller ahead of this fold (a restart lost it) must resync whole.
        let truncated = revision > fold.revision
            || fold
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
        // Called only when a mutation changed the fold. Written beside the
        // target and renamed over it, so a crash never leaves half a fold.
        if let Some(path) = &self.persist_path {
            if let Ok(text) = serde_json::to_string(fold) {
                let tmp = path.with_extension("json.tmp");
                if std::fs::write(&tmp, text).is_ok() {
                    let _ = std::fs::rename(&tmp, path);
                }
            }
        }
    }

    /// Record that `key`'s fetched state changed (fetch, refresh, eviction):
    /// a sync-visible change at a new revision, with the fold itself unchanged.
    pub fn touch(&self, key: &str) {
        let mut fold = self.fold.write().unwrap_or_else(|p| p.into_inner());
        if fold.docs.contains_key(key) {
            Self::bump(&mut fold, &[key.to_owned()]);
            self.persist(&fold);
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
            entry.label = entry.mentions.iter().find_map(|m| m.label.clone());
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
        if !affected.is_empty() {
            self.persist(&fold);
        }
    }

    fn remove_slot(&self, fold: &mut Fold, slot: &str) {
        if let Some(keys) = fold.by_message.remove(slot) {
            for key in keys {
                let empty = fold.docs.get_mut(&key).is_some_and(|entry| {
                    entry
                        .mentions
                        .retain(|m| message_slot(&m.channel_id, &m.message_id) != slot);
                    // The label names the row until fetch: it is the first
                    // surviving mention's label, never a deleted one's.
                    entry.label = entry.mentions.iter().find_map(|m| m.label.clone());
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

    /// Admit a document by canonical key alone (`#1081
    /// documents-relative-links`): register a mention-less entry for `key`
    /// when [`parse_key`] canonicalises it under this index's configuration,
    /// and return the entry; `None` — and nothing indexed — otherwise. An
    /// already-known key returns its entry unchanged.
    ///
    /// A mention-less entry lists with no mentions (mention order puts it with
    /// the oldest, so it never displaces a mentioned document) and takes part
    /// in sync like any other change. It is in no message's slot, so folding
    /// messages out never removes it; if a message later mentions it, it is an
    /// ordinary entry from then on.
    pub fn register_key(&self, key: &str) -> Option<Entry> {
        if let Some(entry) = self.get(key) {
            return Some(entry);
        }
        let rec = parse_key(key, &self.paste_prefixes)?;
        let mut fold = self.fold.write().unwrap_or_else(|p| p.into_inner());
        if !fold.docs.contains_key(&rec.key) {
            fold.docs.insert(
                rec.key.clone(),
                Entry {
                    key: rec.key.clone(),
                    kind: rec.kind,
                    locator: rec.locator.clone(),
                    file_name: rec.file_name.clone(),
                    label: None,
                    mentions: Vec::new(),
                },
            );
            Self::bump(&mut fold, std::slice::from_ref(&rec.key));
            self.persist(&fold);
        }
        fold.docs.get(&rec.key).cloned()
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Where the document lives now, when a moved fetch found it elsewhere.
    pub moved_to: Option<String>,
    /// Front-matter status, for issues (`#1176 doc-kind-buckets`); else `None`.
    pub issue_status: Option<String>,
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
    /// The source did not answer within the fetch deadline, so the server
    /// stopped waiting (`#2241 vibe-talk-document-load-502-readable`): a
    /// stalled upstream ends as this server's own answer naming it, never as
    /// a request held open until whatever fronts the deployment gives up
    /// and answers its own error page.
    #[error("fetch timed out: {0}")]
    TimedOut(String),
}

/// How long one document fetch or freshness check may take before the
/// server stops waiting and answers `document_fetch_timeout`
/// (`#2241 vibe-talk-document-load-502-readable`). Under the common
/// 60-second proxy timeout (and a tunnel's 100 seconds), so the page hears
/// this server's reason rather than a front end's error page.
pub const FETCH_DEADLINE: std::time::Duration = std::time::Duration::from_secs(45);

/// How long the GitHub client may take to open a connection. A host that
/// never accepts fails by itself, inside [`FETCH_DEADLINE`].
const GITHUB_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// A deadline in words: whole seconds, or milliseconds below one.
fn deadline_words(deadline: std::time::Duration) -> String {
    if deadline.as_secs() >= 1 {
        format!("{} s", deadline.as_secs())
    } else {
        format!("{} ms", deadline.as_millis())
    }
}

/// `host[:port]` of a base address, for naming the upstream in a failure.
fn authority_of(base: &str) -> Option<String> {
    let parsed = url::Url::parse(base).ok()?;
    let host = parsed.host_str()?;
    Some(match parsed.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    })
}

/// SHA-256 of `content`, hex: the version a fetched document is cached under.
#[must_use]
pub fn version_of(content: &str) -> String {
    use ring::digest::{digest, SHA256};
    let hash = digest(&SHA256, content.as_bytes());
    hash.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// The title and outline of a Markdown document.
///
/// Fence-aware, like [`headings`]: a `#` line inside a fenced code block is
/// code, not a heading, so it names no outline entry and no section. (This
/// function used to scan raw lines; [`section_of`] and the outline now agree
/// by construction because both read [`headings`].)
#[must_use]
pub fn title_and_outline(content: &str, fallback: &str) -> (String, Vec<String>) {
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
    let mut outline = Vec::new();
    for heading in headings(content) {
        if heading.level <= 2 {
            if heading.level == 1 && title.is_none() {
                title = Some(heading.text.clone());
            }
            outline.push(heading.text);
        }
    }
    (title.unwrap_or_else(|| fallback.to_owned()), outline)
}

/// One ATX heading outside any fenced code block. `#1040 documents-voice-agent`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Heading {
    /// Heading level: 1 (`#`) to 6 (`######`).
    pub level: usize,
    /// The heading text, trimmed: what an outline entry carries.
    pub text: String,
    /// Byte offset of the heading line's first byte in the source.
    pub start: usize,
}

/// The ATX heading a line is, as `(level, text)`, or `None`.
///
/// Column 0 only — the same shape the outline has always recognised — with
/// 1 to 6 `#` then a space, a tab, or the end of the line.
fn atx_heading(line: &str) -> Option<(usize, &str)> {
    let level = line.bytes().take_while(|b| *b == b'#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &line[level..];
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    Some((level, rest.trim()))
}

/// Every ATX heading in `content`, in order, skipping fenced code blocks.
///
/// A fence is what [`without_code`] takes it to be: a line whose first
/// non-space characters are ` ``` ` toggles the fenced state.
#[must_use]
pub fn headings(content: &str) -> Vec<Heading> {
    let mut found = Vec::new();
    let mut fenced = false;
    let mut offset = 0;
    for line in content.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        if let Some((level, text)) = atx_heading(line.trim_end_matches(['\n', '\r'])) {
            found.push(Heading {
                level,
                text: text.to_owned(),
                start,
            });
        }
    }
    found
}

/// One heading's section of a document: from its heading line to the next
/// heading of its level or higher (a smaller level number), or to the end of
/// the document. Deeper subsections are part of the section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Section<'a> {
    /// The section's source, trailing blank lines removed.
    pub text: &'a str,
    /// Byte offset of the heading line in the source.
    pub start: usize,
    /// The heading's level.
    pub level: usize,
}

/// The section headed exactly `heading` (case-sensitive; first match wins),
/// or `None` when no heading outside a code fence carries that text.
#[must_use]
pub fn section_of<'a>(content: &'a str, heading: &str) -> Option<Section<'a>> {
    let all = headings(content);
    let (index, own) = all.iter().enumerate().find(|(_, h)| h.text == heading)?;
    let end = all[index + 1..]
        .iter()
        .find(|h| h.level <= own.level)
        .map_or(content.len(), |h| h.start);
    Some(Section {
        text: content[own.start..end].trim_end_matches(['\n', '\r']),
        start: own.start,
        level: own.level,
    })
}

/// How many words `text` holds, counting whitespace-separated runs.
#[must_use]
pub fn word_count(text: &str) -> usize {
    text.split_whitespace().count()
}

/// One source document, as a fetcher returns it before titling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchedRaw {
    /// Raw Markdown source.
    pub content: String,
    /// The source's own version (an ETag or blob id), when it gives one.
    pub version: Option<String>,
    /// Where the document lives now, when it had moved from the linked path.
    pub moved_to: Option<String>,
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
    /// The GitHub helper's standing as last checked, without running it
    /// (`#1998`); only the GitHub fetcher has a helper to report on.
    fn github_helper_report(&self) -> HelperReport {
        HelperReport::not_configured()
    }
    /// Check the GitHub helper now (`#1998`); only the GitHub fetcher runs one.
    async fn check_github_helper(&self) -> HelperReport {
        self.github_helper_report()
    }
    /// The upstream this fetcher would ask for `entry`, in words, for a
    /// failure that must name it (`#2241`); `None` where it asks none or
    /// does not serve the kind.
    fn upstream(&self, _entry: &Entry) -> Option<String> {
        None
    }
}

/// How the GitHub login helper stands (`#1998 vibe-talk-github-helper-health-fallback`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelperStatus {
    /// No helper is configured (no command, or a token file instead).
    NotConfigured,
    /// The helper (or its fallback) returned a token.
    Ok,
    /// The configured helper does not exist and no fallback worked.
    Missing,
    /// The helper exists but did not return a token.
    Failing,
}

impl HelperStatus {
    /// The word `/healthz` reports for this standing.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            HelperStatus::NotConfigured => "not_configured",
            HelperStatus::Ok => "ok",
            HelperStatus::Missing => "missing",
            HelperStatus::Failing => "failing",
        }
    }
}

/// The standing of the GitHub login helper, as `/healthz` and the ops check
/// report it. It holds no token: a check runs the helper, drops the token it
/// printed at once, and keeps only this outcome (`#1998`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelperReport {
    /// How the helper stands.
    pub status: HelperStatus,
    /// The cause, in the `#1997` layer-named wording; never a token.
    pub detail: String,
    /// The fallback helper in use, as a path only, when one is.
    pub fallback: Option<String>,
}

impl HelperReport {
    /// The standing when no helper command is configured.
    #[must_use]
    pub fn not_configured() -> Self {
        Self {
            status: HelperStatus::NotConfigured,
            detail: "no GitHub helper is configured".to_owned(),
            fallback: None,
        }
    }

    /// The `using fallback <path>` line `/healthz` surfaces, when a fallback is in use.
    #[must_use]
    pub fn fallback_text(&self) -> Option<String> {
        self.fallback
            .as_ref()
            .map(|path| format!("using fallback {path}"))
    }
}

/// The fallback helpers tried, in order, when the configured helper path
/// does not exist (`#1998`): an empty candidate list turns fallbacks off.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelperFallbacks {
    /// Candidate helper programs, in the order they are tried.
    pub candidates: Vec<std::path::PathBuf>,
    /// Candidate paths that are never used, however they answer.
    pub denylist: Vec<String>,
}

impl HelperFallbacks {
    /// Fallbacks off: no candidate is ever tried.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            candidates: Vec::new(),
            denylist: Vec::new(),
        }
    }

    /// The fallbacks `[documents]` configuration names: its explicit list
    /// (an empty list turns fallbacks off), else the known installs.
    #[must_use]
    pub fn from_config(config: &crate::config::DocumentsConfig) -> Self {
        Self {
            candidates: match &config.github_token_helper_fallbacks {
                Some(list) => list.clone(),
                None => default_helper_candidates(),
            },
            denylist: config.github_token_helper_denylist.clone(),
        }
    }
}

/// The known helper installs, in order: the user-local install under the
/// home directory's `.local/opt/gh/usr/bin/gh`, then `gh` on `PATH` (`#1998`).
#[must_use]
pub fn default_helper_candidates() -> Vec<std::path::PathBuf> {
    let mut candidates = Vec::new();
    if let Some(home) = std::env::var_os("HOME").filter(|home| !home.is_empty()) {
        candidates.push(std::path::PathBuf::from(home).join(".local/opt/gh/usr/bin/gh"));
    }
    match find_on_path("gh") {
        Some(on_path) => {
            if !candidates.contains(&on_path) {
                candidates.push(on_path);
            }
        }
        None => candidates.push(std::path::PathBuf::from("gh")),
    }
    candidates
}

/// The first executable file named `name` on `PATH`, when one is there.
fn find_on_path(name: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let candidate = dir.join(name);
        is_executable_file(&candidate).then_some(candidate)
    })
}

/// Whether `program` can be found: a path is itself, a bare name is on `PATH`.
#[must_use]
pub fn helper_program_exists(program: &str) -> bool {
    if program.contains('/') || program.contains('\\') {
        std::path::Path::new(program).is_file()
    } else {
        find_on_path(program).is_some()
    }
}

fn is_executable_file(path: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::metadata(path)
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// The `--user` a helper argv asks for, as `--user NAME` or `--user=NAME`.
fn configured_user(argv: &[String]) -> Option<String> {
    let mut args = argv.iter();
    while let Some(arg) = args.next() {
        if arg == "--user" {
            return args.next().cloned();
        }
        if let Some(user) = arg.strip_prefix("--user=") {
            return Some(user.to_owned());
        }
    }
    None
}

/// Whether a script candidate forces a GitHub account other than
/// `configured`: it names a `--user` of its own that is not the configured
/// one (or any `--user` at all when none is configured). Only scripts are
/// read; a real helper binary is never scanned (`#1998`).
fn script_forces_other_user(bytes: &[u8], configured: Option<&str>) -> bool {
    if !bytes.starts_with(b"#!") {
        return false;
    }
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(8192)]);
    let mut rest: &str = &text;
    while let Some(at) = rest.find("--user") {
        let after = &rest[at + "--user".len()..];
        let value = if let Some(stripped) = after.strip_prefix('=') {
            stripped
        } else {
            after.trim_start_matches([' ', '\t'])
        };
        let value = value
            .trim_start_matches(['"', '\''])
            .split(|c: char| c.is_whitespace() || c == '"' || c == '\'')
            .next()
            .unwrap_or("");
        if !value.is_empty() && configured != Some(value) {
            return true;
        }
        rest = after;
    }
    false
}

/// Whether a fallback candidate may be tried at all: a real, executable
/// file, not denylisted, and not a wrapper forcing another account (`#1998`).
fn candidate_is_eligible(
    path: &std::path::Path,
    configured: Option<&str>,
    denylist: &[String],
) -> bool {
    if denylist
        .iter()
        .any(|denied| denied == &path.to_string_lossy())
    {
        return false;
    }
    if !is_executable_file(path) {
        return false;
    }
    match std::fs::read(path) {
        Ok(bytes) => !script_forces_other_user(&bytes, configured),
        Err(_) => false,
    }
}

fn failure_detail(error: &FetchError) -> String {
    match error {
        FetchError::Failed(detail) | FetchError::TimedOut(detail) => detail.clone(),
        FetchError::Unsupported(kind) => format!("no fetcher for {kind} documents yet"),
    }
}

fn report_for_failure(argv: &[String], detail: String) -> HelperReport {
    let missing = argv
        .first()
        .is_some_and(|program| !helper_program_exists(program));
    HelperReport {
        status: if missing {
            HelperStatus::Missing
        } else {
            HelperStatus::Failing
        },
        detail,
        fallback: None,
    }
}

/// Probe one helper argv and drop the token it prints: only the outcome is
/// kept, and the token is never logged, stored, or returned (`#1998`).
async fn probe_helper(argv: &[String]) -> Result<(), FetchError> {
    run_token_command(argv).await.map(|_token| ())
}

/// Resolve the helper argv to use: the configured one when it answers, else
/// the first eligible fallback that returns a token for the configured
/// `--user` when the configured path does not exist (`#1998`).
async fn resolve_helper(
    argv: &[String],
    fallbacks: &HelperFallbacks,
) -> (Vec<String>, HelperReport) {
    let Some(program) = argv.first() else {
        return (argv.to_vec(), HelperReport::not_configured());
    };
    match probe_helper(argv).await {
        Ok(()) => (
            argv.to_vec(),
            HelperReport {
                status: HelperStatus::Ok,
                detail: "the configured GitHub helper is working".to_owned(),
                fallback: None,
            },
        ),
        Err(error) => {
            let detail = failure_detail(&error);
            if helper_program_exists(program) {
                return (
                    argv.to_vec(),
                    HelperReport {
                        status: HelperStatus::Failing,
                        detail,
                        fallback: None,
                    },
                );
            }
            let user = configured_user(argv);
            for candidate in &fallbacks.candidates {
                let candidate_argv: Vec<String> =
                    std::iter::once(candidate.to_string_lossy().into_owned())
                        .chain(argv[1..].iter().cloned())
                        .collect();
                if candidate_argv[0] == *program {
                    continue;
                }
                if !candidate_is_eligible(candidate, user.as_deref(), &fallbacks.denylist) {
                    continue;
                }
                if probe_helper(&candidate_argv).await.is_ok() {
                    let fallback = candidate_argv[0].clone();
                    return (
                        candidate_argv,
                        HelperReport {
                            status: HelperStatus::Ok,
                            detail: format!(
                                "the configured helper {program} is missing; using fallback {fallback}"
                            ),
                            fallback: Some(fallback),
                        },
                    );
                }
            }
            (
                argv.to_vec(),
                HelperReport {
                    status: HelperStatus::Missing,
                    detail,
                    fallback: None,
                },
            )
        }
    }
}

fn store_resolution(
    health: &std::sync::Mutex<HelperReport>,
    resolved: &std::sync::Mutex<Option<Vec<String>>>,
    switch_logged: &std::sync::Mutex<bool>,
    effective: Vec<String>,
    report: HelperReport,
) {
    if let Some(fallback) = &report.fallback {
        let mut logged = switch_logged.lock().unwrap_or_else(|p| p.into_inner());
        if !*logged {
            // Path only, once: never the token the fallback returned.
            tracing::info!(fallback = %fallback, "vibe-talk github helper: using fallback helper");
            *logged = true;
        }
    }
    *resolved.lock().unwrap_or_else(|p| p.into_inner()) = Some(effective);
    *health.lock().unwrap_or_else(|p| p.into_inner()) = report;
}

/// Where the GitHub fetcher's bearer token comes from.
pub enum TokenSource {
    /// No token.
    None,
    /// A token read once (from `github_token_file`).
    Static(String),
    /// A command printing the token (`github_token_command`), cached in memory.
    Command {
        /// The argv, run without a shell.
        argv: Vec<String>,
        /// The cached token and when it was fetched.
        cache: std::sync::Mutex<Option<(String, std::time::Instant)>>,
        /// Fallback helpers tried when the configured path does not exist.
        fallbacks: std::sync::Arc<HelperFallbacks>,
        /// The helper's standing, from the last check or fetch.
        health: std::sync::Arc<std::sync::Mutex<HelperReport>>,
        /// The argv that last resolved (the configured one or a fallback).
        resolved: std::sync::Mutex<Option<Vec<String>>>,
        /// Whether the fallback switch was already logged (once, path only).
        switch_logged: std::sync::Mutex<bool>,
    },
}

impl TokenSource {
    /// A token command with the default fallback helpers.
    #[must_use]
    pub fn command(argv: Vec<String>) -> Self {
        Self::command_with_fallbacks(
            argv,
            HelperFallbacks {
                candidates: default_helper_candidates(),
                denylist: Vec::new(),
            },
        )
    }

    /// A token command with named fallback helpers (possibly none).
    #[must_use]
    pub fn command_with_fallbacks(argv: Vec<String>, fallbacks: HelperFallbacks) -> Self {
        TokenSource::Command {
            argv,
            cache: std::sync::Mutex::new(None),
            fallbacks: std::sync::Arc::new(fallbacks),
            health: std::sync::Arc::new(std::sync::Mutex::new(HelperReport::not_configured())),
            resolved: std::sync::Mutex::new(None),
            switch_logged: std::sync::Mutex::new(false),
        }
    }

    /// The helper's standing as last checked, without running it.
    #[must_use]
    pub fn github_helper_report(&self) -> HelperReport {
        match self {
            TokenSource::None => HelperReport::not_configured(),
            TokenSource::Static(_) => HelperReport {
                status: HelperStatus::NotConfigured,
                detail: "a GitHub token file is in use; no helper is configured".to_owned(),
                fallback: None,
            },
            TokenSource::Command { health, .. } => {
                health.lock().unwrap_or_else(|p| p.into_inner()).clone()
            }
        }
    }

    /// Check the helper now (startup and heartbeat, `#1998`): probe it (and
    /// any fallback), drop every token obtained, keep only the report.
    pub async fn check_github_helper(&self) -> HelperReport {
        match self {
            TokenSource::None | TokenSource::Static(_) => self.github_helper_report(),
            TokenSource::Command {
                argv,
                fallbacks,
                health,
                resolved,
                switch_logged,
                ..
            } => {
                let (effective, report) = resolve_helper(argv, fallbacks).await;
                store_resolution(health, resolved, switch_logged, effective, report.clone());
                report
            }
        }
    }
}

/// How often the helper heartbeat runs (`#1998`).
pub const GITHUB_HELPER_HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(600);

/// How long a command token is reused before the command runs again.
const TOKEN_COMMAND_TTL: std::time::Duration = std::time::Duration::from_secs(600);
/// How long a token command may run.
const TOKEN_COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// The layer every helper failure is named at (`#1997 vibe-talk-github-login-error-text`):
/// the reader and the document row show this sentence verbatim, so it must
/// name vibe-talk and the real cause, and never carry the token or more
/// than the first stderr line.
const HELPER_ERROR_PREFIX: &str = "vibe-talk could not get your GitHub login:";

/// Run a token command and return its trimmed stdout. The token is never logged;
/// a failure names only the first line of its stderr.
async fn run_token_command(argv: &[String]) -> Result<String, FetchError> {
    let Some((program, args)) = argv.split_first() else {
        return Err(FetchError::Failed(format!(
            "{HELPER_ERROR_PREFIX} no helper is configured"
        )));
    };
    let output = tokio::time::timeout(
        TOKEN_COMMAND_TIMEOUT,
        tokio::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .output(),
    )
    .await
    .map_err(|_| FetchError::Failed(format!("{HELPER_ERROR_PREFIX} the helper timed out")))?
    .map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            FetchError::Failed(format!(
                "{HELPER_ERROR_PREFIX} the helper {program} is missing"
            ))
        } else {
            FetchError::Failed(format!(
                "{HELPER_ERROR_PREFIX} the helper {program} could not be run: {e}"
            ))
        }
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let first = stderr.lines().next().unwrap_or("").trim();
        return Err(FetchError::Failed(match output.status.code() {
            Some(code) if first.is_empty() => {
                format!("{HELPER_ERROR_PREFIX} the helper exited {code}")
            }
            Some(code) => {
                format!("{HELPER_ERROR_PREFIX} the helper exited {code}: {first}")
            }
            None if first.is_empty() => {
                format!("{HELPER_ERROR_PREFIX} the helper exited without an exit code")
            }
            None => {
                format!("{HELPER_ERROR_PREFIX} the helper exited without an exit code: {first}")
            }
        }));
    }
    let token = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if token.is_empty() {
        return Err(FetchError::Failed(format!(
            "{HELPER_ERROR_PREFIX} the helper printed no token"
        )));
    }
    Ok(token)
}

impl TokenSource {
    /// The bearer token, running (and caching) the command when configured.
    async fn token(&self) -> Result<Option<String>, FetchError> {
        match self {
            TokenSource::None => Ok(None),
            TokenSource::Static(token) => Ok(Some(token.clone())),
            TokenSource::Command {
                argv,
                cache,
                fallbacks,
                health,
                resolved,
                switch_logged,
            } => {
                if let Some((token, at)) = cache.lock().unwrap_or_else(|p| p.into_inner()).clone() {
                    if at.elapsed() < TOKEN_COMMAND_TTL {
                        return Ok(Some(token));
                    }
                }
                let resolved_so_far =
                    { resolved.lock().unwrap_or_else(|p| p.into_inner()).clone() };
                let effective = match resolved_so_far {
                    Some(effective) => effective,
                    None => {
                        let (effective, report) = resolve_helper(argv, fallbacks).await;
                        store_resolution(
                            health,
                            resolved,
                            switch_logged,
                            effective.clone(),
                            report,
                        );
                        effective
                    }
                };
                let token = match run_token_command(&effective).await {
                    Ok(token) => token,
                    Err(error) => {
                        // A helper that stopped answering is re-resolved on
                        // the next ask (and by the heartbeat, `#1998`).
                        *resolved.lock().unwrap_or_else(|p| p.into_inner()) = None;
                        *health.lock().unwrap_or_else(|p| p.into_inner()) =
                            report_for_failure(&effective, failure_detail(&error));
                        return Err(error);
                    }
                };
                *cache.lock().unwrap_or_else(|p| p.into_inner()) =
                    Some((token.clone(), std::time::Instant::now()));
                Ok(Some(token))
            }
        }
    }

    /// Forget a cached command token (after a 401/403, so it is re-run).
    fn invalidate(&self) {
        if let TokenSource::Command { cache, .. } = self {
            *cache.lock().unwrap_or_else(|p| p.into_inner()) = None;
        }
    }

    /// Whether this source can produce a fresh token after a refusal.
    fn is_command(&self) -> bool {
        matches!(self, TokenSource::Command { .. })
    }

    /// Whether any credential is configured at all. GitHub answers 404 for a
    /// private repository it will not show an unauthenticated caller, exactly
    /// as for a missing file, so this fact changes what a 404 may be said to
    /// mean (`#1980 vibe-talk-docs-private-github-load`).
    fn has_credential(&self) -> bool {
        !matches!(self, TokenSource::None)
    }
}

/// What GitHub answered when asked for one thread's title. `#1058 pr-name-enrichment`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IssueTitle {
    /// The thread's title as GitHub holds it, and whether it has the thread closed.
    Found {
        /// The title, unshortened: shortening it for a label is the caller's rule.
        title: String,
        /// Whether the thread is closed (a merged pull request reads closed here).
        closed: bool,
    },
    /// No such thread, or none the token may see: GitHub answers 404 to both.
    NotFound,
    /// GitHub asked for a pause (429, or a 403 carrying its rate-limit headers).
    RateLimited,
}

/// What a non-success GitHub answer means, in the words the reader is shown
/// (`#1980 vibe-talk-docs-private-github-load`): a rate limit is a pause, a
/// 401/403 is a refusal, and a 404 with no credential configured cannot be
/// told apart from a private repository GitHub will not show vibe-talk, so
/// it is never reported as a plain missing file. Any other status keeps
/// GitHub's own number.
fn github_status_failure(
    subject: &str,
    status: reqwest::StatusCode,
    headers: &reqwest::header::HeaderMap,
    has_credential: bool,
) -> FetchError {
    let code = status.as_u16();
    if code == 429 || (code == 403 && rate_limited(headers)) {
        FetchError::Failed(format!(
            "GitHub rate limit: GitHub answered {code} for {subject}; try again later"
        ))
    } else if code == 401 || code == 403 {
        FetchError::Failed(format!(
            "vibe-talk has no access to {subject}: GitHub answered {code}; \
             the repository may be private or vibe-talk's GitHub credential was refused"
        ))
    } else if code == 404 && !has_credential {
        FetchError::Failed(format!(
            "vibe-talk has no access to {subject}: GitHub answered 404 and no GitHub \
             credential is configured, so a private repository cannot be fetched (a \
             missing repository or file answers the same way)"
        ))
    } else if code == 404 {
        FetchError::Failed(format!(
            "vibe-talk has no access to {subject}, or it does not exist: GitHub answered 404; \
             a private repository vibe-talk's credential cannot read answers the same way"
        ))
    } else {
        FetchError::Failed(format!("GitHub answered {code} for {subject}"))
    }
}

/// Whether a 403's headers say it is a rate limit rather than a refusal: a `retry-after`, or
/// no requests left in the window.
fn rate_limited(headers: &reqwest::header::HeaderMap) -> bool {
    headers.contains_key(reqwest::header::RETRY_AFTER)
        || headers
            .get("x-ratelimit-remaining")
            .and_then(|value| value.to_str().ok())
            == Some("0")
}

/// The GitHub fetcher: raw content over HTTPS, with an optional token.
pub struct GithubFetcher {
    client: reqwest::Client,
    token: TokenSource,
    raw_base: String,
    api_base: String,
    trees: std::sync::Mutex<std::collections::HashMap<String, (std::time::Instant, Vec<String>)>>,
}

/// How long a repository tree listing is reused for moved-document lookups.
const TREE_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(600);

/// Whether `r#ref` names one commit (40 hex digits), not a branch.
fn is_commit_ref(r#ref: &str) -> bool {
    r#ref.len() == 40 && r#ref.bytes().all(|b| b.is_ascii_hexdigit())
}

impl GithubFetcher {
    /// A fetcher against `raw_base` (default `https://raw.githubusercontent.com`).
    /// `https_proxy`, when set, applies to this client only.
    #[must_use]
    pub fn new(token: TokenSource, raw_base: Option<String>, https_proxy: Option<String>) -> Self {
        let mut builder = reqwest::Client::builder().connect_timeout(GITHUB_CONNECT_TIMEOUT);
        if let Some(proxy) = https_proxy.as_deref().filter(|p| !p.trim().is_empty()) {
            if let Ok(proxy) = reqwest::Proxy::https(proxy) {
                builder = builder.proxy(proxy);
            }
        }
        Self {
            client: builder.build().unwrap_or_else(|_| reqwest::Client::new()),
            token,
            raw_base: raw_base
                .unwrap_or_else(|| "https://raw.githubusercontent.com".to_owned())
                .trim_end_matches('/')
                .to_owned(),
            api_base: "https://api.github.com".to_owned(),
            trees: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Fetch from an API host other than GitHub's (a fake in tests).
    #[must_use]
    pub fn with_api_base(mut self, api_base: String) -> Self {
        self.api_base = api_base.trim_end_matches('/').to_owned();
        self
    }

    /// Every blob path in `owner/repo` at `r#ref`, cached per repo/ref.
    async fn tree_paths(
        &self,
        owner: &str,
        repo: &str,
        r#ref: &str,
    ) -> Result<Vec<String>, FetchError> {
        let key = format!("{owner}/{repo}@{ref}");
        if let Some((at, paths)) = self
            .trees
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&key)
            .cloned()
        {
            if at.elapsed() < TREE_CACHE_TTL {
                return Ok(paths);
            }
        }
        let url = format!(
            "{}/repos/{owner}/{repo}/git/trees/{ref}?recursive=1",
            self.api_base
        );
        let mut response = self.send_get(&url).await?;
        if !response.status().is_success() {
            return Err(github_status_failure(
                &format!("{owner}/{repo}"),
                response.status(),
                response.headers(),
                self.token.has_credential(),
            ));
        }
        let mut bytes: Vec<u8> = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| FetchError::Failed(format!("could not reach GitHub: {e}")))?
        {
            if bytes.len() + chunk.len() > 8 * MAX_DOCUMENT_BYTES {
                return Err(FetchError::Failed("repository tree too large".to_owned()));
            }
            bytes.extend_from_slice(&chunk);
        }
        let parsed: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|e| FetchError::Failed(e.to_string()))?;
        let paths: Vec<String> = parsed
            .get("tree")
            .and_then(serde_json::Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter(|e| e.get("type").and_then(serde_json::Value::as_str) == Some("blob"))
                    .filter_map(|e| {
                        e.get("path")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_owned)
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.trees
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(key, (std::time::Instant::now(), paths.clone()));
        Ok(paths)
    }

    /// The one path in the tree with `entry`'s file name, when exactly one matches.
    /// A tree that GitHub will not show (a private repository, a refused
    /// credential, a rate limit, a network failure) is an error, not an
    /// absence: only a tree that answered may say the file is not in it
    /// (`#1980 vibe-talk-docs-private-github-load`).
    async fn moved_path(&self, entry: &Entry) -> Result<Option<String>, FetchError> {
        let (Some(owner), Some(repo), Some(r#ref)) = (
            entry.locator.owner.as_deref(),
            entry.locator.repo.as_deref(),
            entry.locator.ref_name.as_deref(),
        ) else {
            return Ok(None);
        };
        if is_commit_ref(r#ref) {
            return Ok(None);
        }
        let paths = self.tree_paths(owner, repo, r#ref).await?;
        let name = entry.file_name.as_str();
        let mut matches = paths
            .iter()
            .filter(|path| path.rsplit('/').next().is_some_and(|last| last == name))
            .cloned();
        let Some(first) = matches.next() else {
            return Ok(None);
        };
        if matches.next().is_some() {
            return Ok(None);
        }
        Ok(Some(first))
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

    /// One GET, with a fresh command token retried once after a 401/403.
    async fn send_get(&self, url: &str) -> Result<reqwest::Response, FetchError> {
        for attempt in 0..2 {
            let token = self.token.token().await?;
            let mut request = self.client.get(url).header("user-agent", "vibe-talk");
            if let Some(token) = &token {
                request = request.bearer_auth(token);
            }
            let response = request
                .send()
                .await
                .map_err(|e| FetchError::Failed(format!("could not reach GitHub: {e}")))?;
            let refused = matches!(response.status().as_u16(), 401 | 403);
            if refused && attempt == 0 && self.token.is_command() {
                self.token.invalidate();
                continue;
            }
            return Ok(response);
        }
        unreachable!("the retry loop always returns");
    }

    async fn read_body(
        &self,
        mut response: reqwest::Response,
    ) -> Result<(String, Option<String>), FetchError> {
        let etag = response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        // Streamed with the cap enforced as it arrives, never buffered whole.
        let mut bytes: Vec<u8> = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| FetchError::Failed(e.to_string()))?
        {
            if bytes.len() + chunk.len() > MAX_DOCUMENT_BYTES {
                return Err(FetchError::Failed("document over 1 MiB".to_owned()));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok((String::from_utf8_lossy(&bytes).into_owned(), etag))
    }

    /// The `owner/repo` a GitHub document names, for failure wording.
    fn repo_subject(entry: &Entry) -> String {
        match (
            entry.locator.owner.as_deref(),
            entry.locator.repo.as_deref(),
        ) {
            (Some(owner), Some(repo)) => format!("{owner}/{repo}"),
            _ => entry.locator.path.clone(),
        }
    }

    async fn get(&self, entry: &Entry) -> Result<FetchedRaw, FetchError> {
        let url = self.url(entry)?;
        let response = self.send_get(&url).await?;
        if response.status().as_u16() == 404 {
            // A commit names no tree to walk, so its 404 keeps GitHub's
            // meaning (private or missing) instead of a moved guess.
            if entry.locator.ref_name.as_deref().is_some_and(is_commit_ref) {
                return Err(github_status_failure(
                    &Self::repo_subject(entry),
                    response.status(),
                    response.headers(),
                    self.token.has_credential(),
                ));
            }
            // A moved document on a branch: find its file name in the tree and
            // fetch it from its new home; otherwise say where it was lost. A
            // tree GitHub will not show is the fetch's real failure, not an
            // absence (`#1980 vibe-talk-docs-private-github-load`).
            let Some(new_path) = self.moved_path(entry).await? else {
                return Err(FetchError::Failed(format!(
                    "not found at {} (moved or deleted)",
                    entry.locator.path
                )));
            };
            let mut moved_entry = entry.clone();
            moved_entry.locator.path = new_path.clone();
            let moved_url = self.url(&moved_entry)?;
            let moved_response = self.send_get(&moved_url).await?;
            if !moved_response.status().is_success() {
                return Err(FetchError::Failed(format!(
                    "not found at {} (moved or deleted)",
                    entry.locator.path
                )));
            }
            let (content, etag) = self.read_body(moved_response).await?;
            return Ok(FetchedRaw {
                version: etag.or_else(|| Some(version_of(&content))),
                content,
                moved_to: Some(new_path),
            });
        }
        if !response.status().is_success() {
            return Err(github_status_failure(
                &Self::repo_subject(entry),
                response.status(),
                response.headers(),
                self.token.has_credential(),
            ));
        }
        let (content, etag) = self.read_body(response).await?;
        Ok(FetchedRaw {
            version: etag.or_else(|| Some(version_of(&content))),
            content,
            moved_to: None,
        })
    }

    /// The title of one issue or pull request — the API's issues endpoint answers for both,
    /// their numbers being one sequence in a repository — and whether GitHub has it closed.
    /// `#1058 pr-name-enrichment`: the name a thread's label may carry. The one API read here
    /// besides the tree walk, and through the same token and proxy as the rest.
    ///
    /// `owner`, `repo` and `number` come from an address [`crate::link_labels`] has already
    /// read as a thread page, so each is made only of the characters that part may hold.
    pub async fn issue_title(
        &self,
        owner: &str,
        repo: &str,
        number: &str,
    ) -> Result<IssueTitle, FetchError> {
        let url = format!("{}/repos/{owner}/{repo}/issues/{number}", self.api_base);
        let response = self.send_get(&url).await?;
        let status = response.status().as_u16();
        if status == 404 {
            // Also what a thread the token may not see answers; the two are one answer to us.
            return Ok(IssueTitle::NotFound);
        }
        if status == 429 || (status == 403 && rate_limited(response.headers())) {
            return Ok(IssueTitle::RateLimited);
        }
        if !response.status().is_success() {
            return Err(FetchError::Failed(format!(
                "GitHub answered {}",
                response.status()
            )));
        }
        let (body, _) = self.read_body(response).await?;
        let parsed: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| FetchError::Failed(e.to_string()))?;
        let Some(title) = parsed.get("title").and_then(serde_json::Value::as_str) else {
            return Err(FetchError::Failed(
                "GitHub's answer held no title".to_owned(),
            ));
        };
        Ok(IssueTitle::Found {
            title: title.to_owned(),
            closed: parsed.get("state").and_then(serde_json::Value::as_str) == Some("closed"),
        })
    }

    /// Fetch one gist through the API (`#1176 doc-kind-buckets`): its files in
    /// name order, joined; a truncated file is read from its raw address.
    async fn get_gist(&self, entry: &Entry) -> Result<FetchedRaw, FetchError> {
        let url = format!("{}/gists/{}", self.api_base, entry.locator.path);
        let response = self.send_get(&url).await?;
        if !response.status().is_success() {
            return Err(github_status_failure(
                &format!("gist {}", entry.locator.path),
                response.status(),
                response.headers(),
                self.token.has_credential(),
            ));
        }
        let (body, _) = self.read_body(response).await?;
        let parsed: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| FetchError::Failed(e.to_string()))?;
        let Some(files) = parsed.get("files").and_then(serde_json::Value::as_object) else {
            return Err(FetchError::Failed(
                "GitHub's answer held no gist files".to_owned(),
            ));
        };
        let mut names: Vec<&String> = files.keys().collect();
        names.sort();
        let mut parts: Vec<String> = Vec::new();
        for name in names {
            let file = &files[name];
            let truncated =
                file.get("truncated").and_then(serde_json::Value::as_bool) == Some(true);
            let content = match file.get("content").and_then(serde_json::Value::as_str) {
                Some(text) if !truncated => text.to_owned(),
                _ => {
                    let raw_url = file
                        .get("raw_url")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| {
                            FetchError::Failed("gist file held no content".to_owned())
                        })?;
                    let raw = self.send_get(raw_url).await?;
                    if !raw.status().is_success() {
                        return Err(github_status_failure(
                            &format!("gist {}", entry.locator.path),
                            raw.status(),
                            raw.headers(),
                            self.token.has_credential(),
                        ));
                    }
                    self.read_body(raw).await?.0
                }
            };
            parts.push(content);
        }
        if parts.is_empty() {
            return Err(FetchError::Failed("the gist holds no files".to_owned()));
        }
        let content = if parts.len() == 1 {
            parts.pop().unwrap_or_default()
        } else {
            parts.join("\n\n")
        };
        Ok(FetchedRaw {
            version: Some(version_of(&content)),
            content,
            moved_to: None,
        })
    }

    async fn etag(&self, entry: &Entry) -> Result<Option<String>, FetchError> {
        let url = self.url(entry)?;
        let response = self.send_get(&url).await?;
        if !response.status().is_success() {
            return Err(github_status_failure(
                &Self::repo_subject(entry),
                response.status(),
                response.headers(),
                self.token.has_credential(),
            ));
        }
        // Headers only: the body is dropped unread, so a freshness check
        // costs a round trip, not a download.
        Ok(response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned))
    }
}

#[async_trait::async_trait]
impl Fetcher for GithubFetcher {
    async fn fetch(&self, entry: &Entry) -> Result<FetchedRaw, FetchError> {
        match entry.kind {
            DocKind::GithubMarkdown | DocKind::GithubFile | DocKind::GithubIssue => {
                self.get(entry).await
            }
            DocKind::GithubGist => self.get_gist(entry).await,
            kind => Err(FetchError::Unsupported(kind.as_str())),
        }
    }

    async fn current_version(&self, entry: &Entry) -> Result<Option<String>, FetchError> {
        match entry.kind {
            DocKind::GithubMarkdown | DocKind::GithubFile | DocKind::GithubIssue => {
                self.etag(entry).await
            }
            DocKind::GithubGist => Ok(self.get_gist(entry).await?.version),
            kind => Err(FetchError::Unsupported(kind.as_str())),
        }
    }

    fn github_helper_report(&self) -> HelperReport {
        self.token.github_helper_report()
    }

    async fn check_github_helper(&self) -> HelperReport {
        self.token.check_github_helper().await
    }

    fn upstream(&self, entry: &Entry) -> Option<String> {
        let base = match entry.kind {
            DocKind::GithubMarkdown | DocKind::GithubFile | DocKind::GithubIssue => &self.raw_base,
            DocKind::GithubGist => &self.api_base,
            _ => return None,
        };
        Some(match authority_of(base) {
            Some(host) => format!("GitHub ({host})"),
            None => "GitHub".to_owned(),
        })
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
        // Stdout is read with the cap enforced as it arrives, never whole.
        use tokio::io::AsyncReadExt as _;
        let mut child = tokio::process::Command::new(&argv[0])
            .args(&argv[1..])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| FetchError::Failed(e.to_string()))?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| FetchError::Failed("paste command gave no stdout".to_owned()))?;
        let mut bytes: Vec<u8> = Vec::new();
        let reading = async {
            let mut buf = [0u8; 8192];
            loop {
                let n = stdout
                    .read(&mut buf)
                    .await
                    .map_err(|e| FetchError::Failed(e.to_string()))?;
                if n == 0 {
                    break;
                }
                if bytes.len() + n > MAX_DOCUMENT_BYTES {
                    return Err(FetchError::Failed("document over 1 MiB".to_owned()));
                }
                bytes.extend_from_slice(&buf[..n]);
            }
            child
                .wait()
                .await
                .map_err(|e| FetchError::Failed(e.to_string()))
        };
        let status = tokio::time::timeout(std::time::Duration::from_secs(15), reading)
            .await
            .map_err(|_| FetchError::Failed("paste command timed out".to_owned()))??;
        if !status.success() {
            return Err(FetchError::Failed(format!(
                "paste command answered {status}"
            )));
        }
        let content = String::from_utf8_lossy(&bytes).into_owned();
        let version = Some(version_of(&content));
        Ok(FetchedRaw {
            content,
            version,
            moved_to: None,
        })
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

    fn github_helper_report(&self) -> HelperReport {
        self.fetchers
            .iter()
            .map(|fetcher| fetcher.github_helper_report())
            .find(|report| report.status != HelperStatus::NotConfigured)
            .unwrap_or_else(HelperReport::not_configured)
    }

    async fn check_github_helper(&self) -> HelperReport {
        for fetcher in &self.fetchers {
            let report = fetcher.check_github_helper().await;
            if report.status != HelperStatus::NotConfigured {
                return report;
            }
        }
        HelperReport::not_configured()
    }

    fn upstream(&self, entry: &Entry) -> Option<String> {
        self.fetchers
            .iter()
            .find_map(|fetcher| fetcher.upstream(entry))
    }
}

struct CacheState {
    docs: HashMap<String, Fetched>,
    order: VecDeque<String>,
    bytes: usize,
    max_entries: usize,
    max_bytes: usize,
}

impl CacheState {
    fn evict_to_ceilings(&mut self) -> Vec<String> {
        let mut evicted = Vec::new();
        while self.docs.len() > self.max_entries || self.bytes > self.max_bytes {
            let Some(old_key) = self.order.pop_front() else {
                break;
            };
            if let Some(old) = self.docs.remove(&old_key) {
                self.bytes = self.bytes.saturating_sub(old.content.len());
                evicted.push(old_key);
            }
        }
        evicted
    }

    fn insert(&mut self, fetched: Fetched) -> Vec<String> {
        let key = fetched.key.clone();
        if let Some(old) = self.docs.remove(&key) {
            self.bytes = self.bytes.saturating_sub(old.content.len());
            self.order.retain(|k| k != &key);
        }
        self.bytes += fetched.content.len();
        self.order.push_back(key);
        self.docs.insert(fetched.key.clone(), fetched);
        self.evict_to_ceilings()
    }

    /// This cache in persisted order, oldest first.
    fn persisted_docs(&self) -> Vec<Fetched> {
        self.order
            .iter()
            .filter_map(|key| self.docs.get(key).cloned())
            .collect()
    }

    /// Take in what a previous run persisted, under the same ceilings and
    /// the same identity and content version the cache holds. An entry
    /// whose stored SHA-256 version does not match its content, an
    /// oversized entry and an empty key are dropped, never trusted.
    /// Returns whether anything was dropped or evicted.
    fn restore(&mut self, persisted: PersistedFetched) -> bool {
        let mut dropped = false;
        for fetched in persisted.docs {
            if fetched.key.is_empty() || fetched.content.len() > MAX_DOCUMENT_BYTES {
                dropped = true;
                continue;
            }
            if is_sha256_hex(&fetched.version) && fetched.version != version_of(&fetched.content) {
                dropped = true;
                continue;
            }
            if !self.insert(fetched).is_empty() {
                dropped = true;
            }
        }
        dropped
    }
}

/// The persisted fetched cache: the bounded cache's copies in cache order.
/// `documents-fetched-persist`.
#[derive(Serialize, Deserialize)]
struct PersistedFetched {
    version: u32,
    docs: Vec<Fetched>,
}

/// The persisted fetched-cache file format this build reads and writes.
const FETCHED_PERSIST_VERSION: u32 = 1;

fn is_sha256_hex(version: &str) -> bool {
    version.len() == 64 && version.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Where fetched copies persist for a fold at `index_path`: beside it, as
/// `documents-fetched.json`, in the same data directory as the fold JSON.
#[must_use]
pub fn fetched_path_for_index(index_path: &std::path::Path) -> std::path::PathBuf {
    match index_path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.join("documents-fetched.json"),
        _ => std::path::PathBuf::from("documents-fetched.json"),
    }
}

/// Load the persisted fetched cache at `path` under the given ceilings.
/// A missing file starts empty, as a server without the file always has;
/// a corrupt, partial or foreign-version file is ignored with one warning
/// and also starts empty — it never fails startup.
fn load_persisted_cache(
    path: &std::path::Path,
    max_entries: usize,
    max_bytes: usize,
) -> (CacheState, bool) {
    let mut state = CacheState {
        docs: HashMap::new(),
        order: VecDeque::new(),
        bytes: 0,
        max_entries: max_entries.max(1),
        max_bytes: max_bytes.max(1),
    };
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return (state, false),
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "fetched documents not loaded; starting empty");
            return (state, false);
        }
    };
    match serde_json::from_str::<PersistedFetched>(&text) {
        Ok(persisted) if persisted.version == FETCHED_PERSIST_VERSION => {
            let trimmed = state.restore(persisted);
            (state, trimmed)
        }
        Ok(persisted) => {
            tracing::warn!(
                version = persisted.version,
                path = %path.display(),
                "fetched documents file is of a version this build does not read; starting empty"
            );
            (state, false)
        }
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "fetched documents not loaded; starting empty");
            (state, false)
        }
    }
}

/// Write `docs` to `path` atomically: beside the target, then renamed over
/// it, so a crash never leaves half a file. A failure is one warning; the
/// copies live on in memory.
fn write_persisted_cache(path: &std::path::Path, docs: Vec<Fetched>) {
    let persisted = PersistedFetched {
        version: FETCHED_PERSIST_VERSION,
        docs,
    };
    let written = serde_json::to_string(&persisted)
        .map_err(|e| e.to_string())
        .and_then(|text| {
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, text)
                .and_then(|()| std::fs::rename(&tmp, path))
                .map_err(|e| e.to_string())
        });
    if let Err(error) = written {
        tracing::warn!(%error, path = %path.display(), "fetched documents not written");
    }
}

/// The GitHub token `[documents]` configures, as every fetcher asking GitHub on this server
/// shares it: the command that prints one when it is set, else the file read once here, else
/// none. `#1058 pr-name-enrichment` asks through the same source.
#[must_use]
pub fn token_source_from_config(config: &crate::config::DocumentsConfig) -> TokenSource {
    if !config.github_token_command.is_empty() {
        TokenSource::command_with_fallbacks(
            config.github_token_command.clone(),
            HelperFallbacks::from_config(config),
        )
    } else {
        match config
            .github_token_file
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty())
        {
            Some(token) => TokenSource::Static(token),
            None => TokenSource::None,
        }
    }
}

/// Build the service from `[documents]` configuration: load the persisted
/// fold when a path is configured, and wire the GitHub and paste fetchers.
#[must_use]
pub fn service_from_config(config: &crate::config::DocumentsConfig) -> Arc<Service> {
    let index = match &config.index_path {
        Some(path) => Arc::new(
            match Index::load(config.paste_url_prefixes.clone(), path.clone()) {
                Ok(index) => index,
                Err(error) => {
                    // A fold that cannot be read is rebuilt from the cache at
                    // startup; say so rather than discard it silently.
                    tracing::warn!(%error, "documents index not loaded; rebuilding from the message cache");
                    Index::new(config.paste_url_prefixes.clone())
                }
            },
        ),
        None => Arc::new(Index::new(config.paste_url_prefixes.clone())),
    };
    let token = token_source_from_config(config);
    let mut fetchers: Vec<Arc<dyn Fetcher>> = vec![Arc::new(GithubFetcher::new(
        token,
        config.github_raw_base.clone(),
        config.https_proxy.clone(),
    ))];
    if !config.paste_command.is_empty() {
        fetchers.push(Arc::new(PasteFetcher::new(config.paste_command.clone())));
    }
    // Fetched copies persist beside the fold, in its data directory, so
    // the downloaded state the page shows survives a restart. No fold
    // path, no persistence: the cache is in memory only, as before.
    let fetched_path = config
        .index_path
        .as_ref()
        .map(|p| fetched_path_for_index(p));
    Arc::new(Service::with_persistence(
        index,
        Arc::new(CompositeFetcher::new(fetchers)),
        config.max_cached_documents,
        config.max_cached_bytes,
        fetched_path,
    ))
}

/// The service for a whole server configuration: the fold persists beside
/// the store unless `[documents] index_path` names another place.
#[must_use]
pub fn service_for(config: &crate::config::Config) -> Arc<Service> {
    let mut documents = config.documents.clone();
    if documents.index_path.is_none() {
        documents.index_path = config
            .storage
            .path
            .as_ref()
            .and_then(|p| p.parent().map(|d| d.join("documents-index.json")));
    }
    service_from_config(&documents)
}

/// Rebuild the fold from what the message cache already holds, at startup:
/// each listed channel's newest rows in both scopes, folded in. Startup work
/// only; the read path never scans. Returns how many messages were folded.
#[must_use]
pub fn rebuild_from_cache(
    index: &Index,
    cache: &crate::cache::MessageCache,
    chans: &[crate::cache::ChanKey],
) -> usize {
    let mut folded = 0;
    for chan in chans {
        for scope in [crate::cache::Scope::Main, crate::cache::Scope::Flat] {
            if let crate::cache::Read::Served(rows) = cache.newest(chan, &scope, 5_000) {
                for row in rows {
                    let m = &row.message;
                    index.apply_message(
                        m.channel_id.as_str(),
                        m.id.as_str(),
                        Some(row.created_ms),
                        &m.author,
                        m.reply_thread(),
                        &m.content,
                    );
                    folded += 1;
                }
            }
        }
    }
    folded
}

/// The documents service: the fold index plus the bounded fetched cache.
pub struct Service {
    /// The fold index.
    pub index: Arc<Index>,
    cache: Mutex<CacheState>,
    fetcher: Arc<dyn Fetcher>,
    persist_path: Option<std::path::PathBuf>,
    /// How long one fetch or freshness check may take ([`FETCH_DEADLINE`]
    /// unless a test sets a shorter one with [`Service::with_fetch_deadline`]).
    fetch_deadline: std::time::Duration,
}

impl Service {
    /// A service over `index`, fetching through `fetcher`, with cache bounds.
    /// Fetched copies live in memory only.
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
            persist_path: None,
            fetch_deadline: FETCH_DEADLINE,
        }
    }

    /// The same service with another fetch deadline: a test's stalled
    /// upstream ends in milliseconds instead of [`FETCH_DEADLINE`]
    /// (`#2241`).
    #[must_use]
    pub fn with_fetch_deadline(mut self, deadline: std::time::Duration) -> Self {
        self.fetch_deadline = deadline;
        self
    }

    /// Run one upstream call for `entry` under the fetch deadline; one that
    /// outlasts it is [`FetchError::TimedOut`], naming the upstream
    /// (`#2241 vibe-talk-document-load-502-readable`).
    async fn within_deadline<T>(
        &self,
        entry: &Entry,
        call: impl std::future::Future<Output = Result<T, FetchError>>,
    ) -> Result<T, FetchError> {
        match tokio::time::timeout(self.fetch_deadline, call).await {
            Ok(answer) => answer,
            Err(_) => {
                let who = self
                    .fetcher
                    .upstream(entry)
                    .unwrap_or_else(|| "the document's source".to_owned());
                let detail = format!(
                    "{who} did not answer within {}, so vibe-talk stopped waiting; try again later",
                    deadline_words(self.fetch_deadline)
                );
                tracing::warn!(key = %entry.key, %detail, "document fetch timed out");
                Err(FetchError::TimedOut(detail))
            }
        }
    }

    /// A service whose fetched copies persist at `persist_path` beside the
    /// fold: loaded on startup under the same ceilings, written through on
    /// every fetch and eviction, atomically. A missing file starts empty;
    /// a corrupt or partial file is ignored with a warning and also starts
    /// empty — it never fails startup. `None` persists nowhere.
    #[must_use]
    pub fn with_persistence(
        index: Arc<Index>,
        fetcher: Arc<dyn Fetcher>,
        max_entries: usize,
        max_bytes: usize,
        persist_path: Option<std::path::PathBuf>,
    ) -> Self {
        let (cache, trimmed) = match &persist_path {
            Some(path) => load_persisted_cache(path, max_entries, max_bytes),
            None => (
                CacheState {
                    docs: HashMap::new(),
                    order: VecDeque::new(),
                    bytes: 0,
                    max_entries: max_entries.max(1),
                    max_bytes: max_bytes.max(1),
                },
                false,
            ),
        };
        let service = Self {
            index,
            cache: Mutex::new(cache),
            fetcher,
            persist_path,
            fetch_deadline: FETCH_DEADLINE,
        };
        if trimmed {
            // What was dropped or evicted on load leaves the disk too, so
            // the file and the cache hold the same copies from the start.
            service.persist_cache();
        }
        service
    }

    fn persist_cache(&self) {
        let Some(path) = &self.persist_path else {
            return;
        };
        let docs = self
            .cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .persisted_docs();
        write_persisted_cache(path, docs);
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
        let key = fetched.key.clone();
        let evicted = {
            let mut cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
            cache.insert(fetched)
        };
        // Write through: the persisted file holds exactly the bounded
        // cache, evictions included, before the fetch is announced.
        self.persist_cache();
        // Fetched state is part of the synced record: say it changed.
        self.index.touch(&key);
        for old_key in evicted {
            if old_key != key {
                self.index.touch(&old_key);
            }
        }
    }

    /// The GitHub helper's standing as last checked, without running it (`#1998`).
    #[must_use]
    pub fn github_helper_report(&self) -> HelperReport {
        self.fetcher.github_helper_report()
    }

    /// Check the GitHub helper now: startup and the heartbeat call this
    /// (`#1998`). The token the helper prints is dropped inside the check;
    /// only the report is kept.
    pub async fn check_github_helper(&self) -> HelperReport {
        self.fetcher.check_github_helper().await
    }

    /// How many fetched copies the cache holds.
    #[must_use]
    pub fn cache_len(&self) -> usize {
        self.cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .docs
            .len()
    }

    /// Drop every fetched copy (a storage purge).
    pub fn clear_cache(&self) {
        {
            let mut cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
            cache.docs.clear();
            cache.order.clear();
            cache.bytes = 0;
        }
        self.persist_cache();
    }

    /// Fetch one document and cache it: the "sync this item" operation.
    /// Idempotent: a cached copy is returned as it stands; use
    /// [`Service::fetch_one_refresh`] to replace it with the newest version.
    ///
    /// # Errors
    ///
    /// [`FetchError`] when no fetcher serves its kind or the source fails.
    pub async fn fetch_one(&self, key: &str) -> Result<Fetched, FetchError> {
        // Content never outlives its last mention: an unindexed key has no
        // cached copy to return, whatever the cache still holds.
        if self.index.get(key).is_none() {
            return Err(FetchError::Failed(format!("unknown document {key}")));
        }
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
        let raw = self
            .within_deadline(&entry, self.fetcher.fetch(&entry))
            .await?;
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
            issue_status: (entry.kind == DocKind::GithubIssue)
                .then(|| issue_status_of(&raw.content))
                .flatten(),
            content: raw.content,
            moved_to: raw.moved_to,
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

    /// Every fetched copy the cache holds, in no promised order.
    #[must_use]
    pub fn cached_all(&self) -> Vec<Fetched> {
        self.cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .docs
            .values()
            .cloned()
            .collect()
    }

    /// Search the FETCHED documents only (`#1266 documents-search`): the
    /// shared message-search language ([`crate::search_query`]) matched over
    /// each cached copy's title plus body. Unfetched index entries and message
    /// text are never searched. `searched` says how many fetched copies were
    /// examined, so a bounded-cache eviction is visible, never silent.
    #[must_use]
    pub fn search_documents(&self, query: &crate::search_query::Query, limit: usize) -> DocSearch {
        let limit = limit.clamp(1, 50);
        let terms = query.positive_terms();
        let day_start = |day: crate::search_query::Day| day.start_ms_at_offset(0);
        let mut hits: Vec<DocSearchHit> = Vec::new();
        let all = self.cached_all();
        let searched = all.len();
        for doc in all {
            let haystack = format!("{}\n{}", doc.title, doc.content);
            let subject = crate::search_query::Subject::new(
                &haystack,
                "",
                "",
                doc.fetched_ms,
                false,
                doc.content.contains("http://") || doc.content.contains("https://"),
            );
            if !query.matches(&subject, &day_start) {
                continue;
            }
            let (snippet, snippet_start) = snippet_of(&doc.content, &terms);
            let folded_title = crate::search_query::fold(&doc.title);
            hits.push(DocSearchHit {
                title_match: terms.iter().any(|t| folded_title.contains(t.as_str())),
                key: doc.key,
                title: doc.title,
                kind: doc.kind,
                fetched_ms: doc.fetched_ms,
                snippet,
                snippet_start,
            });
        }
        hits.sort_by(|a, b| {
            b.fetched_ms
                .cmp(&a.fetched_ms)
                .then_with(|| a.key.cmp(&b.key))
        });
        hits.truncate(limit);
        DocSearch {
            hits,
            searched,
            terms,
        }
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
        let latest = self
            .within_deadline(&entry, self.fetcher.current_version(&entry))
            .await?;
        Ok(Freshness {
            key: key.to_owned(),
            fetched: true,
            update_available: latest.as_deref().is_some_and(|v| v != cached.version),
            version: Some(cached.version),
            latest_version: latest,
        })
    }
}

/// One hit of [`Service::search_documents`] (`#1266 documents-search`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DocSearchHit {
    /// Document key.
    pub key: String,
    /// Real title of the fetched copy.
    pub title: String,
    /// What kind of document it is.
    pub kind: DocKind,
    /// When the cached copy was fetched, in Unix milliseconds.
    pub fetched_ms: i64,
    /// Whether a matched term is in the title (else the body matched).
    pub title_match: bool,
    /// An excerpt of the body around the first matched term (the body's head
    /// when only the title matched).
    pub snippet: String,
    /// Byte offset of `snippet` in the body.
    pub snippet_start: usize,
}

/// The answer of [`Service::search_documents`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DocSearch {
    /// Matching fetched documents, most recently fetched first.
    pub hits: Vec<DocSearchHit>,
    /// How many fetched documents were searched.
    pub searched: usize,
    /// The query's positive terms, folded: what a reader marks.
    pub terms: Vec<String>,
}

/// The excerpt of `content` around the earliest of `terms`: `(text, start)`.
/// The body's head when no term is in the body (a title-only match).
fn snippet_of(content: &str, terms: &[String]) -> (String, usize) {
    let lower = content.to_lowercase();
    let mut best: Option<(usize, usize)> = None;
    for term in terms {
        if term.is_empty() {
            continue;
        }
        if let Some(at) = lower.find(term.as_str()) {
            if best.is_none_or(|(old, _)| at < old) {
                best = Some((at, term.len()));
            }
        }
    }
    let Some((at, _)) = best else {
        let end = content
            .char_indices()
            .nth(180)
            .map_or(content.len(), |(i, _)| i);
        return (content[..end].to_owned(), 0);
    };
    let mut start = at.saturating_sub(60);
    while start > 0 && !content.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (at + 140).min(content.len());
    while end < content.len() && !content.is_char_boundary(end) {
        end += 1;
    }
    (content[start..end].to_owned(), start)
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
    fn a_bracketed_prefix_is_not_the_link_label() {
        // A cached message that opens with a bracketed author tag and links a
        // document at its end: the document's label is the link's own, never
        // the whole message prefix.
        let rec = recognize(
            "[agent] *Done.* Write-up: [RESULTS.md](https://github.com/octo/widgets/blob/main/docs/RESULTS.md)",
            &prefixes(),
        );
        assert_eq!(rec.len(), 1);
        assert_eq!(rec[0].label.as_deref(), Some("RESULTS.md"));
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
    fn recognises_pastes_and_reserves_google_docs() {
        let rec = recognize(
            "<https://paste.example/Ab12|a paste> https://docs.google.com/document/d/DocId123/edit",
            &prefixes(),
        );
        // Google Docs is a reserved kind with no fetcher yet: no row appears.
        assert_eq!(rec.len(), 1);
        assert_eq!(rec[0].kind, DocKind::Paste);
        assert_eq!(rec[0].key, "paste:Ab12");
        assert_eq!(DocKind::GoogleDoc.as_str(), "google_doc");
    }

    #[test]
    fn recognises_minibeads_issues_not_ordinary_markdown() {
        for (url, number) in [
            (
                "https://github.com/octo/widgets/blob/main/.minibeads/issues/agent-utils-1176.md",
                1176,
            ),
            (
                "https://raw.githubusercontent.com/octo/widgets/main/.minibeads/issues/x-9.md",
                9,
            ),
        ] {
            let rec = recognize_url(url, None, &prefixes()).expect("issue recognised");
            assert_eq!(rec.kind, DocKind::GithubIssue, "{url}");
            assert_eq!(rec.kind.as_str(), "github_issue");
            assert_eq!(issue_number_of(&rec.locator.path), Some(number), "{url}");
        }
        // Near-misses stay ordinary markdown (or an ordinary file for non-.md).
        for url in [
            "https://github.com/octo/widgets/blob/main/.minibeads/other/agent-utils-1.md",
            "https://github.com/octo/widgets/blob/main/.minibeads/issues/no-number.md",
            "https://github.com/octo/widgets/blob/main/.minibeads/issues/agent-utils-.md",
            "https://github.com/octo/widgets/blob/main/docs/.minibeads/issues/a-1.md",
            "https://github.com/octo/widgets/blob/main/.minibeads/issues/sub/a-1.md",
        ] {
            let rec = recognize_url(url, None, &prefixes()).expect("still a document");
            assert_eq!(rec.kind, DocKind::GithubMarkdown, "{url}");
        }
        let txt = recognize_url(
            "https://github.com/octo/widgets/blob/main/.minibeads/issues/agent-utils-1.txt",
            None,
            &prefixes(),
        )
        .expect("still a document");
        assert_eq!(txt.kind, DocKind::GithubFile);
    }

    #[test]
    fn recognises_gist_url_forms() {
        for url in [
            "https://gist.github.com/octo/0123456789abcdef0123456789abcdef",
            "https://gist.github.com/0123456789abcdef0123456789abcdef",
            "https://gist.github.com/octo/0123456789abcdef0123456789abcdef/deadbeef",
            "https://gist.githubusercontent.com/octo/0123456789abcdef0123456789abcdef/raw/deadbeef/note.md",
        ] {
            let rec = recognize_url(url, None, &prefixes()).expect("gist recognised");
            assert_eq!(rec.kind, DocKind::GithubGist, "{url}");
            assert_eq!(rec.kind.as_str(), "github_gist");
            assert_eq!(rec.key, "gist:0123456789abcdef0123456789abcdef", "{url}");
        }
        assert!(recognize_url("https://gist.github.com/", None, &prefixes()).is_none());
    }

    #[test]
    fn kind_names_round_trip_through_serde() {
        for (kind, name) in [
            (DocKind::GithubMarkdown, "github_markdown"),
            (DocKind::GithubFile, "github_file"),
            (DocKind::Paste, "paste"),
            (DocKind::GoogleDoc, "google_doc"),
            (DocKind::GithubIssue, "github_issue"),
            (DocKind::GithubGist, "github_gist"),
        ] {
            assert_eq!(kind.as_str(), name);
            let json = serde_json::to_string(&kind).expect("serialise");
            assert_eq!(json, format!("\"{name}\""));
            assert_eq!(serde_json::from_str::<DocKind>(&json).expect("parse"), kind);
        }
    }

    #[test]
    fn issue_frontmatter_status_is_read() {
        let doc = "---\ntitle: \"Fix it\"\nstatus: in_progress\n---\n\n# Body\n";
        assert_eq!(issue_status_of(doc).as_deref(), Some("in_progress"));
        assert_eq!(issue_status_of("# No frontmatter\n").as_deref(), None);
        assert_eq!(
            issue_status_of("---\ntitle: x\n---\n").as_deref(),
            None,
            "no status line, no status"
        );
    }

    #[test]
    fn issue_body_drops_only_a_complete_front_matter_block() {
        // `#2004`: the speech cut is taken from this body, as the reader
        // draws it, so its block indices name the drawn blocks.
        let doc = "---\ntitle: \"Fix it\"\nstatus: open\n---\n\n# Body\n\nWords.\n";
        assert_eq!(issue_body(doc), "\n# Body\n\nWords.\n");
        assert_eq!(issue_body("# No frontmatter\n"), "# No frontmatter\n");
        let open = "---\ntitle: x\n# Body\n";
        assert_eq!(issue_body(open), open, "no closing fence, no split");
        assert_eq!(issue_body("---\n---\nBody.\n"), "Body.\n");
    }

    #[test]
    fn parse_key_round_trips_minted_keys() {
        // Every key recognize_url mints parses back to the same recognition.
        for url in [
            "https://github.com/octo/widgets/blob/main/docs/report.md",
            "https://github.com/octo/widgets/blob/main/data/x.json",
            "https://raw.githubusercontent.com/octo/widgets/main/docs/a.markdown",
            "https://github.com/o/r/blob/feature/x/docs/a.md",
            "https://github.com/o/r/blob/main/.minibeads/issues/agent-utils-1176.md",
            "https://gist.github.com/octo/0123456789abcdef0123456789abcdef",
            "https://paste.example/Ab12",
        ] {
            let minted = recognize_url(url, None, &prefixes()).expect("minted");
            let parsed = parse_key(&minted.key, &prefixes()).expect("parsed back");
            assert_eq!(parsed.key, minted.key, "{url}");
            assert_eq!(parsed.kind, minted.kind, "{url}");
            assert_eq!(parsed.locator.path, minted.locator.path, "{url}");
        }
        let md = parse_key("github:octo/widgets@main:docs/a.md", &prefixes()).expect("md");
        assert_eq!(md.kind, DocKind::GithubMarkdown);
        assert_eq!(md.locator.owner.as_deref(), Some("octo"));
        assert_eq!(md.locator.ref_name.as_deref(), Some("main"));
        let file = parse_key("github:octo/widgets@main:src/lib.rs", &prefixes()).expect("file");
        assert_eq!(file.kind, DocKind::GithubFile);
    }

    #[test]
    fn parse_key_refuses_what_recognition_would_not_mint() {
        for key in [
            // Not keys at all — including an address wearing a key's clothes:
            // admission is by canonical key, never a free-form fetch.
            "not a key",
            "https://github.com/octo/widgets/blob/main/docs/a.md",
            "example:thing",
            "google_doc:DocId123",
            // Broken GitHub shapes.
            "github:octo/widgets@main:",
            "github:octo/widgets@:docs/a.md",
            "github:octo/@main:docs/a.md",
            "github:/widgets@main:docs/a.md",
            "github:octo/widgets@main:docs//a.md",
            "github:octo/widgets@main:../escape.md",
            "github:octo/widgets@main:docs/%2e%2e/escape.md",
            "github:octo/widgets@main:docs/a.md#fragment",
            "github:octo/widgets@main:docs/a.md?plain=1",
            "github:octo/widgets@feat/ure:docs/a.md",
            // Broken paste shapes.
            "paste:",
            "paste:bad/id",
            "paste:bad id",
            "paste:-leading",
        ] {
            assert!(parse_key(key, &prefixes()).is_none(), "admitted {key}");
        }
        // A paste key with no paste source configured is an unconfigured kind.
        assert!(parse_key("paste:Ab12", &[]).is_none());
    }

    #[test]
    fn register_key_admits_mention_less_and_refuses_the_rest() {
        let index = Index::new(prefixes());
        let entry = index
            .register_key("github:octo/widgets@main:docs/linked.md")
            .expect("admitted");
        assert!(entry.mentions.is_empty());
        assert_eq!(index.len(), 1);
        // Re-registering is a no-op returning the same entry.
        let before = index.revision();
        assert!(index
            .register_key("github:octo/widgets@main:docs/linked.md")
            .is_some());
        assert_eq!(index.revision(), before);
        // What does not canonicalise is never indexed.
        assert!(index.register_key("github:octo/widgets@main:").is_none());
        assert!(index.register_key("https://example.com/x").is_none());
        assert_eq!(index.len(), 1);
        // A later mention lands on the admitted entry like any other.
        index.apply_message(
            "c",
            "m1",
            Some(7),
            "a",
            None,
            "https://github.com/octo/widgets/blob/main/docs/linked.md",
        );
        assert_eq!(
            index
                .get("github:octo/widgets@main:docs/linked.md")
                .expect("entry")
                .mentions
                .len(),
            1
        );
    }

    #[test]
    fn slash_branch_and_code_spans_and_long_messages() {
        // The first segment after blob/ is the ref as written; a branch with a
        // slash still keys and fetches as written (the fetcher reassembles it).
        let rec = recognize_url(
            "https://github.com/o/r/blob/feature/x/docs/a.md",
            None,
            &prefixes(),
        )
        .expect("recognised");
        assert_eq!(rec.locator.ref_name.as_deref(), Some("feature"));
        assert_eq!(rec.locator.path, "x/docs/a.md");
        // Code never becomes a document.
        assert!(
            recognize(
                "`https://github.com/o/r/blob/main/a.md`\n```\nhttps://github.com/o/r/blob/main/b.md\n```",
                &prefixes()
            )
            .is_empty()
        );
        // A link past 16 KiB of prose is still recognised from the source text.
        let mut text = "x".repeat(20_000);
        text.push_str(" https://github.com/o/r/blob/main/big.md");
        assert_eq!(recognize(&text, &prefixes()).len(), 1);
    }

    #[test]
    fn fold_persists_and_reloads() {
        let dir = std::env::temp_dir().join(format!("documents-index-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("index.json");
        let index = Index::load(prefixes(), path.clone()).expect("load");
        index.apply_message(
            "c",
            "m1",
            Some(1),
            "a",
            None,
            "https://github.com/o/r/blob/main/a.md",
        );
        let reloaded = Index::load(prefixes(), path.clone()).expect("reload");
        assert_eq!(reloaded.len(), 1);
        assert!(reloaded.revision() >= 1);
        let (changed, deleted, truncated, _) = reloaded.changes_since(0);
        assert_eq!(changed.len(), 1);
        assert!(deleted.is_empty() && !truncated);
        // A caller ahead of the fold must resync whole.
        let (_, _, truncated, _) = reloaded.changes_since(999);
        assert!(truncated);
        std::fs::remove_dir_all(&dir).ok();
    }

    fn persisted_fetched(key: &str, content: &str, version: &str) -> Fetched {
        Fetched {
            key: key.to_owned(),
            kind: DocKind::GithubMarkdown,
            title: "Title".to_owned(),
            content: content.to_owned(),
            version: version.to_owned(),
            fetched_ms: 1,
            outline: Vec::new(),
            moved_to: None,
            issue_status: None,
        }
    }

    #[test]
    fn restored_fetched_copies_keep_etags_and_drop_sha_mismatches() {
        let good_sha = version_of("good body");
        let dir =
            std::env::temp_dir().join(format!("documents-fetched-unit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("documents-fetched.json");
        write_persisted_cache(
            &path,
            vec![
                persisted_fetched("github:o/r@main:a.md", "good body", &good_sha),
                // An ETag is a source version, not a content hash: kept as-is.
                persisted_fetched("github:o/r@main:b.md", "etag body", "\"etag-1\""),
                // A SHA-256 version that does not hash its content is corrupt.
                persisted_fetched("github:o/r@main:c.md", "tampered", &good_sha),
                persisted_fetched("", "no key", &version_of("no key")),
            ],
        );
        assert!(
            !path.with_extension("json.tmp").exists(),
            "atomic write left a temp file"
        );
        let (cache, trimmed) = load_persisted_cache(&path, 100, 1 << 20);
        assert!(trimmed, "the corrupt entries were dropped on load");
        assert_eq!(cache.docs.len(), 2);
        assert!(cache.docs.contains_key("github:o/r@main:a.md"));
        assert_eq!(
            cache.docs["github:o/r@main:b.md"].version, "\"etag-1\"",
            "a source version survives verbatim"
        );
        // What was dropped leaves the disk too.
        let (reloaded, _) = load_persisted_cache(&path, 100, 1 << 20);
        assert_eq!(reloaded.docs.len(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn fetched_path_sits_beside_the_fold() {
        assert_eq!(
            fetched_path_for_index(std::path::Path::new("/data/documents-index.json")),
            std::path::PathBuf::from("/data/documents-fetched.json")
        );
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

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(future)
    }

    #[test]
    fn token_command_token_is_trimmed_and_cached() {
        let source = TokenSource::command_with_fallbacks(
            vec!["/bin/echo".to_owned(), "tok-123".to_owned()],
            HelperFallbacks::disabled(),
        );
        let first = block_on(source.token()).expect("token");
        let second = block_on(source.token()).expect("cached token");
        assert_eq!(first.as_deref(), Some("tok-123"));
        assert_eq!(second.as_deref(), Some("tok-123"));
        source.invalidate();
        let third = block_on(source.token()).expect("refreshed token");
        assert_eq!(third.as_deref(), Some("tok-123"));
    }

    #[test]
    fn token_command_failure_names_only_the_first_stderr_line() {
        let argv: Vec<String> = [
            "/bin/sh",
            "-c",
            "echo first-problem >&2; echo second >&2; exit 3",
        ]
        .iter()
        .map(ToString::to_string)
        .collect();
        let error = block_on(run_token_command(&argv)).expect_err("must fail");
        let FetchError::Failed(detail) = error else {
            panic!("wrong failure: {error}");
        };
        assert_eq!(
            detail,
            "vibe-talk could not get your GitHub login: the helper exited 3: first-problem"
        );
        assert!(!detail.contains("second"), "full stderr leaked: {detail}");
    }

    #[test]
    fn token_command_missing_helper_names_the_helper_at_the_vibe_talk_layer() {
        let argv = vec![
            "/definitely-missing-github-helper/gh".to_owned(),
            "auth".to_owned(),
            "token".to_owned(),
        ];
        let error = block_on(run_token_command(&argv)).expect_err("must fail");
        let FetchError::Failed(detail) = error else {
            panic!("wrong failure: {error}");
        };
        assert_eq!(
            detail,
            "vibe-talk could not get your GitHub login: the helper /definitely-missing-github-helper/gh is missing"
        );
    }

    /// Write a fake helper script (`#1998`): fixtures only, in a temp dir.
    #[cfg(unix)]
    fn fake_helper(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt as _;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write fake helper");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fake helper");
        path
    }

    #[cfg(unix)]
    fn helper_temp_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("documents-helper-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("create helper temp dir");
        dir
    }

    #[cfg(unix)]
    #[test]
    fn helper_check_reports_ok_missing_and_failing_without_keeping_a_token() {
        let dir = helper_temp_dir("check");
        let ok = fake_helper(&dir, "gh-ok", "echo fake-token-1998");
        let failing = fake_helper(&dir, "gh-failing", "echo not-logged-in >&2; exit 1");
        let empty = fake_helper(&dir, "gh-empty", "exit 0");
        let missing = dir.join("gh-gone");
        let cases: Vec<(Vec<String>, HelperStatus, &str)> = vec![
            (
                vec![
                    ok.to_string_lossy().into_owned(),
                    "auth".into(),
                    "token".into(),
                ],
                HelperStatus::Ok,
                "working",
            ),
            (
                vec![
                    missing.to_string_lossy().into_owned(),
                    "auth".into(),
                    "token".into(),
                ],
                HelperStatus::Missing,
                "is missing",
            ),
            (
                vec![
                    failing.to_string_lossy().into_owned(),
                    "auth".into(),
                    "token".into(),
                ],
                HelperStatus::Failing,
                "the helper exited 1: not-logged-in",
            ),
            (
                vec![
                    empty.to_string_lossy().into_owned(),
                    "auth".into(),
                    "token".into(),
                ],
                HelperStatus::Failing,
                "the helper printed no token",
            ),
        ];
        for (argv, status, detail_part) in cases {
            let source = TokenSource::command_with_fallbacks(argv, HelperFallbacks::disabled());
            let report = block_on(source.check_github_helper());
            assert_eq!(report.status, status, "{report:?}");
            assert!(
                report.detail.contains(detail_part),
                "detail: {}",
                report.detail
            );
            assert_eq!(source.github_helper_report(), report);
            assert!(
                !format!("{report:?}").contains("fake-token-1998"),
                "a check kept a token: {report:?}"
            );
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn helper_fallback_uses_the_first_working_candidate_and_surfaces_it() {
        let dir = helper_temp_dir("fallback");
        let failing = fake_helper(&dir, "gh-failing", "echo nope >&2; exit 1");
        let good = fake_helper(&dir, "gh-good", "echo fake-token-1998");
        let configured = vec![
            dir.join("gh-gone").to_string_lossy().into_owned(),
            "auth".to_owned(),
            "token".to_owned(),
            "--user".to_owned(),
            "placeholder-user".to_owned(),
        ];
        let source = TokenSource::command_with_fallbacks(
            configured,
            HelperFallbacks {
                candidates: vec![failing, good.clone()],
                denylist: Vec::new(),
            },
        );
        let report = block_on(source.check_github_helper());
        assert_eq!(report.status, HelperStatus::Ok, "{report:?}");
        assert_eq!(
            report.fallback.as_deref(),
            Some(good.to_string_lossy().as_ref())
        );
        assert_eq!(
            report.fallback_text(),
            Some(format!("using fallback {}", good.display()))
        );
        assert!(!report.detail.contains("fake-token-1998"), "{report:?}");
        // The fallback is the helper fetches actually use.
        let token = block_on(source.token()).expect("fallback token");
        assert_eq!(token.as_deref(), Some("fake-token-1998"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn helper_fallback_skips_denylisted_and_other_user_wrappers() {
        let dir = helper_temp_dir("skip");
        let wrapper = fake_helper(
            &dir,
            "gh-wrapper",
            "exec /bin/echo fake-token-1998 --user other-user",
        );
        let denied = fake_helper(&dir, "gh-denied", "echo fake-token-1998");
        let good = fake_helper(&dir, "gh-good", "echo fake-token-1998");
        let configured = vec![
            dir.join("gh-gone").to_string_lossy().into_owned(),
            "auth".to_owned(),
            "token".to_owned(),
            "--user".to_owned(),
            "placeholder-user".to_owned(),
        ];
        let source = TokenSource::command_with_fallbacks(
            configured,
            HelperFallbacks {
                candidates: vec![wrapper, denied.clone(), good.clone()],
                denylist: vec![denied.to_string_lossy().into_owned()],
            },
        );
        let report = block_on(source.check_github_helper());
        assert_eq!(report.status, HelperStatus::Ok, "{report:?}");
        assert_eq!(
            report.fallback.as_deref(),
            Some(good.to_string_lossy().as_ref()),
            "a wrapper for another account or a denylisted helper was used: {report:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn helper_fallback_off_keeps_a_missing_helper_missing() {
        let dir = helper_temp_dir("off");
        let configured = vec![
            dir.join("gh-gone").to_string_lossy().into_owned(),
            "auth".to_owned(),
            "token".to_owned(),
        ];
        let source = TokenSource::command_with_fallbacks(configured, HelperFallbacks::disabled());
        let report = block_on(source.check_github_helper());
        assert_eq!(report.status, HelperStatus::Missing, "{report:?}");
        assert_eq!(report.fallback, None);
        assert!(report.detail.contains("is missing"), "{}", report.detail);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn token_command_printing_no_token_is_named_at_the_vibe_talk_layer() {
        // `/bin/true` exits 0 and prints nothing.
        let argv = vec!["/bin/true".to_owned()];
        let error = block_on(run_token_command(&argv)).expect_err("must fail");
        let FetchError::Failed(detail) = error else {
            panic!("wrong failure: {error}");
        };
        assert_eq!(
            detail,
            "vibe-talk could not get your GitHub login: the helper printed no token"
        );
    }

    const SLICING_DOC: &str = "\
# Report

intro words here

## Methods

method body

### Detail

deep body

## Results

result body

# Appendix

appendix body
";

    #[test]
    fn a_section_stops_at_the_next_heading_of_its_level_or_higher() {
        let methods = section_of(SLICING_DOC, "Methods").expect("section");
        assert!(methods.text.starts_with("## Methods"), "{}", methods.text);
        assert!(methods.text.contains("method body"), "{}", methods.text);
        assert!(
            !methods.text.contains("result body"),
            "the next ## ends it: {}",
            methods.text
        );
        // A level-1 section runs until the next level-1 heading.
        let report = section_of(SLICING_DOC, "Report").expect("section");
        assert!(report.text.contains("result body"), "{}", report.text);
        assert!(
            !report.text.contains("appendix body"),
            "the next # ends it: {}",
            report.text
        );
        assert_eq!(report.level, 1);
        assert_eq!(methods.level, 2);
        assert_eq!(&SLICING_DOC[methods.start..methods.start + 2], "##");
    }

    #[test]
    fn a_section_includes_its_deeper_subsections() {
        let methods = section_of(SLICING_DOC, "Methods").expect("section");
        assert!(
            methods.text.contains("### Detail") && methods.text.contains("deep body"),
            "{}",
            methods.text
        );
    }

    #[test]
    fn the_last_section_runs_to_the_end_of_the_document() {
        let appendix = section_of(SLICING_DOC, "Appendix").expect("section");
        assert_eq!(appendix.text, "# Appendix\n\nappendix body");
    }

    #[test]
    fn a_heading_inside_a_code_fence_is_not_a_section_boundary() {
        let doc = "\
## Real

before

```markdown
## Fake

code, not a heading
```

after

## Next

next body
";
        let real = section_of(doc, "Real").expect("section");
        assert!(
            real.text.contains("## Fake"),
            "fenced text is kept: {}",
            real.text
        );
        assert!(real.text.contains("after"), "{}", real.text);
        assert!(
            !real.text.contains("next body"),
            "the real next ## still ends it: {}",
            real.text
        );
        assert!(
            section_of(doc, "Fake").is_none(),
            "a fenced heading heads no section"
        );
        // The outline agrees: a fenced heading is no outline entry either.
        let (_title, outline) = title_and_outline(doc, "fallback");
        assert_eq!(outline, ["Real", "Next"]);
    }

    #[test]
    fn a_missing_or_differently_cased_heading_has_no_section() {
        assert!(section_of(SLICING_DOC, "Conclusions").is_none());
        assert!(
            section_of(SLICING_DOC, "methods").is_none(),
            "heading match is case-sensitive"
        );
        assert!(section_of("", "Methods").is_none());
    }

    #[test]
    fn headings_and_word_count_read_the_whole_shape() {
        let all = headings(SLICING_DOC);
        let shape: Vec<(usize, &str)> = all.iter().map(|h| (h.level, h.text.as_str())).collect();
        assert_eq!(
            shape,
            [
                (1, "Report"),
                (2, "Methods"),
                (3, "Detail"),
                (2, "Results"),
                (1, "Appendix")
            ]
        );
        assert_eq!(word_count("one  two\nthree\tfour "), 4);
        assert_eq!(word_count(""), 0);
        assert_eq!(word_count(SLICING_DOC), 21);
    }

    /// Serve each of `responses` once, in order, on a loopback port, and answer its address:
    /// the whole of what an `issue_title` test needs of the GitHub API.
    fn serve_once_each(responses: Vec<String>) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = format!("http://{}", listener.local_addr().expect("address"));
        std::thread::spawn(move || {
            for response in responses {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let mut request = [0_u8; 4096];
                let _ = stream.read(&mut request);
                let _ = stream.write_all(response.as_bytes());
            }
        });
        address
    }

    fn http(status: &str, headers: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n{headers}content-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    #[test]
    fn an_old_markdown_row_for_an_issue_path_reclassifies_on_load() {
        let dir = std::env::temp_dir().join(format!("documents-reclassify-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("index.json");
        let key = "github:o/r@main:.minibeads/issues/agent-utils-5.md";
        let json = format!(
            r#"{{"docs":{{"{key}":{{"key":"{key}","kind":"github_markdown","locator":{{"url":"https://github.com/o/r/blob/main/.minibeads/issues/agent-utils-5.md","owner":"o","repo":"r","ref_name":"main","path":".minibeads/issues/agent-utils-5.md"}},"file_name":"agent-utils-5.md","label":null,"mentions":[]}}}},"by_message":{{}},"revision":1,"changes":[]}}"#
        );
        std::fs::write(&path, json).expect("write fold");
        let index = Index::load(prefixes(), path).expect("load");
        assert_eq!(
            index.get(key).expect("entry").kind,
            DocKind::GithubIssue,
            "an old markdown row for an issue path reads as an issue"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_gist_fetches_its_files_through_the_api() {
        let body = r#"{"files":{"b.md":{"filename":"b.md","content":"second file","truncated":false},"a.md":{"filename":"a.md","content":"first file","truncated":false}}}"#;
        let api = serve_once_each(vec![http("200 OK", "", body)]);
        let fetcher = GithubFetcher::new(TokenSource::None, None, None).with_api_base(api);
        let entry = parse_key("gist:0123456789abcdef0123456789abcdef", &prefixes())
            .map(|rec| Entry {
                key: rec.key,
                kind: rec.kind,
                locator: rec.locator,
                file_name: rec.file_name,
                label: None,
                mentions: Vec::new(),
            })
            .expect("gist entry");
        let raw = block_on(fetcher.fetch(&entry)).expect("gist fetched");
        assert_eq!(raw.content, "first file\n\nsecond file");
        assert!(raw.version.is_some());
    }

    #[test]
    fn an_issue_title_is_read_from_the_issues_endpoint() {
        let api = serve_once_each(vec![
            http(
                "200 OK",
                "",
                r#"{"title":"Repair the sprocket","state":"closed","pull_request":{}}"#,
            ),
            http("404 Not Found", "", r#"{"message":"Not Found"}"#),
            http(
                "403 Forbidden",
                "retry-after: 60\r\nx-ratelimit-remaining: 0\r\n",
                r#"{"message":"API rate limit exceeded"}"#,
            ),
            http("403 Forbidden", "", r#"{"message":"Forbidden"}"#),
            http("200 OK", "", r#"{"state":"open"}"#),
        ]);
        let fetcher = GithubFetcher::new(TokenSource::Static("tok".to_owned()), None, None)
            .with_api_base(api);
        assert_eq!(
            block_on(fetcher.issue_title("octo", "widgets", "41")).expect("title"),
            IssueTitle::Found {
                title: "Repair the sprocket".to_owned(),
                closed: true,
            }
        );
        assert_eq!(
            block_on(fetcher.issue_title("octo", "widgets", "42")).expect("missing"),
            IssueTitle::NotFound
        );
        assert_eq!(
            block_on(fetcher.issue_title("octo", "widgets", "43")).expect("limited"),
            IssueTitle::RateLimited
        );
        // A plain refusal is a failure, not a pause and not an absence.
        assert!(block_on(fetcher.issue_title("octo", "widgets", "44")).is_err());
        // And an answer with no title in it is a failure, never an empty name.
        assert!(block_on(fetcher.issue_title("octo", "widgets", "45")).is_err());
    }
}
