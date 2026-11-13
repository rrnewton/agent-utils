//! Host-local allowlist for admin-only `gh api` calls.
//!
//! The file is consumer data: this crate ships only the format, documentation and an
//! example with placeholder names. The file itself lives outside the checkout (default
//! `~/.local/etc/gh-paced/admin-allow`) and is never read from the repository or a synced
//! directory unless the caller explicitly points [`GH_PACED_ADMIN_ALLOW_FILE`] or
//! `--admin-allow-file` there.
//!
//! Format, one entry per line; blank lines and `#` comments are ignored:
//!
//! ```text
//! PATCH repos/OWNER/REPO fields=default_branch,allow_merge_commit
//! POST repos/OWNER/REPO/rulesets
//! PUT repos/OWNER/REPO/rulesets/{id}
//! DELETE repos/OWNER/REPO/rulesets/{id}
//! ```
//!
//! * `METHOD` is an HTTP method (`PATCH`, `POST`, ...), case-insensitive.
//! * The path is an exact API path with no scheme, host, leading slash or query. It is
//!   normalised the same way `classify::normalize_endpoint` normalises a request
//!   (leading slash and any `api/v3/` prefix dropped) and compared exactly. No globs:
//!   `*`, `?` and any `{...}` other than the single ruleset form below are malformed.
//! * `/{id}` is allowed only as the final segment of a `PUT` or `DELETE` on a
//!   `.../rulesets` path; it matches exactly one further non-empty segment.
//! * `fields=a,b` (only as a third token) restricts the request body: every field the
//!   call sends (`-f`/`-F`/`--raw-field`/`--field` keys and top-level `--input` JSON
//!   keys) must be listed. A call whose body cannot be inspected is refused when the
//!   entry restricts fields (fail closed).
//!
//! A missing file means no entries (every admin call stays refused). A malformed,
//! non-regular, symlinked, wrongly-owned or wrongly-moded file yields no entries and a
//! stored error that refuses every admin call; non-admin calls are unaffected.

use crate::classify::{ApiArgs, MAX_INSPECT_BYTES};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Environment variable naming the allowlist file.
pub const ENV_VAR: &str = "GH_PACED_ADMIN_ALLOW_FILE";

/// One allowlist entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowEntry {
    /// HTTP method, upper case.
    pub method: String,
    /// Normalised exact path (leading slash and `api/v3/` dropped, lower case). For a
    /// ruleset-id entry this is the base before `/{id}`.
    pub path: String,
    /// True when the entry ended in `/{id}` (PUT/DELETE on `.../rulesets` only).
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_ruleset_id: bool,
    /// Allowed body fields when the entry named `fields=...`; `None` = unrestricted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<String>>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl AllowEntry {
    /// Display form (`METHOD path[/{id}][ fields=...]`).
    pub fn display(&self) -> String {
        let mut s = format!("{} {}", self.method, self.path);
        if self.is_ruleset_id {
            s.push_str("/{id}");
        }
        if let Some(f) = &self.fields {
            s.push_str(&format!(" fields={}", f.join(",")));
        }
        s
    }

    fn matches(&self, method: &str, normalized_path: &str) -> bool {
        if self.method != method {
            return false;
        }
        if self.is_ruleset_id {
            let Some(rest) = normalized_path.strip_prefix(&format!("{}/", self.path)) else {
                return false;
            };
            !rest.is_empty() && !rest.contains('/')
        } else {
            self.path == normalized_path
        }
    }
}

/// A call the allowlist admitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminAllow {
    /// HTTP method, upper case.
    pub method: String,
    /// Request path as normalised for display (endpoint with scheme/host/slash/query dropped).
    pub path: String,
    /// Body fields the call sends (sorted, de-duplicated; empty when it sends none).
    pub fields: Vec<String>,
    /// The entry that admitted it, in display form.
    pub entry: String,
}

/// Body fields a call sends, or why they cannot be known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldSet {
    /// Every field name, in first-seen order.
    Known(Vec<String>),
    /// The body cannot be inspected without consuming stdin or reading an unusable file.
    Unknown(String),
}

impl FieldSet {
    /// Sorted, de-duplicated field list for logging; `None` when unknown.
    pub fn sorted_unique(&self) -> Option<Vec<String>> {
        match self {
            FieldSet::Known(v) => {
                let mut out = v.clone();
                out.sort();
                out.dedup();
                Some(out)
            }
            FieldSet::Unknown(_) => None,
        }
    }
}

