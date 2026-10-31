//! Deterministic gzip sidecars, glossary rendering, and GitHub
//! pull-request references at parity with `py/wrkviz/static_assets.py`,
//! the rendering half of `py/wrkviz/terminology.py`, and
//! `py/wrkviz/github_refs.py`.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use sha2::Digest as _;

use crate::summarize_core::{char_len, char_take, py_trim};
use crate::transcript_io::write_text_if_changed;

/// Default gzip minimum bytes.
pub const GZIP_MINIMUM_BYTES: u64 = 1024;

/// The conventional precompressed sidecar path.
pub fn gzip_sidecar_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".gz");
    path.with_file_name(name)
}

/// A reproducible gzip-6 stream with no filename or timestamp.
pub fn deterministic_gzip(data: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::new(6),
    );
    // flate2's header carries an mtime; rewrite it to zero afterwards
    // (bytes 4..8 of the gzip header).
    let _ = encoder.write_all(data);
    let mut out = encoder.finish().unwrap_or_default();
    if out.len() >= 8 {
        out[4] = 0;
        out[5] = 0;
        out[6] = 0;
        out[7] = 0;
    }
    // Ensure no original filename is present (FLG bit 3).
    if out.len() >= 4 {
        out[3] &= !0x08;
    }
    out
}

fn is_symlink(path: &Path) -> bool {
    path.symlink_metadata()
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
}

/// Write text without pairing new identity bytes with an old sidecar.
pub fn write_text_with_gzip_invalidation(
    path: &Path,
    text: &str,
) -> Result<i64, String> {
    write_text_with_gzip_invalidation_executable(path, text, false)
}

/// Executable variant.
pub fn write_text_with_gzip_invalidation_executable(
    path: &Path,
    text: &str,
    executable: bool,
) -> Result<i64, String> {
    if is_symlink(path) {
        return Err(format!(
            "refusing to replace symlinked static asset: {}",
            path.display()
        ));
    }
    let sidecar = gzip_sidecar_path(path);
    if is_symlink(&sidecar) || (sidecar.exists() && !sidecar.is_file()) {
        return Err(format!(
            "refusing unsafe gzip sidecar: {}",
            sidecar.display()
        ));
    }
    let content_changed = !path.is_file()
        || std::fs::read_to_string(path).map_err(|error| crate::json::os_error(&error, path))?
            != text;
    if !content_changed {
        if executable {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                if let Ok(meta) = std::fs::metadata(path) {
                    let mut permissions = meta.permissions();
                    permissions.set_mode(permissions.mode() | 0o111);
                    let _ = std::fs::set_permissions(path, permissions);
                }
            }
        }
        return Ok(0);
    }
    let mut changed = 0i64;
    if sidecar.is_file() {
        std::fs::remove_file(&sidecar)
            .map_err(|error| crate::json::os_error(&error, &sidecar))?;
        changed += 1;
    }
    let written = if executable {
        crate::transcript_io::write_text_if_changed_executable(path, text)?
    } else {
        write_text_if_changed(path, text)?
    };
    changed += i64::from(written);
    Ok(changed)
}

/// Store text as `<path>.gz` and nothing else.
pub fn write_gzip_only(path: &Path, text: &str) -> Result<i64, String> {
    let sidecar = gzip_sidecar_path(path);
    if is_symlink(path) || is_symlink(&sidecar) {
        return Err(format!(
            "refusing unsafe gzip-only target: {}",
            path.display()
        ));
    }
    if sidecar.exists() && !sidecar.is_file() {
        return Err(format!(
            "refusing unsafe gzip sidecar: {}",
            sidecar.display()
        ));
    }
    let mut changed = 0i64;
    let body = deterministic_gzip(text.as_bytes());
    if !sidecar.is_file()
        || std::fs::read(&sidecar).map_err(|error| crate::json::os_error(&error, &sidecar))?
            != body
    {
        crate::transcript_io::atomic_write(&sidecar, &body)?;
        changed += 1;
    }
    if path.is_file() {
        std::fs::remove_file(path).map_err(|error| crate::json::os_error(&error, path))?;
        changed += 1;
    }
    Ok(changed)
}

/// Create, refresh, or remove the deterministic `.gz` companion.
pub fn sync_gzip_sidecar(path: &Path) -> Result<bool, String> {
    sync_gzip_sidecar_min(path, GZIP_MINIMUM_BYTES)
}

