//! Short references in message text: `:thread:NNNN`, `:email:NNNN`, `:issue:NNNN`.
//! `#1565 vibe-talk-short-ref-links`, from the design note
//! `ai_docs/plans/2026-11-15-thread-and-message-short-refs.md` (owner 093).
//!
//! The chat bridge assigns every thread a reply-alias number and every admitted
//! message a short number, and publishes them in `short-refs.json`
//! (`agentctl-short-refs/v1`) in its own state directory. This server only reads
//! that file: the paths are configuration (`[render] short_refs_files`), never
//! hard-coded, the file is re-read when its mtime changes, and a missing or
//! stale file simply resolves nothing — a reference then stays plain text,
//! never a wrong link. No network on either side.
//!
//! Resolution happens at display time in [`crate::render`]: a resolved
//! `:thread:` becomes a thread link and a resolved `:email:` a message link,
//! both carried by the same `data-msg` marking as `#543 in-app-message-links`
//! (plus a `data-shortref` kind for the page's short-ref view rules); a
//! resolved `:issue:` links through the configured `issue_url_template`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, RwLock};
use std::time::SystemTime;

/// The schema the bridge writes; anything else is not read.
pub const SCHEMA: &str = "agentctl-short-refs/v1";

/// One thread entry, as the bridge writes it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
struct ThreadEntry {
    n: u64,
    channel: String,
    root_ts: String,
    root_message: String,
}

/// One message entry, as the bridge writes it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
struct MessageEntry {
    n: u64,
    channel: String,
    message: String,
    #[serde(default)]
    thread_root_ts: Option<String>,
}

/// The file's shape.
#[derive(Clone, Debug, serde::Deserialize)]
struct ShortRefsFile {
    schema: String,
    #[serde(default)]
    threads: Vec<ThreadEntry>,
    #[serde(default)]
    messages: Vec<MessageEntry>,
}

/// What a `:thread:NNNN` or `:email:NNNN` resolves to, in this server's ids.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    /// The channel, as this server names it.
    pub channel: String,
    /// The message's id here; `None` for a thread link.
    pub message: Option<String>,
    /// The thread's root `ts`; `None` for a message on the channel's timeline.
    pub thread: Option<String>,
}

/// The resolved references from every configured file, merged. A later file
/// wins on a duplicate number only within its own kind; numbers are only
/// meaningful beside the bridge that assigned them, and deployments name the
/// files of the bridges whose channels this server reads.
#[derive(Clone, Debug, Default)]
pub struct ShortRefs {
    threads: HashMap<u64, ThreadEntry>,
    messages: HashMap<u64, MessageEntry>,
}

/// A provider message id as this server names it: a Slack `ts` becomes its
/// snowflake-shaped id; anything else is taken verbatim.
fn server_message_id(provider_id: &str) -> String {
    crate::slack::ids::message_id_from_ts(provider_id)
        .map(|id| id.as_str().to_owned())
        .unwrap_or_else(|| provider_id.to_owned())
}

impl ShortRefs {
    /// Parse one `short-refs.json` body; `None` when it is not the v1 schema.
    #[must_use]
    pub fn parse(body: &str) -> Option<Self> {
        let file: ShortRefsFile = serde_json::from_str(body).ok()?;
        if file.schema != SCHEMA {
            return None;
        }
        Some(Self {
            threads: file.threads.into_iter().map(|entry| (entry.n, entry)).collect(),
            messages: file.messages.into_iter().map(|entry| (entry.n, entry)).collect(),
        })
    }

    /// Merge `other` into this, its entries winning on duplicate numbers.
    fn merge(&mut self, other: Self) {
        self.threads.extend(other.threads);
        self.messages.extend(other.messages);
    }

    /// The thread `n` names, drilled into at its root.
    #[must_use]
    pub fn thread(&self, n: u64) -> Option<Resolved> {
        let entry = self.threads.get(&n)?;
        Some(Resolved {
            channel: entry.channel.clone(),
            message: None,
            thread: Some(entry.root_ts.clone()),
        })
    }

    /// The message `n` names, in Main; a reply also names its thread's root.
    #[must_use]
    pub fn message(&self, n: u64) -> Option<Resolved> {
        let entry = self.messages.get(&n)?;
        let thread = entry
            .thread_root_ts
            .clone()
            .filter(|root| root != &entry.message);
        Some(Resolved {
            channel: entry.channel.clone(),
            message: Some(server_message_id(&entry.message)),
            thread,
        })
    }

