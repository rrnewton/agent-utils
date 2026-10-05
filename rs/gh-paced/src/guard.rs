//! Content guard for WRITE requests: refuse oversized bodies and base64-looking payloads.
//!
//! GitHub issue, comment and pull-request text is for short human-readable notes. This guard
//! refuses a write whose body sources (inline flags, body files, `gh api` fields, `--input`
//! request files, and stdin) total more than `max_body_bytes`, or contain a base64-looking run
//! longer than `max_base64_run` characters. `GH_PACED_ALLOW_LARGE_BODY=1` skips it.

use crate::classify::Classification;
use crate::config::Config;
use std::io::Read;

/// How a body source supplies its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceKind {
    /// The flag value itself is the text.
    Inline(String),
    /// The text is in this file.
    File(String),
    /// The text arrives on stdin.
    Stdin,
}

/// One place a write request takes body text from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodySource {
    /// The flag (or `<file>` for a positional file) that names this source.
    pub flag: String,
    /// Where the content comes from.
    pub kind: SourceKind,
    /// The content is a JSON request body (`gh api --input`, `workflow run --json`).
    pub json: bool,
}

impl BodySource {
    /// True when this source reads stdin.
    pub fn is_stdin(&self) -> bool {
        self.kind == SourceKind::Stdin
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Inline text, or a path when the value is a file flag.
    Text,
    /// A path, `-` = stdin.
    File,
    /// `key=value`, value inline.
    RawField,
    /// `key=value`, value `@path`, `@-` (stdin) or inline.
    TypedField,
    /// A JSON request body file, `-` = stdin.
    Input,
    /// A boolean flag meaning "the JSON body is on stdin".
    JsonStdin,
}

struct Table {
    long: &'static [(&'static str, Kind)],
    short: &'static [(char, Kind)],
    value_long: &'static [&'static str],
    value_short: &'static str,
    positional_files: bool,
    stdin_when_no_body: bool,
}

const API: Table = Table {
    long: &[
        ("raw-field", Kind::RawField),
        ("field", Kind::TypedField),
        ("input", Kind::Input),
    ],
    short: &[('f', Kind::RawField), ('F', Kind::TypedField)],
    value_long: &[
        "method", "header", "jq", "template", "preview", "cache", "hostname",
    ],
    value_short: "XHqtp",
    positional_files: false,
    stdin_when_no_body: false,
};

const WORKFLOW_RUN: Table = Table {
    long: &[
        ("raw-field", Kind::RawField),
        ("field", Kind::TypedField),
        ("json", Kind::JsonStdin),
    ],
    short: &[('f', Kind::RawField), ('F', Kind::TypedField)],
    value_long: &["ref", "repo"],
    value_short: "rR",
    positional_files: false,
    stdin_when_no_body: false,
};

const CREATE: Table = Table {
    long: &[
        ("title", Kind::Text),
        ("body", Kind::Text),
        ("body-file", Kind::File),
    ],
    short: &[('t', Kind::Text), ('b', Kind::Text), ('F', Kind::File)],
    value_long: &[
        "base",
        "head",
        "assignee",
        "label",
        "milestone",
        "project",
        "reviewer",
        "repo",
        "template",
        "recover",
    ],
    value_short: "BHalmprRT",
    positional_files: false,
    stdin_when_no_body: false,
};

const COMMENT: Table = Table {
    long: &[("body", Kind::Text), ("body-file", Kind::File)],
    short: &[('b', Kind::Text), ('F', Kind::File)],
    value_long: &["repo"],
    value_short: "R",
    positional_files: false,
    stdin_when_no_body: false,
};

const EDIT: Table = Table {
    long: &[
        ("title", Kind::Text),
        ("body", Kind::Text),
        ("body-file", Kind::File),
    ],
    short: &[('t', Kind::Text), ('b', Kind::Text), ('F', Kind::File)],
    value_long: &[
        "base",
        "milestone",
        "add-assignee",
        "remove-assignee",
        "add-label",
        "remove-label",
        "add-project",
        "remove-project",
        "add-reviewer",
        "remove-reviewer",
        "repo",
    ],
    value_short: "BmR",
    positional_files: false,
    stdin_when_no_body: false,
};