/// Default allowlist path for a home directory: `~/.local/etc/gh-paced/admin-allow`.
pub fn default_path(home: &str) -> PathBuf {
    PathBuf::from(home).join(".local/etc/gh-paced/admin-allow")
}

/// Normalise an entry or request path for exact comparison: drop scheme/host (entries
/// must not carry one), leading slashes, query/fragment, and any `api/v3/` prefix, then
/// lower-case. Returns the display path separately in [`normalize_display`].
pub fn normalize_for_match(raw: &str) -> String {
    normalize_display(raw).to_ascii_lowercase()
}

/// Normalise for display: drop scheme/host, leading slashes, query/fragment and any
/// `api/v3/` prefix, preserving case.
pub fn normalize_display(raw: &str) -> String {
    let mut e = raw.trim();
    for scheme in ["https://", "http://"] {
        if let Some(after) = e.strip_prefix(scheme) {
            e = after.find('/').map(|i| &after[i..]).unwrap_or("");
            break;
        }
    }
    let e = e.trim_start_matches('/');
    let e = e.split(['?', '#']).next().unwrap_or("");
    let trimmed = e.trim_end_matches('/');
    // Drop an api/v3/ prefix (matched case-insensitively, as in `admin_only`).
    let lower = trimmed.to_ascii_lowercase();
    if lower == "api/v3" {
        return String::new();
    }
    if let Some(rest) = lower.strip_prefix("api/v3/") {
        // Preserve the case of the remainder from the original string.
        let orig_rest = &trimmed[trimmed.len() - rest.len()..];
        return orig_rest.trim_end_matches('/').to_string();
    }
    trimmed.to_string()
}

fn valid_field_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.')
}

/// Parse allowlist contents. `source` names the file in error messages.
pub fn parse_contents(text: &str, source: &str) -> Result<Vec<AllowEntry>, String> {
    let mut entries = Vec::new();
    for (idx, raw_line) in text.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let bad = |msg: &str| format!("{source}:{line_no}: {msg}: {line:?}");
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.len() < 2 || tokens.len() > 3 {
            return Err(bad("expected `METHOD PATH [fields=a,b]`"));
        }
        let method = tokens[0].to_ascii_uppercase();
        if !matches!(
            method.as_str(),
            "GET" | "POST" | "PATCH" | "PUT" | "DELETE" | "HEAD" | "OPTIONS"
        ) {
            return Err(bad("unknown HTTP method"));
        }
        let raw_path = tokens[1];
        if raw_path.contains('*') {
            return Err(bad("globs are not allowed; name an exact path"));
        }
        let (is_ruleset_id, base_raw) = if let Some(base) = raw_path.strip_suffix("/{id}") {
            if raw_path.matches("{id}").count() != 1 || base.contains(['{', '}']) {
                return Err(bad("only a single trailing `/{id}` is allowed"));
            }
            if !matches!(method.as_str(), "PUT" | "DELETE") {
                return Err(bad("`/{id}` is allowed only for PUT or DELETE"));
            }
            (true, base)
        } else if raw_path.contains(['{', '}']) {
            return Err(bad("only a trailing `/{id}` on a rulesets path is allowed"));
        } else {
            (false, raw_path)
        };
        if raw_path.contains(['?', '#', ':']) {
            return Err(bad("path must not carry a scheme, host or query"));
        }
        if base_raw.contains("//") {
            return Err(bad("path must not contain empty segments"));
        }
        let display = normalize_display(base_raw);
        if display.is_empty() {
            return Err(bad("empty path"));
        }
        if display.split('/').any(|s| s.is_empty()) {
            return Err(bad("path must not contain empty segments"));
        }
        let path = display.to_ascii_lowercase();
        if is_ruleset_id {
            let last = path.rsplit('/').next().unwrap_or("");
            if last != "rulesets" {
                return Err(bad("`/{id}` is allowed only on a `.../rulesets` path"));
            }
        }
        let fields = if tokens.len() == 3 {
            let Some(list) = tokens[2].strip_prefix("fields=") else {
                return Err(bad("third token must be `fields=a,b`"));
            };
            if list.is_empty() {
                return Err(bad("`fields=` names no fields"));
            }
            let mut fs = Vec::new();
            for name in list.split(',') {
                let name = name.trim();
                if !valid_field_name(name) {
                    return Err(bad("invalid field name in `fields=`"));
                }
                fs.push(name.to_string());
            }
            Some(fs)
        } else {
            None
        };
        entries.push(AllowEntry {
            method,
            path,
            is_ruleset_id,
            fields,
        });
    }
    Ok(entries)
}

