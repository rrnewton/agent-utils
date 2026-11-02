//! Append-only audit log: one JSON line per decision, per account.
//!
//! Records hold the time (UTC and US Eastern), host, process ID, class, cost, command, a short
//! argument summary with every inline body replaced by `<N bytes>` and header values redacted,
//! the event, and for completed calls the exit status and time waited. They never contain
//! request bodies, tokens or environment variables. The log is rotated to `.1` past 10 MiB;
//! write it only while holding the account lock so rotation cannot race.

use crate::timefmt::{eastern_stamp, rfc3339_utc};
use serde::Serialize;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// Rotate the log past this size, bytes.
pub const ROTATE_BYTES: u64 = 10 * 1024 * 1024;
/// Longest argument summary kept, characters.
pub const SUMMARY_CHARS: usize = 300;

/// One audit record.
#[derive(Debug, Clone, Serialize)]
pub struct Record {
    /// UTC time, RFC 3339.
    pub ts: String,
    /// US Eastern wall time.
    pub et: String,
    /// Short host name.
    pub host: String,
    /// gh-paced process ID.
    pub pid: u32,
    /// `admit`, `throttle`, `refuse`, `pushback`, `exit`, `refresh`, `guard`.
    pub event: String,
    /// Request class.
    pub class: String,
    /// Tokens charged.
    pub cost: u32,
    /// Command (`pr comment`, `api GET repos/o/r`).
    pub command: String,
    /// Redacted, truncated argument summary.
    pub argv: String,
    /// Child exit status (`exit` events), or gh-paced's own refusal status.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rc: Option<i32>,
    /// Signal that killed the child, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal: Option<i32>,
    /// Who asked, for git-credential events: a `GH_PACED_CALLER` env tag
    /// when the caller set one, else the parent chain (the git process and
    /// the process that launched it, with its cwd). Empty for other classes.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub caller: String,
    /// Seconds this invocation slept before it was admitted or refused.
    pub waited_secs: f64,
    /// Short human-readable detail (reason for a throttle, refusal or pushback).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

impl Record {
    /// A record stamped at `now`, with the remaining fields filled by the caller.
    pub fn at(now: f64, host: &str, event: &str) -> Self {
        Self {
            ts: rfc3339_utc(now),
            et: eastern_stamp(now),
            host: host.to_string(),
            pid: std::process::id(),
            event: event.to_string(),
            class: String::new(),
            cost: 0,
            command: String::new(),
            argv: String::new(),
            rc: None,
            signal: None,
            caller: String::new(),
            waited_secs: 0.0,
            detail: String::new(),
        }
    }
}

/// Longest caller command line kept, characters.
pub const CALLER_CHARS: usize = 160;

/// Flatten a process command line for the audit log: NULs and control
/// characters become spaces, and the result is clipped. Never an argument
/// body or environment content, only the invoking command line itself.
pub fn flatten_cmdline(raw: &[u8]) -> String {
    let text: String = raw
        .iter()
        .map(|&b| if b == 0 || b < 0x20 { ' ' } else { b as char })
        .collect();
    let text = text.trim();
    let mut out = String::new();
    for word in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
        if out.chars().count() > CALLER_CHARS {
            let clipped: String = out.chars().take(CALLER_CHARS).collect();
            return format!("{clipped}...");
        }
    }
    out
}

/// The parent pid from a `/proc/<pid>/stat` body (the comm field may itself
/// contain spaces and parentheses, so parse after the last `)`).
pub fn parent_pid_of_stat(stat: &str) -> Option<u32> {
    let after_comm = stat.rsplit_once(')')?.1;
    let mut fields = after_comm.split_whitespace();
    let _state = fields.next()?;
    fields.next()?.parse().ok()
}