const MERGE: Table = Table {
    long: &[
        ("body", Kind::Text),
        ("body-file", Kind::File),
        ("subject", Kind::Text),
    ],
    short: &[('b', Kind::Text), ('F', Kind::File), ('t', Kind::Text)],
    value_long: &["author-email", "match-head-commit", "repo"],
    value_short: "AR",
    positional_files: false,
    stdin_when_no_body: false,
};

const CLOSE: Table = Table {
    long: &[("comment", Kind::Text)],
    short: &[('c', Kind::Text)],
    value_long: &["reason", "repo"],
    value_short: "rR",
    positional_files: false,
    stdin_when_no_body: false,
};

const RELEASE: Table = Table {
    long: &[
        ("notes", Kind::Text),
        ("notes-file", Kind::File),
        ("title", Kind::Text),
    ],
    short: &[('n', Kind::Text), ('F', Kind::File), ('t', Kind::Text)],
    value_long: &[
        "target",
        "discussion-category",
        "notes-start-tag",
        "tag",
        "repo",
    ],
    value_short: "R",
    positional_files: false,
    stdin_when_no_body: false,
};

const GIST_CREATE: Table = Table {
    long: &[("desc", Kind::Text)],
    short: &[('d', Kind::Text)],
    value_long: &["filename"],
    value_short: "f",
    positional_files: true,
    stdin_when_no_body: false,
};

const GIST_EDIT: Table = Table {
    long: &[("desc", Kind::Text), ("add", Kind::File)],
    short: &[('d', Kind::Text), ('a', Kind::File)],
    value_long: &["filename", "remove"],
    value_short: "fr",
    positional_files: false,
    stdin_when_no_body: false,
};

const LABEL: Table = Table {
    long: &[("description", Kind::Text), ("name", Kind::Text)],
    short: &[('d', Kind::Text), ('n', Kind::Text)],
    value_long: &["color", "repo"],
    value_short: "cR",
    positional_files: false,
    stdin_when_no_body: false,
};

const REPO: Table = Table {
    long: &[("description", Kind::Text)],
    short: &[('d', Kind::Text)],
    value_long: &[
        "homepage",
        "team",
        "template",
        "gitignore",
        "license",
        "source",
        "remote",
    ],
    value_short: "hgltpsr",
    positional_files: false,
    stdin_when_no_body: false,
};

const SECRET: Table = Table {
    long: &[("body", Kind::Text), ("env-file", Kind::File)],
    short: &[('b', Kind::Text), ('f', Kind::File)],
    value_long: &["app", "env", "org", "visibility", "repos", "repo"],
    value_short: "aeovrRu",
    positional_files: false,
    stdin_when_no_body: true,
};

const GENERIC: Table = Table {
    long: &[
        ("body", Kind::Text),
        ("body-file", Kind::File),
        ("input", Kind::Input),
    ],
    short: &[('b', Kind::Text)],
    value_long: &["repo"],
    value_short: "R",
    positional_files: false,
    stdin_when_no_body: false,
};

fn table_for(c: &Classification) -> &'static Table {
    match (c.family.as_str(), c.sub.as_deref()) {
        ("api", _) => &API,
        ("workflow", Some("run")) => &WORKFLOW_RUN,
        ("pr" | "issue", Some("create")) => &CREATE,
        ("pr" | "issue", Some("comment")) | ("pr", Some("review")) => &COMMENT,
        ("pr" | "issue", Some("edit")) => &EDIT,
        ("pr", Some("merge")) => &MERGE,
        ("pr" | "issue", Some("close" | "reopen")) => &CLOSE,
        ("release", Some("create" | "edit")) => &RELEASE,
        ("gist", Some("create")) => &GIST_CREATE,
        ("gist", Some("edit")) => &GIST_EDIT,
        ("label", Some("create" | "edit")) => &LABEL,
        ("repo", Some("create" | "edit")) => &REPO,
        ("secret" | "variable", Some("set")) => &SECRET,
        _ => &GENERIC,
    }
}