/// Minimum-bytes variant.
pub fn sync_gzip_sidecar_min(path: &Path, minimum_bytes: u64) -> Result<bool, String> {
    let sidecar = gzip_sidecar_path(path);
    if is_symlink(&sidecar) {
        return Err(format!(
            "refusing unsafe gzip sidecar: {}",
            sidecar.display()
        ));
    }
    let too_small = !path.is_file()
        || std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0) < minimum_bytes;
    if too_small {
        if !sidecar.exists() {
            return Ok(false);
        }
        if !sidecar.is_file() {
            return Err(format!(
                "refusing unsafe gzip sidecar: {}",
                sidecar.display()
            ));
        }
        std::fs::remove_file(&sidecar)
            .map_err(|error| crate::json::os_error(&error, &sidecar))?;
        return Ok(true);
    }
    if is_symlink(path) {
        return Err(format!(
            "refusing to compress symlinked static asset: {}",
            path.display()
        ));
    }
    let data = std::fs::read(path).map_err(|error| crate::json::os_error(&error, path))?;
    let body = deterministic_gzip(&data);
    if sidecar.is_file()
        && std::fs::read(&sidecar).map_err(|error| crate::json::os_error(&error, &sidecar))?
            == body
    {
        return Ok(false);
    }
    crate::transcript_io::atomic_write(&sidecar, &body)?;
    Ok(true)
}

/// A detected glossary term (terminology.py shape).
#[derive(Clone, Debug)]
pub struct GlossaryTerm {
    /// Exact term.
    pub term: String,
    /// Introduced at ms.
    pub introduced_at_ms: i64,
    /// Occurrences.
    pub occurrences: i64,
    /// Context.
    pub context: String,
    /// Week key.
    pub week: String,
    /// Term id.
    pub term_id: String,
    /// Definition.
    pub definition: String,
    /// Definition status.
    pub definition_status: String,
    /// Available at ms override.
    pub available_at_ms: Option<i64>,
}

impl GlossaryTerm {
    /// When this term may first affect summaries.
    pub fn summary_available_at_ms(&self) -> i64 {
        self.available_at_ms.unwrap_or(self.introduced_at_ms)
    }
}

fn iso_ms(at_ms: i64) -> String {
    use chrono::TimeZone as _;
    let dt = chrono::Utc.timestamp_millis_opt(at_ms).single().unwrap();
    if dt.timestamp_subsec_micros() == 0 {
        dt.format("%Y-%m-%dT%H:%M:%SZ").to_string()
    } else {
        dt.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string()
    }
}