/// Load and validate the allowlist at `path` (permissions, ownership, then contents).
/// Any failure is an error string that refuses every admin call (fail closed).
pub fn load_file(path: &Path) -> Result<Vec<AllowEntry>, String> {
    let shown = path.display().to_string();
    let link_meta =
        std::fs::symlink_metadata(path).map_err(|e| format!("admin allowlist {shown}: {e}"))?;
    if link_meta.file_type().is_symlink() {
        return Err(format!(
            "admin allowlist {shown} is a symlink; name the file itself"
        ));
    }
    let meta = std::fs::metadata(path).map_err(|e| format!("admin allowlist {shown}: {e}"))?;
    if !meta.is_file() {
        return Err(format!("admin allowlist {shown} is not a regular file"));
    }
    check_permissions(path, &meta)?;
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("admin allowlist {shown}: {e}"))?;
    parse_contents(&text, &shown)
}

fn check_permissions(path: &Path, meta: &std::fs::Metadata) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let mode = meta.permissions().mode() & 0o777;
        if mode != 0o600 && mode != 0o644 {
            return Err(format!(
                "admin allowlist {} has mode {:03o}; it must be 0600 or 0644",
                path.display(),
                mode
            ));
        }
        // SAFETY: geteuid has no preconditions.
        let euid = unsafe { libc::geteuid() };
        if meta.uid() != euid {
            return Err(format!(
                "admin allowlist {} is owned by uid {}, not by the current user (uid {euid})",
                path.display(),
                meta.uid()
            ));
        }
    }
    Ok(())
}

/// The body field name a `-f`/`-F` pair sends: the key before `=`, up to any `[`.
pub fn field_key(pair: &str) -> &str {
    let key = pair.split('=').next().unwrap_or("");
    key.split('[').next().unwrap_or("")
}

fn read_small_text(path: &str) -> Option<String> {
    use std::io::Read;
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_INSPECT_BYTES {
        return None;
    }
    let file = std::fs::File::open(path).ok()?;
    let mut buf = String::new();
    file.take(MAX_INSPECT_BYTES + 1)
        .read_to_string(&mut buf)
        .ok()?;
    if buf.len() as u64 > MAX_INSPECT_BYTES {
        return None;
    }
    Some(buf)
}

/// Every body field a `gh api` call sends, or why they cannot be known.
pub fn extract_fields(a: &ApiArgs) -> FieldSet {
    let mut fields: Vec<String> = Vec::new();
    for pair in a.raw_fields.iter().chain(a.typed_fields.iter()) {
        let key = field_key(pair);
        if key.is_empty() {
            return FieldSet::Unknown(format!("field {pair:?} has an empty key"));
        }
        fields.push(key.to_string());
    }
    if let Some(input) = &a.input {
        if input == "-" {
            return FieldSet::Unknown("the request body is on stdin (--input -)".to_string());
        }
        let Some(text) = read_small_text(input) else {
            return FieldSet::Unknown(format!(
                "the --input file {input:?} cannot be read for inspection"
            ));
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            return FieldSet::Unknown(format!("the --input file {input:?} is not valid JSON"));
        };
        let Some(obj) = value.as_object() else {
            return FieldSet::Unknown(format!("the --input file {input:?} is not a JSON object"));
        };
        for key in obj.keys() {
            fields.push(key.clone());
        }
    }
    FieldSet::Known(fields)
}