fn push_source(out: &mut Vec<BodySource>, flag: &str, kind: Kind, value: &str) {
    let (source, json) = match kind {
        Kind::Text => (SourceKind::Inline(value.to_string()), false),
        Kind::File => {
            if value == "-" {
                (SourceKind::Stdin, false)
            } else {
                (SourceKind::File(value.to_string()), false)
            }
        }
        Kind::RawField => {
            let v = value.split_once('=').map(|(_, v)| v).unwrap_or(value);
            (SourceKind::Inline(v.to_string()), false)
        }
        Kind::TypedField => {
            let v = value.split_once('=').map(|(_, v)| v).unwrap_or(value);
            match v.strip_prefix('@') {
                Some("-") => (SourceKind::Stdin, false),
                Some(path) => (SourceKind::File(path.to_string()), false),
                None => (SourceKind::Inline(v.to_string()), false),
            }
        }
        Kind::Input => {
            if value == "-" {
                (SourceKind::Stdin, true)
            } else {
                (SourceKind::File(value.to_string()), true)
            }
        }
        Kind::JsonStdin => (SourceKind::Stdin, true),
    };
    out.push(BodySource {
        flag: flag.to_string(),
        kind: source,
        json,
    });
}

/// Find every body source on a WRITE command line. `rest` is the argument list after the
/// command words. `stdin_is_tty` matters only for `secret set`/`variable set`, which read the
/// value from stdin when no flag supplies it and stdin is not a terminal.
pub fn body_sources(c: &Classification, rest: &[String], stdin_is_tty: bool) -> Vec<BodySource> {
    scan(c, rest, stdin_is_tty).0
}

/// Replace inline body text in `rest` with `<N bytes>` markers (and `gh api` header values with
/// `<redacted>`), for audit records and messages. File paths are kept.
pub fn redact(c: &Classification, rest: &[String]) -> Vec<String> {
    let mut out: Vec<String> = rest.to_vec();
    for (idx, replacement) in scan(c, rest, true).1 {
        if let Some(slot) = out.get_mut(idx) {
            *slot = replacement;
        }
    }
    out
}

fn inline_marker(kind: Kind, value: &str) -> Option<String> {
    match kind {
        Kind::Text => Some(format!("<{} bytes>", value.len())),
        Kind::RawField | Kind::TypedField => {
            let (key, v) = value.split_once('=').unwrap_or(("", value));
            if kind == Kind::TypedField && v.starts_with('@') {
                return None;
            }
            Some(format!("{key}=<{} bytes>", v.len()))
        }
        Kind::File | Kind::Input | Kind::JsonStdin => None,
    }
}

type Redactions = Vec<(usize, String)>;