    /// Whether nothing is resolved (no files, or none readable).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.threads.is_empty() && self.messages.is_empty()
    }
}

/// The configured reader: file paths plus the issue template, and the cache.
struct Reader {
    files: Vec<PathBuf>,
    issue_url_template: Option<String>,
    stamps: Vec<Option<(SystemTime, u64)>>,
    refs: Arc<ShortRefs>,
}

static READER: LazyLock<RwLock<Reader>> = LazyLock::new(|| {
    RwLock::new(Reader {
        files: Vec::new(),
        issue_url_template: None,
        stamps: Vec::new(),
        refs: Arc::new(ShortRefs::default()),
    })
});

/// Configure the reader once at startup: the `short-refs.json` paths and the
/// `issue_url_template` (`{n}` where the number goes), as `[render]` names them.
pub fn set_short_refs(files: Vec<PathBuf>, issue_url_template: Option<String>) {
    let mut reader = READER.write().unwrap_or_else(std::sync::PoisonError::into_inner);
    reader.files = files;
    reader.issue_url_template = issue_url_template;
    reader.stamps = Vec::new();
    reader.refs = Arc::new(ShortRefs::default());
}

/// The configured issue address for `n`, or `None` when no usable template is
/// configured. The template was checked at load: exactly one `{n}`, https.
#[must_use]
pub fn issue_url(n: u64) -> Option<String> {
    let reader = READER.read().unwrap_or_else(std::sync::PoisonError::into_inner);
    reader
        .issue_url_template
        .as_ref()
        .map(|template| template.replace("{n}", &n.to_string()))
}

/// Whether an `issue_url_template` can be used: an https address holding
/// exactly one `{n}` and no other brace. `None` when it can.
#[must_use]
pub fn issue_template_problem(template: &str) -> Option<String> {
    if !template.starts_with("https://") || template.len() <= "https://".len() {
        return Some("issue_url_template must be an https URL".to_owned());
    }
    if template.matches("{n}").count() != 1
        || template.matches('{').count() != 1
        || template.matches('}').count() != 1
    {
        return Some("issue_url_template must hold exactly one {n} placeholder and no other brace".to_owned());
    }
    None
}

/// The current references, re-reading any configured file whose mtime or size
/// changed since it was last read. A file that vanishes or stops parsing
/// resolves nothing from itself, never a wrong link.
#[must_use]
pub fn short_refs() -> Arc<ShortRefs> {
    #[cfg(test)]
    if let Some(refs) = TEST_REFS.with(|cell| cell.borrow().clone()) {
        return refs;
    }
    let mut reader = READER.write().unwrap_or_else(std::sync::PoisonError::into_inner);
    let stamps: Vec<Option<(SystemTime, u64)>> = reader
        .files
        .iter()
        .map(|path| {
            std::fs::metadata(path)
                .ok()
                .and_then(|meta| meta.modified().ok().map(|mtime| (mtime, meta.len())))
        })
        .collect();
    if stamps != reader.stamps {
        let mut merged = ShortRefs::default();
        for path in &reader.files {
            if let Ok(body) = std::fs::read_to_string(path) {
                if let Some(parsed) = ShortRefs::parse(&body) {
                    merged.merge(parsed);
                }
            }
        }
        reader.refs = Arc::new(merged);
        reader.stamps = stamps;
    }
    Arc::clone(&reader.refs)
}

/// What names the current references in a render version, so a body or index
/// drawn under other references is drawn again: a stable hash of each
/// configured file's path, mtime and size, and of the issue template.
#[must_use]
pub fn fingerprint() -> Option<String> {
    #[cfg(test)]
    if TEST_REFS.with(|cell| cell.borrow().is_some()) {
        return Some("test".to_owned());
    }
    let reader = READER.read().unwrap_or_else(std::sync::PoisonError::into_inner);
    if reader.files.is_empty() && reader.issue_url_template.is_none() {
        return None;
    }
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    };
    for path in &reader.files {
        mix(path.to_string_lossy().as_bytes());
        mix(&[0]);
        let stamp = std::fs::metadata(path)
            .ok()
            .and_then(|meta| meta.modified().ok().map(|mtime| (mtime, meta.len())));
        match stamp {
            Some((mtime, len)) => {
                let secs = mtime
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                mix(&secs.to_le_bytes());
                mix(&len.to_le_bytes());
            }
            None => mix(b"missing"),
        }
        mix(&[0]);
    }
    if let Some(template) = &reader.issue_url_template {
        mix(template.as_bytes());
    }
    Some(format!("{hash:016x}"))
}