/// Stable URL-safe ID for an exact glossary name.
pub fn glossary_term_id(term: &str) -> Result<String, String> {
    let clean = py_trim(term);
    if clean.is_empty() {
        return Err("glossary term must not be empty".to_string());
    }
    let lowered = clean.to_lowercase();
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in lowered.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    let mut slug = slug.trim_matches('-').to_string();
    slug = char_take(&slug, 48).trim_end_matches('-').to_string();
    if slug.is_empty() {
        slug = "term".to_string();
    }
    let digest: String = sha2::Sha256::digest(clean.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
        .chars()
        .take(12)
        .collect();
    Ok(format!("term-{slug}-{digest}"))
}

fn definition_text(term: &GlossaryTerm) -> &str {
    if !term.definition.is_empty() {
        &term.definition
    } else {
        "No model-backed definition is available; consult the first-use evidence below."
    }
}

fn rstrip(text: &str) -> String {
    text.trim_end().to_string()
}

/// Render one weekly glossary file.
pub fn glossary_markdown(team_slug: &str, week: &str, terms: &[GlossaryTerm]) -> String {
    let mut lines: Vec<String> = vec![
        format!("# {week} {team_slug} terminology"),
        String::new(),
        "Terms are ordered by first appearance. Definitions are generated only from retained".into(),
        "source occurrences; first-use evidence is shown separately.".into(),
        String::new(),
    ];
    let matching: Vec<&GlossaryTerm> = terms.iter().filter(|term| term.week == week).collect();
    if matching.is_empty() {
        lines.push("_No new stable terms were introduced in this week._".into());
    }
    for term in matching {
        lines.extend([
            format!("## {}", term.term),
            String::new(),
            format!("Stable glossary ID: `{}`  ", term.term_id),
            format!(
                "Introduced {}; seen {} time(s).",
                iso_ms(term.introduced_at_ms),
                term.occurrences
            ),
            String::new(),
            "### Definition".into(),
            String::new(),
            definition_text(term).to_string(),
            String::new(),
            "### First-use evidence".into(),
            String::new(),
            format!("> {}", term.context),
            String::new(),
        ]);
    }
    format!("{}\n", rstrip(&lines.join("\n")))
}

/// Render the all-time glossary catalog.
pub fn glossary_catalog_markdown(
    team_slug: &str,
    terms: &[GlossaryTerm],
    project_overview: &str,
) -> String {
    let mut lines: Vec<String> = vec![
        format!("# {team_slug} project glossary"),
        String::new(),
        "## Project overview".into(),
        String::new(),
        {
            let trimmed = py_trim(project_overview);
            if trimmed.is_empty() {
                "No model-backed project overview is available for this archive.".to_string()
            } else {
                trimmed.to_string()
            }
        },
        String::new(),
        "## Project terms".into(),
        String::new(),
        "These source-bounded terms are ordered by first appearance. In the website, recognized".into(),
        "uses of an exact term link to its stable `#glossary/<term-id>` entry. Definitions and".into(),
        "their first-use evidence are deliberately separate.".into(),
        String::new(),
    ];
    if terms.is_empty() {
        lines.push("_No supported semantic project concepts are available._".into());
    }
    for term in terms {
        lines.extend([
            format!("## {}", term.term),
            String::new(),
            format!("Stable glossary ID: `{}`  ", term.term_id),
            format!(
                "Introduced {} in {}; seen {} time(s).",
                iso_ms(term.introduced_at_ms),
                term.week,
                term.occurrences
            ),
            String::new(),
            "### Definition".into(),
            String::new(),
            definition_text(term).to_string(),
            String::new(),
            "### First-use evidence".into(),
            String::new(),
            format!("> {}", term.context),
            String::new(),
        ]);
    }
    format!("{}\n", rstrip(&lines.join("\n")))
}

/// One recognized pull-request reference.
#[derive(Clone, Debug)]
pub struct PullRequestReference {
    /// Start char index.
    pub start: usize,
    /// End char index.
    pub end: usize,
    /// Matched text.
    pub text: String,
    /// Kind value.
    pub kind: &'static str,
    /// Repository slug.
    pub repository: String,
    /// Number.
    pub number: i64,
}

impl PullRequestReference {
    /// Canonical pull URL.
    pub fn url(&self) -> String {
        format!(
            "https://github.com/{}/pull/{}",
            self.repository, self.number
        )
    }
}

const OWNER_PATTERN: &str = r"[A-Za-z0-9](?:[A-Za-z0-9-]{0,37}[A-Za-z0-9])?";
const REPO_PATTERN: &str = r"[A-Za-z0-9_.-]{1,100}";

fn char_at(text: &str, char_index: usize) -> Option<char> {
    text.chars().nth(char_index)
}

fn find_references(
    text: &str,
    pattern: &str,
    kind: &'static str,
    boundary_ok: fn(Option<char>) -> bool,
    tail_ok: fn(Option<char>) -> bool,
) -> Vec<PullRequestReference> {
    let finder = regex::Regex::new(pattern).unwrap();
    let total_chars = char_len(text);
    let byte_to_char = |byte: usize| text[..byte].chars().count();
    let mut out = Vec::new();
    for captures in finder.captures_iter(text) {
        let reference = captures.name("reference").unwrap();
        let start_char = byte_to_char(reference.start());
        let end_char = byte_to_char(reference.end());
        let before = if start_char == 0 {
            None
        } else {
            char_at(text, start_char - 1)
        };
        if !boundary_ok(before) {
            continue;
        }
        let after = if end_char >= total_chars {
            None
        } else {
            char_at(text, end_char)
        };
        if !tail_ok(after) {
            continue;
        }
        let owner = captures.name("owner").unwrap().as_str().to_string();
        let repository = captures.name("repository").unwrap().as_str().to_string();
        let number: i64 = captures
            .name("number")
            .unwrap()
            .as_str()
            .parse()
            .unwrap_or(0);
        out.push(PullRequestReference {
            start: start_char,
            end: end_char,
            text: reference.as_str().to_string(),
            kind,
            repository: format!("{owner}/{repository}"),
            number,
        });
    }
    out
}

/// Find conservatively linkable pull-request references (no context).
pub fn find_pull_request_references(text: &str) -> Vec<PullRequestReference> {
    let explicit = find_references(
        text,
        &format!(
            r"(?i)(?P<reference>https://github\.com/(?P<owner>{OWNER_PATTERN})/(?P<repository>{REPO_PATTERN})/pull/(?P<number>[1-9][0-9]*))"
        ),
        "explicit_url",
        |before| !matches!(before, Some(ch) if ch.is_ascii_alphanumeric()),
        |after| {
            after.is_none()
                || matches!(after, Some(ch) if matches!(ch, '/' | '?' | '#' | ')' | ']' | '}' | '>' | ',' | '.' | ';' | ':' | '!') || ch.is_whitespace())
        },
    );
    let qualified = find_references(
        text,
        &format!(
            r"(?P<reference>(?P<owner>{OWNER_PATTERN})/(?P<repository>{REPO_PATTERN})#(?P<number>[1-9][0-9]*))"
        ),
        "qualified",
        |before| {
            !matches!(before, Some(ch) if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '/' | '-'))
        },
        |after| !matches!(after, Some(ch) if ch.is_ascii_digit()),
    );
    let mut all = explicit;
    all.extend(qualified);
    all.sort_by_key(|item| (item.start, item.end));
    all
}

/// The bounded pull metadata the UI renders.
#[derive(Clone, Debug)]
pub struct PullRequestMetadata {
    /// Repository slug.
    pub repository: String,
    /// Number.
    pub number: i64,
    /// Title.
    pub title: String,
    /// State.
    pub state: String,
    /// Draft.
    pub draft: bool,
    /// Merged at.
    pub merged_at: Option<String>,
    /// Body excerpt.
    pub body_excerpt: String,
    /// Base ref.
    pub base_ref: String,
    /// Head label.
    pub head_label: String,
    /// Author.
    pub author: Option<String>,
    /// Updated at.
    pub updated_at: String,
}