fn scan(c: &Classification, rest: &[String], stdin_is_tty: bool) -> (Vec<BodySource>, Redactions) {
    let table = table_for(c);
    let is_api = std::ptr::eq(table, &API);
    let mut out = Vec::new();
    let mut red: Redactions = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        let at = i;
        let t = rest[i].as_str();
        i += 1;
        if t == "--" {
            if table.positional_files {
                for p in &rest[i..] {
                    push_source(&mut out, "<file>", Kind::File, p);
                }
            }
            break;
        }
        if let Some(long) = t.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((n, v)) => (n, Some(v)),
                None => (long, None),
            };
            if let Some((_, kind)) = table.long.iter().find(|(n, _)| *n == name) {
                if *kind == Kind::JsonStdin {
                    if inline.is_none_or(|v| v != "false") {
                        push_source(&mut out, t, *kind, "");
                    }
                    continue;
                }
                let value = match inline {
                    Some(v) => {
                        if let Some(m) = inline_marker(*kind, v) {
                            red.push((at, format!("--{name}={m}")));
                        }
                        v.to_string()
                    }
                    None => {
                        let v = rest.get(i).cloned().unwrap_or_default();
                        if let Some(m) = inline_marker(*kind, &v) {
                            red.push((i, m));
                        }
                        i += 1;
                        v
                    }
                };
                push_source(&mut out, &format!("--{name}"), *kind, &value);
            } else if table.value_long.contains(&name) {
                if is_api && name == "header" {
                    match inline {
                        Some(v) => red.push((at, format!("--header={}", redact_header(v)))),
                        None => {
                            if let Some(v) = rest.get(i) {
                                red.push((i, redact_header(v)));
                            }
                        }
                    }
                }
                if inline.is_none() {
                    i += 1;
                }
            }
            continue;
        }
        if t.len() > 1 && t.starts_with('-') {
            let chars: Vec<char> = t[1..].chars().collect();
            for (j, ch) in chars.iter().enumerate() {
                if let Some((_, kind)) = table.short.iter().find(|(s, _)| s == ch) {
                    let attached: String = chars[j + 1..].iter().collect();
                    let value = if attached.is_empty() {
                        let v = rest.get(i).cloned().unwrap_or_default();
                        if let Some(m) = inline_marker(*kind, &v) {
                            red.push((i, m));
                        }
                        i += 1;
                        v
                    } else {
                        let v = attached.strip_prefix('=').unwrap_or(&attached).to_string();
                        if let Some(m) = inline_marker(*kind, &v) {
                            let prefix: String = chars[..=j].iter().collect();
                            red.push((at, format!("-{prefix}{m}")));
                        }
                        v
                    };
                    push_source(&mut out, &format!("-{ch}"), *kind, &value);
                    break;
                }
                if table.value_short.contains(*ch) {
                    let attached: String = chars[j + 1..].iter().collect();
                    if is_api && *ch == 'H' {
                        if attached.is_empty() {
                            if let Some(v) = rest.get(i) {
                                red.push((i, redact_header(v)));
                            }
                        } else {
                            let prefix: String = chars[..=j].iter().collect();
                            red.push((at, format!("-{prefix}{}", redact_header(&attached))));
                        }
                    }
                    if attached.is_empty() {
                        i += 1;
                    }
                    break;
                }
            }
            continue;
        }
        if table.positional_files {
            push_source(&mut out, "<file>", Kind::File, t);
        }
    }
    if table.stdin_when_no_body && out.is_empty() && !stdin_is_tty {
        out.push(BodySource {
            flag: "<stdin>".to_string(),
            kind: SourceKind::Stdin,
            json: false,
        });
    }
    (out, red)
}

/// Keep a header's name and drop its value (it may carry a credential).
fn redact_header(h: &str) -> String {
    match h.split_once(':') {
        Some((name, _)) => format!("{}: <redacted>", name.trim()),
        None => "<redacted>".to_string(),
    }
}

/// Characters of the standard and URL-safe base64 alphabets, plus padding.
fn is_b64(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=' | b'_' | b'-')
}

#[derive(Default)]
struct Mix {
    upper: bool,
    lower: bool,
    digit: bool,
}

impl Mix {
    fn add(&mut self, b: u8) {
        self.upper |= b.is_ascii_uppercase();
        self.lower |= b.is_ascii_lowercase();
        self.digit |= b.is_ascii_digit();
    }
    fn mixed(&self) -> bool {
        self.upper && self.lower && self.digit
    }
}

/// Length of the longest base64-looking content in `text`: either one unbroken run of base64
/// characters, or a block of consecutive lines (each at least 20 characters and made only of
/// base64 characters) taken together. Content must mix upper case, lower case and digits to
/// count, so hex digests, rulers and long words are not mistaken for base64.
pub fn longest_base64_run(text: &[u8]) -> usize {
    let mut best = 0;
    let mut run = 0;
    let mut mix = Mix::default();
    for &b in text.iter().chain(std::iter::once(&b' ')) {
        if is_b64(b) {
            run += 1;
            mix.add(b);
        } else {
            if mix.mixed() && run > best {
                best = run;
            }
            run = 0;
            mix = Mix::default();
        }
    }
    let mut block = 0;
    let mut block_mix = Mix::default();
    for line in text.split(|&b| b == b'\n').chain(std::iter::once(&b""[..])) {
        let trimmed = trim_ascii(line);
        if trimmed.len() >= 20 && trimmed.iter().all(|&b| is_b64(b)) {
            block += trimmed.len();
            for &b in trimmed {
                block_mix.add(b);
            }
        } else {
            if block_mix.mixed() && block > best {
                best = block;
            }
            block = 0;
            block_mix = Mix::default();
        }
    }
    best
}