#[cfg(test)]
thread_local! {
    /// A test's own references, on its own thread.
    static TEST_REFS: std::cell::RefCell<Option<Arc<ShortRefs>>> = const { std::cell::RefCell::new(None) };
    /// A test's own issue template, on its own thread.
    static TEST_ISSUE: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// A test's own references and issue template until the guard is dropped.
#[cfg(test)]
pub struct ShortRefsGuard(Option<Arc<ShortRefs>>, Option<String>);

#[cfg(test)]
impl ShortRefsGuard {
    /// Install `refs` and the issue template on this thread until the guard drops.
    pub fn set(refs: ShortRefs, issue_url_template: Option<String>) -> Self {
        Self(
            TEST_REFS.with(|cell| cell.replace(Some(Arc::new(refs)))),
            TEST_ISSUE.with(|cell| cell.replace(issue_url_template)),
        )
    }
}

#[cfg(test)]
impl Drop for ShortRefsGuard {
    fn drop(&mut self) {
        TEST_REFS.with(|cell| *cell.borrow_mut() = self.0.take());
        TEST_ISSUE.with(|cell| *cell.borrow_mut() = self.1.take());
    }
}

/// The issue address under a test's own template, when one is set.
#[cfg(test)]
fn test_issue_url(n: u64) -> Option<String> {
    TEST_ISSUE.with(|cell| {
        cell.borrow()
            .as_ref()
            .map(|template| template.replace("{n}", &n.to_string()))
    })
}

/// The issue address for `n` under test configuration, else the configured one.
#[must_use]
pub fn issue_url_configured(n: u64) -> Option<String> {
    #[cfg(test)]
    if TEST_ISSUE.with(|cell| cell.borrow().is_some()) {
        return test_issue_url(n);
    }
    issue_url(n)
}

#[cfg(test)]
mod tests {
    use super::{issue_template_problem, ShortRefs};

    const FIXTURE: &str = r#"{"schema":"agentctl-short-refs/v1","threads":[{"n":60,"channel":"C0123ABCD","root_ts":"1700000000.000100","root_message":"1700000000.000100"},{"n":61,"channel":"C0123ABCD","root_ts":"1700000100.000200","root_message":"1700000100.000200"}],"messages":[{"n":1846,"channel":"C0123ABCD","message":"1700000000.000100","thread_root_ts":"1700000000.000100"},{"n":1847,"channel":"C0123ABCD","message":"1700000050.000300","thread_root_ts":"1700000000.000100"},{"n":1848,"channel":"C0123ABCD","message":"1700000100.000200","thread_root_ts":"1700000100.000200"}]}"#;

    #[test]
    fn the_fixture_resolves_threads_and_messages() {
        let refs = ShortRefs::parse(FIXTURE).expect("the fixture parses");
        let thread = refs.thread(60).expect("thread 60");
        assert_eq!(thread.channel, "C0123ABCD");
        assert_eq!(thread.message, None);
        assert_eq!(thread.thread.as_deref(), Some("1700000000.000100"));
        // A root message names no thread over itself; a reply names its root.
        let root = refs.message(1846).expect("message 1846");
        assert_eq!(root.thread, None);
        assert!(root.message.is_some_and(|id| !id.is_empty() && id != "1700000000.000100"));
        let reply = refs.message(1847).expect("message 1847");
        assert_eq!(reply.thread.as_deref(), Some("1700000000.000100"));
        assert_eq!(refs.thread(999), None);
        assert_eq!(refs.message(999), None);
    }

    #[test]
    fn other_schemas_and_bad_json_resolve_nothing() {
        assert!(ShortRefs::parse(r#"{"schema":"other/v9","threads":[],"messages":[]}"#).is_none());
        assert!(ShortRefs::parse("not json").is_none());
    }

    #[test]
    fn issue_templates_are_checked() {
        assert_eq!(issue_template_problem("https://tracker.example.invalid/issues/{n}"), None);
        assert!(issue_template_problem("http://tracker.example.invalid/{n}").is_some());
        assert!(issue_template_problem("https://tracker.example.invalid/issues/").is_some());
        assert!(issue_template_problem("https://tracker.example.invalid/{n}/{n}").is_some());
    }
}