/// Whether the allowlist admits `method` on `endpoint` with body `fields`.
/// On success the admitted call; on failure a refusal detail naming why not.
pub fn check(
    entries: &[AllowEntry],
    load_error: Option<&str>,
    method: &str,
    endpoint: &str,
    fields: &FieldSet,
) -> Result<AdminAllow, String> {
    if let Some(e) = load_error {
        return Err(format!("the admin allowlist cannot be used: {e}"));
    }
    let method_up = method.to_ascii_uppercase();
    let normalized = normalize_for_match(endpoint);
    let display = normalize_display(endpoint);
    let Some(entry) = entries.iter().find(|e| e.matches(&method_up, &normalized)) else {
        return Err(format!(
            "no admin allowlist entry matches {method_up} {display} (an entry must name the method and the exact path; add one to the host-local allowlist file or set GH_PACED_ALLOW_ADMIN_API=1)"
        ));
    };
    if let Some(allowed) = &entry.fields {
        match fields {
            FieldSet::Unknown(why) => {
                return Err(format!(
                    "entry `{}` restricts the body to fields={}, but {why}, so the fields cannot be checked",
                    entry.display(),
                    allowed.join(",")
                ));
            }
            FieldSet::Known(sent) => {
                for f in sent {
                    if !allowed.contains(f) {
                        return Err(format!(
                            "field {f:?} is not in the allowlisted fields={} for {} {}",
                            allowed.join(","),
                            method_up,
                            display
                        ));
                    }
                }
            }
        }
    }
    let sent = fields.sorted_unique().unwrap_or_default();
    Ok(AdminAllow {
        method: method_up,
        path: display,
        fields: sent,
        entry: entry.display(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(text: &str) -> Vec<AllowEntry> {
        parse_contents(text, "test-allow").unwrap()
    }

    fn known(fields: &[&str]) -> FieldSet {
        FieldSet::Known(fields.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn exact_match_and_ruleset_id() {
        let es = entries(
            "PATCH repos/OWNER/REPO fields=default_branch,allow_merge_commit\n\
             POST repos/OWNER/REPO/rulesets\n\
             PUT repos/OWNER/REPO/rulesets/{id}\n",
        );
        assert!(check(
            &es,
            None,
            "PATCH",
            "repos/owner/repo",
            &known(&["default_branch"])
        )
        .is_ok());
        assert!(check(
            &es,
            None,
            "PUT",
            "repos/owner/repo/rulesets/42",
            &known(&[])
        )
        .is_ok());
        // No id, extra segment, wrong method and non-listed paths all miss.
        assert!(check(&es, None, "PUT", "repos/owner/repo/rulesets", &known(&[])).is_err());
        assert!(check(
            &es,
            None,
            "PUT",
            "repos/owner/repo/rulesets/42/extra",
            &known(&[])
        )
        .is_err());
        assert!(check(
            &es,
            None,
            "POST",
            "repos/owner/repo/rulesets/42",
            &known(&[])
        )
        .is_err());
        assert!(check(&es, None, "DELETE", "repos/owner/repo", &known(&[])).is_err());
        assert!(check(
            &es,
            None,
            "PUT",
            "repos/owner/repo/branches/main/protection",
            &known(&[])
        )
        .is_err());
    }

    #[test]
    fn fields_restriction_and_unknown_body() {
        let es = entries("PATCH repos/OWNER/REPO fields=default_branch,allow_merge_commit\n");
        let extra = check(
            &es,
            None,
            "PATCH",
            "repos/owner/repo",
            &known(&["default_branch", "description"]),
        )
        .unwrap_err();
        assert!(extra.contains("description"), "{extra}");
        assert!(check(
            &es,
            None,
            "PATCH",
            "repos/owner/repo",
            &FieldSet::Unknown("stdin".into())
        )
        .is_err());
        let ok = check(
            &es,
            None,
            "PATCH",
            "repos/owner/repo",
            &known(&["allow_merge_commit", "default_branch"]),
        )
        .unwrap();
        assert_eq!(ok.fields, ["allow_merge_commit", "default_branch"]);
    }

    #[test]
    fn malformed_lines_are_rejected() {
        for text in [
            "PATCH\n",
            "PATCH repos/o/r fields=\n",
            "PATCH repos/o/r extra=1\n",
            "FLY repos/o/r\n",
            "GET repos/o/r/*\n",
            "PATCH repos/o/r/{id}\n",
            "POST repos/o/r/rulesets/{id}\n",
            "PUT repos/o/r/hooks/{id}\n",
            "PUT repos/o/r/rulesets/{id} fields=a b\n",
            "PATCH https://api.github.com/repos/o/r\n",
            "PATCH repos/o/r?x=1\n",
        ] {
            assert!(parse_contents(text, "t").is_err(), "accepted {text:?}");
        }
        // Comments, blanks and an api/v3 prefix are fine.
        let es = entries("# note\n\nPATCH /api/v3/repos/o/r\n");
        assert_eq!(es[0].path, "repos/o/r");
    }

    #[test]
    fn load_error_refuses_everything() {
        let es = entries("PATCH repos/o/r fields=default_branch\n");
        assert!(check(&es, Some("bad file"), "PATCH", "repos/o/r", &known(&[])).is_err());
    }
}