fn trim_ascii(line: &[u8]) -> &[u8] {
    let start = line
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(line.len());
    let end = line
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map(|p| p + 1)
        .unwrap_or(start);
    &line[start..end.max(start)]
}

fn json_strings(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(s) => out.push(s.clone()),
        serde_json::Value::Array(items) => items.iter().for_each(|v| json_strings(v, out)),
        serde_json::Value::Object(map) => map.values().for_each(|v| json_strings(v, out)),
        _ => {}
    }
}

/// Outcome of the content guard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The write may proceed; `bytes` is the total body size inspected.
    Allow {
        /// Total bytes across all body sources.
        bytes: usize,
    },
    /// The write must be refused for the stated reason.
    Refuse(String),
}

/// Read at most `limit + 1` bytes of a file, returning the bytes and the file's full size.
fn read_capped(path: &str, limit: usize) -> Result<(Vec<u8>, u64), String> {
    let file =
        std::fs::File::open(path).map_err(|e| format!("cannot read body file {path}: {e}"))?;
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    let mut buf = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("cannot read body file {path}: {e}"))?;
    let full = size.max(buf.len() as u64);
    Ok((buf, full))
}

/// Inspect every body source. `stdin` holds the buffered stdin when a source reads it (the
/// wrapper buffers at most `max_body_bytes + 1` bytes; a longer stdin is refused before this).
pub fn evaluate(sources: &[BodySource], cfg: &Config, stdin: Option<&[u8]>) -> Verdict {
    let limit = cfg.max_body_bytes;
    let mut total: u64 = 0;
    // (flag, inspected bytes, parse as JSON, the source was longer than what was read)
    let mut texts: Vec<(String, Vec<u8>, bool, bool)> = Vec::new();
    for s in sources {
        let (bytes, size) = match &s.kind {
            SourceKind::Inline(v) => (v.as_bytes().to_vec(), v.len() as u64),
            SourceKind::File(path) => match read_capped(path, limit) {
                Ok(x) => x,
                Err(e) => return Verdict::Refuse(e),
            },
            SourceKind::Stdin => match stdin {
                Some(buf) => (buf.to_vec(), buf.len() as u64),
                None => {
                    return Verdict::Refuse(format!(
                        "{} reads stdin, which was not buffered for inspection",
                        s.flag
                    ))
                }
            },
        };
        total += size;
        let truncated = size > bytes.len() as u64;
        texts.push((s.flag.clone(), bytes, s.json, truncated));
    }
    // Report every reason, not just the first, so the author sees all that must change.
    let mut reasons = Vec::new();
    if total > limit as u64 {
        reasons.push(format!(
            "write body is {total} bytes across {} source(s), over the {limit}-byte limit",
            sources.len()
        ));
    }
    for (flag, bytes, json, truncated) in &texts {
        let mut run = longest_base64_run(bytes);
        if *json {
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) {
                let mut strings = Vec::new();
                json_strings(&value, &mut strings);
                for s in strings {
                    run = run.max(longest_base64_run(s.as_bytes()));
                }
            }
        }
        if run > cfg.max_base64_run {
            let at_least = if *truncated { "at least " } else { "" };
            reasons.push(format!(
                "{flag} contains a base64-looking run of {at_least}{run} characters (limit {}); \
                 never store encoded data in GitHub text",
                cfg.max_base64_run
            ));
        }
    }
    if !reasons.is_empty() {
        return Verdict::Refuse(reasons.join("; "));
    }
    Verdict::Allow {
        bytes: usize::try_from(total).unwrap_or(usize::MAX),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classify::classify;

    fn sources(line: &[&str]) -> Vec<BodySource> {
        let args: Vec<String> = line.iter().map(|s| s.to_string()).collect();
        let c = classify(&args, &Config::default());
        body_sources(&c, &args[c.rest_start..], true)
    }

    fn b64ish(n: usize) -> String {
        const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut x: u32 = 12345;
        (0..n)
            .map(|_| {
                x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                A[((x >> 16) % 64) as usize] as char
            })
            .collect()
    }

    #[test]
    fn finds_body_flags_per_command() {
        let s = sources(&["pr", "comment", "1", "-b", "hello"]);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].kind, SourceKind::Inline("hello".into()));
        let s = sources(&["pr", "create", "-d", "-b", "body", "-t", "title"]);
        assert_eq!(s.len(), 2, "-d is --draft (bool) in pr create: {s:?}");
        let s = sources(&["issue", "create", "--body-file", "-"]);
        assert!(s[0].is_stdin());
        let s = sources(&["pr", "comment", "1", "-RFoo/Bar", "-bx"]);
        assert_eq!(
            s,
            vec![BodySource {
                flag: "-b".into(),
                kind: SourceKind::Inline("x".into()),
                json: false
            }]
        );
        let s = sources(&["pr", "comment", "1", "--body=inline"]);
        assert_eq!(s[0].kind, SourceKind::Inline("inline".into()));
        let s = sources(&["pr", "merge", "1", "--squash", "-t", "subj", "-b", "body"]);
        assert_eq!(s.len(), 2);
        let s = sources(&[
            "issue",
            "close",
            "1",
            "-c",
            "closing note",
            "-r",
            "completed",
        ]);
        assert_eq!(s.len(), 1);
        let s = sources(&["pr", "review", "1", "-c", "-b", "note"]);
        assert_eq!(s.len(), 1, "-c is --comment (bool) in pr review: {s:?}");
        let s = sources(&[
            "release",
            "create",
            "v1",
            "-n",
            "notes",
            "-F",
            "notes.md",
            "asset.bin",
        ]);
        assert_eq!(s.len(), 2);
        let s = sources(&["gist", "create", "-d", "desc", "a.txt", "-"]);
        assert_eq!(s.len(), 3);
        assert!(s[2].is_stdin());
    }

    #[test]
    fn finds_api_body_sources() {
        let s = sources(&[
            "api",
            "repos/o/r/issues/1/comments",
            "-f",
            "body=hi",
            "-F",
            "n=@x.md",
            "-F",
            "m=@-",
            "-F",
            "k=3",
        ]);
        assert_eq!(s.len(), 4);
        assert_eq!(s[0].kind, SourceKind::Inline("hi".into()));
        assert_eq!(s[1].kind, SourceKind::File("x.md".into()));
        assert!(s[2].is_stdin());
        assert_eq!(s[3].kind, SourceKind::Inline("3".into()));
        let s = sources(&["api", "--method", "POST", "x", "--input", "req.json"]);
        assert_eq!(
            s,
            vec![BodySource {
                flag: "--input".into(),
                kind: SourceKind::File("req.json".into()),
                json: true
            }]
        );
        let s = sources(&["api", "-H", "-f", "x", "-fbody=y"]);
        assert_eq!(s.len(), 1, "-f after -H is the header value: {s:?}");
    }

    #[test]
    fn secret_set_reads_stdin_only_when_not_a_terminal() {
        let args: Vec<String> = ["secret", "set", "NAME"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let c = classify(&args, &Config::default());
        assert!(body_sources(&c, &args[2..], true).is_empty());
        assert!(body_sources(&c, &args[2..], false)[0].is_stdin());
    }

    #[test]
    fn base64_detection() {
        assert_eq!(longest_base64_run(b"plain prose with words."), 0);
        let blob = b64ish(1500);
        assert_eq!(longest_base64_run(blob.as_bytes()), 1500);
        // Wrapped at 76 columns, inside a fenced block, the lines still count together.
        let wrapped: String = blob
            .as_bytes()
            .chunks(76)
            .map(|c| format!("  {}\n", String::from_utf8_lossy(c)))
            .collect();
        let text = format!("Here is data:\n```\n{wrapped}```\n");
        assert_eq!(longest_base64_run(text.as_bytes()), 1500);
        // Hex digests and rulers do not count.
        let hex = "0123456789abcdef".repeat(100);
        assert_eq!(longest_base64_run(hex.as_bytes()), 0);
        assert_eq!(longest_base64_run("-".repeat(2000).as_bytes()), 0);
    }

    #[test]
    fn evaluate_size_and_base64() {
        let cfg = Config::default();
        let ok = vec![BodySource {
            flag: "-b".into(),
            kind: SourceKind::Inline("x".repeat(4000)),
            json: false,
        }];
        assert_eq!(evaluate(&ok, &cfg, None), Verdict::Allow { bytes: 4000 });
        let big = vec![BodySource {
            flag: "-b".into(),
            kind: SourceKind::Inline("word ".repeat(2000)),
            json: false,
        }];
        assert!(matches!(evaluate(&big, &cfg, None),
            Verdict::Refuse(m) if m.contains("10000 bytes across 1 source(s), over the 8192-byte limit")
                && !m.contains("base64")));
        let enc = vec![BodySource {
            flag: "-b".into(),
            kind: SourceKind::Inline(b64ish(1200)),
            json: false,
        }];
        assert!(matches!(evaluate(&enc, &cfg, None), Verdict::Refuse(m) if m.contains("base64")));
        // Two sources that are each small but together too large.
        let two = vec![
            BodySource {
                flag: "-t".into(),
                kind: SourceKind::Inline("a ".repeat(2100)),
                json: false,
            },
            BodySource {
                flag: "-b".into(),
                kind: SourceKind::Inline("b ".repeat(2100)),
                json: false,
            },
        ];
        assert!(matches!(evaluate(&two, &cfg, None),
            Verdict::Refuse(m) if m.contains("8400 bytes across 2 source(s)")));
        // JSON request body: base64 split by escaped newlines is found after decoding.
        let lines: Vec<String> = b64ish(1200)
            .as_bytes()
            .chunks(60)
            .map(|c| String::from_utf8_lossy(c).to_string())
            .collect();
        let json = serde_json::json!({"body": lines.join("\n")}).to_string();
        let src = vec![BodySource {
            flag: "--input".into(),
            kind: SourceKind::Stdin,
            json: true,
        }];
        assert!(matches!(
            evaluate(&src, &cfg, Some(json.as_bytes())),
            Verdict::Refuse(_)
        ));
        // Unreadable file is refused, not waved through.
        let missing = vec![BodySource {
            flag: "-F".into(),
            kind: SourceKind::File("/nonexistent/x".into()),
            json: false,
        }];
        assert!(matches!(evaluate(&missing, &cfg, None), Verdict::Refuse(_)));
        // Stdin source without a buffer is refused.
        let unbuffered = vec![BodySource {
            flag: "--input".into(),
            kind: SourceKind::Stdin,
            json: true,
        }];
        assert!(matches!(
            evaluate(&unbuffered, &cfg, None),
            Verdict::Refuse(_)
        ));
    }

    /// An `--input` request file shaped like a chunked-archive upload: far over the size limit and
    /// one long base64 line inside the JSON `body`. Both reasons are reported, and the run length
    /// is marked as a lower bound because only the first `limit + 1` bytes are read.
    #[test]
    fn oversized_encoded_input_file_reports_both_reasons() {
        let dir = std::env::temp_dir().join(format!("gh-paced-guard-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("part.request.json");
        let body = format!("part 01/21\n\n```\n{}\n```", b64ish(60_000));
        std::fs::write(&path, serde_json::json!({ "body": body }).to_string()).unwrap();
        let src = vec![BodySource {
            flag: "--input".into(),
            kind: SourceKind::File(path.display().to_string()),
            json: true,
        }];
        let verdict = evaluate(&src, &Config::default(), None);
        let Verdict::Refuse(m) = verdict else {
            panic!("allowed: {verdict:?}")
        };
        assert!(m.contains("over the 8192-byte limit"), "{m}");
        assert!(
            m.contains("--input contains a base64-looking run of at least"),
            "{m}"
        );
        // A small prose request file passes.
        std::fs::write(
            &path,
            serde_json::json!({ "body": "word ".repeat(800) }).to_string(),
        )
        .unwrap();
        assert!(matches!(
            evaluate(&src, &Config::default(), None),
            Verdict::Allow { .. }
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