fn parent_pid(pid: u32) -> Option<u32> {
    parent_pid_of_stat(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

/// Identify who invoked a git-credential call: the `GH_PACED_CALLER` env
/// tag when set, else the parent chain (git, then the process that
/// launched git) as `cmd @ cwd <- cmd @ cwd`. Best-effort: any unreadable
/// step just shortens the chain.
pub fn caller_identity() -> String {
    if let Ok(tag) = std::env::var("GH_PACED_CALLER") {
        let tag = flatten_cmdline(tag.as_bytes());
        if !tag.is_empty() {
            return format!("env:{tag}");
        }
    }
    let mut parts: Vec<String> = Vec::new();
    let mut pid = std::process::id();
    for _ in 0..2 {
        let Some(ppid) = parent_pid(pid) else { break };
        let cmd = std::fs::read(format!("/proc/{ppid}/cmdline"))
            .map(|raw| flatten_cmdline(&raw))
            .unwrap_or_default();
        let cwd = std::fs::read_link(format!("/proc/{ppid}/cwd"))
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        if cmd.is_empty() && cwd.is_empty() {
            break;
        }
        parts.push(format!("{cmd} @ {cwd}"));
        pid = ppid;
    }
    parts.join(" <- ")
}

/// Join redacted arguments, clipping long tokens and the whole summary.
pub fn summarize(args: &[String]) -> String {
    let mut out = String::new();
    for a in args {
        let token: String = if a.chars().count() > 80 {
            let head: String = a.chars().take(60).collect();
            format!("{head}...<{} bytes>", a.len())
        } else {
            a.clone()
        };
        let token = token.replace(['\n', '\r', '\t'], " ");
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&token);
        if out.chars().count() > SUMMARY_CHARS {
            let clipped: String = out.chars().take(SUMMARY_CHARS).collect();
            return format!("{clipped}...");
        }
    }
    out
}

/// Append one record, rotating first when the log is too large. Errors are returned for the
/// caller to report; they never stop a call.
pub fn append(path: &Path, record: &Record) -> Result<(), String> {
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() > ROTATE_BYTES {
            let mut rotated = path.as_os_str().to_owned();
            rotated.push(".1");
            let _ = std::fs::rename(path, rotated);
        }
    }
    let mut line = serde_json::to_vec(record).map_err(|e| format!("audit encode: {e}"))?;
    line.push(b'\n');
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("cannot open audit log {}: {e}", path.display()))?;
    file.write_all(&line)
        .map_err(|e| format!("cannot write audit log {}: {e}", path.display()))
}

/// The last `n` records of a log (unparseable lines are skipped).
pub fn tail(path: &Path, n: usize) -> Vec<serde_json::Value> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..]
        .iter()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_clips_and_flattens() {
        let args: Vec<String> = vec![
            "pr".into(),
            "comment".into(),
            "a\nb".into(),
            "x".repeat(200),
        ];
        let s = summarize(&args);
        assert!(s.starts_with("pr comment a b "));
        assert!(s.contains("...<200 bytes>"));
        let many: Vec<String> = (0..200).map(|i| format!("arg{i}")).collect();
        assert!(summarize(&many).chars().count() <= SUMMARY_CHARS + 3);
    }

    #[test]
    fn caller_cmdline_is_flattened_and_clipped() {
        let raw = b"git\0-C\0/some/repo\tfetch\norigin";
        assert_eq!(flatten_cmdline(raw), "git -C /some/repo fetch origin");
        let long = vec![b'x'; 500];
        let flat = flatten_cmdline(&long);
        assert!(flat.chars().count() <= CALLER_CHARS + 3);
        assert!(flat.ends_with("..."));
        assert_eq!(flatten_cmdline(b""), "");
    }

    #[test]
    fn caller_parent_pid_survives_parenthesized_comm() {
        let stat = "4242 (weird (name) here) S 1234 4242 4242 0 -1 4194304";
        assert_eq!(parent_pid_of_stat(stat), Some(1234));
        assert_eq!(parent_pid_of_stat("no parens at all"), None);
    }

    #[test]
    fn append_and_tail() {
        let dir = std::env::temp_dir().join(format!("gh-paced-audit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("a.audit.jsonl");
        let _ = std::fs::remove_file(&path);
        for event in ["admit", "exit"] {
            let mut r = Record::at(1_790_000_000.0, "host", event);
            r.class = "read".into();
            append(&path, &r).expect("append");
        }
        let t = tail(&path, 5);
        assert_eq!(t.len(), 2);
        assert_eq!(t[1]["event"], "exit");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
