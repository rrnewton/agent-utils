//! Bounded GitHub pull-request metadata: cache records, the
//! `gh api --include` transport, and conditional fetches. The Rust
//! mirror of `wrkviz.github_metadata` + `wrkviz.gh_resolve`.

use crate::json::{as_object, as_string, read_json};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

const SCHEMA_VERSION: i64 = 1;
const MAX_API_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const MAX_BODY_EXCERPT_CHARS: usize = 600;
const MAX_ETAG_CHARS: usize = 1024;
/// Default per-request timeout in seconds.
pub const DEFAULT_TIMEOUT_SECONDS: f64 = 15.0;

/// A validated repository-and-number cache key.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PullRequestKey {
    /// `owner/repository`.
    pub repository: String,
    /// Pull number.
    pub number: i64,
}

fn valid_repository(repository: &str) -> bool {
    let Some((owner, name)) = repository.split_once('/') else {
        return false;
    };
    if name.contains('/') || name == "." || name == ".." {
        return false;
    }
    let owner_ok = !owner.is_empty()
        && owner.len() <= 39
        && owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && !owner.starts_with('-')
        && !owner.ends_with('-');
    let name_ok = !name.is_empty()
        && name.len() <= 100
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
    owner_ok && name_ok
}

impl PullRequestKey {
    /// Validate and construct a key.
    pub fn new(repository: &str, number: i64) -> Result<PullRequestKey, String> {
        if !valid_repository(repository) {
            return Err(format!(
                "GitHub repository must be an owner/repository slug: '{repository}'"
            ));
        }
        if number <= 0 {
            return Err("a GitHub pull-request number must be positive".into());
        }
        Ok(PullRequestKey {
            repository: repository.to_string(),
            number,
        })
    }

    /// Stable spelling used as a JSON object key.
    pub fn cache_key(&self) -> String {
        format!("{}#{}", self.repository, self.number)
    }

    /// The sole GitHub API endpoint used for this key.
    pub fn api_url(&self) -> String {
        format!(
            "https://api.github.com/repos/{}/pulls/{}",
            self.repository, self.number
        )
    }
}

fn valid_header_value(value: &str, name: &str, maximum: usize) -> Result<String, String> {
    if value.is_empty() || value.chars().count() > maximum || value.contains(['\r', '\n']) {
        return Err(format!("invalid {name}"));
    }
    Ok(value.to_string())
}

/// The bounded subset of a GitHub pull response needed by the UI.
#[derive(Clone, Debug)]
pub struct PullRequestMetadata {
    /// Key.
    pub key: PullRequestKey,
    /// Title.
    pub title: String,
    /// State.
    pub state: String,
    /// Draft.
    pub draft: bool,
    /// Merged timestamp.
    pub merged_at: Option<String>,
    /// Body excerpt.
    pub body_excerpt: String,
    /// Base ref.
    pub base_ref: String,
    /// Head label.
    pub head_label: String,
    /// Author login.
    pub author: Option<String>,
    /// Updated timestamp.
    pub updated_at: String,
    /// ETag.
    pub etag: Option<String>,
    /// Fetched timestamp.
    pub fetched_at: String,
}

impl PullRequestMetadata {
    fn validate(&self) -> Result<(), String> {
        if self.title.is_empty() {
            return Err("pull-request title must not be empty".into());
        }
        if self.state != "open" && self.state != "closed" {
            return Err("pull-request state must be 'open' or 'closed'".into());
        }
        if self.body_excerpt.chars().count() > MAX_BODY_EXCERPT_CHARS {
            return Err("pull-request body excerpt is too long".into());
        }
        for (name, value) in [
            ("base_ref", &self.base_ref),
            ("head_label", &self.head_label),
            ("updated_at", &self.updated_at),
            ("fetched_at", &self.fetched_at),
        ] {
            if value.is_empty() {
                return Err(format!("pull-request {name} must not be empty"));
            }
        }
        if let Some(etag) = &self.etag {
            valid_header_value(etag, "ETag", MAX_ETAG_CHARS)?;
        }
        Ok(())
    }

    /// JSON form; credentials cannot appear.
    pub fn to_json_obj(&self) -> Value {
        json!({
            "repository": self.key.repository,
            "number": self.key.number,
            "title": self.title,
            "state": self.state,
            "draft": self.draft,
            "merged_at": self.merged_at,
            "body_excerpt": self.body_excerpt,
            "base_ref": self.base_ref,
            "head_label": self.head_label,
            "author": self.author,
            "updated_at": self.updated_at,
            "etag": self.etag,
            "fetched_at": self.fetched_at,
        })
    }

    /// Load one record, narrowing every untyped JSON value.
    pub fn from_json_obj(value: &Value, where_: &str) -> Result<PullRequestMetadata, String> {
        let item = as_object(value, where_)?;
        exact_keys(
            item,
            &[
                "repository",
                "number",
                "title",
                "state",
                "draft",
                "merged_at",
                "body_excerpt",
                "base_ref",
                "head_label",
                "author",
                "updated_at",
                "etag",
                "fetched_at",
            ],
            where_,
        )?;
        let record = PullRequestMetadata {
            key: PullRequestKey::new(
                req_string(item, "repository", where_)?,
                req_integer(item, "number", where_)?,
            )?,
            title: req_string(item, "title", where_)?.to_string(),
            state: req_string(item, "state", where_)?.to_string(),
            draft: req_boolean(item, "draft", where_)?,
            merged_at: opt_string(item, "merged_at", where_)?,
            body_excerpt: req_string(item, "body_excerpt", where_)?.to_string(),
            base_ref: req_string(item, "base_ref", where_)?.to_string(),
            head_label: req_string(item, "head_label", where_)?.to_string(),
            author: opt_string(item, "author", where_)?,
            updated_at: req_string(item, "updated_at", where_)?.to_string(),
            etag: opt_string(item, "etag", where_)?,
            fetched_at: req_string(item, "fetched_at", where_)?.to_string(),
        };
        record.validate()?;
        Ok(record)
    }
}

fn field<'a>(item: &'a Map<String, Value>, name: &str) -> &'a Value {
    item.get(name).unwrap_or(&Value::Null)
}

fn req_string<'a>(
    item: &'a Map<String, Value>,
    name: &str,
    where_: &str,
) -> Result<&'a str, String> {
    as_string(field(item, name), &format!("{where_}.{name}"))
}

fn opt_string(
    item: &Map<String, Value>,
    name: &str,
    where_: &str,
) -> Result<Option<String>, String> {
    let value = field(item, name);
    if value.is_null() {
        return Ok(None);
    }
    Ok(Some(
        as_string(value, &format!("{where_}.{name}"))?.to_string(),
    ))
}

fn req_integer(item: &Map<String, Value>, name: &str, where_: &str) -> Result<i64, String> {
    let value = field(item, name);
    value
        .as_i64()
        .filter(|_| !value.is_f64() || value.as_f64() == value.as_i64().map(|n| n as f64))
        .ok_or_else(|| format!("{where_}.{name}: expected an integer"))
}

fn req_boolean(item: &Map<String, Value>, name: &str, where_: &str) -> Result<bool, String> {
    field(item, name)
        .as_bool()
        .ok_or_else(|| format!("{where_}.{name}: expected a boolean"))
}

fn exact_keys(item: &Map<String, Value>, expected: &[&str], where_: &str) -> Result<(), String> {
    let actual: Vec<&str> = item.keys().map(String::as_str).collect();
    let missing: Vec<&str> = expected
        .iter()
        .filter(|name| !actual.contains(name))
        .copied()
        .collect();
    let extra: Vec<&str> = actual
        .iter()
        .filter(|name| !expected.contains(name))
        .copied()
        .collect();
    if missing.is_empty() && extra.is_empty() {
        return Ok(());
    }
    let mut details = Vec::new();
    if !missing.is_empty() {
        details.push(format!("missing {}", missing.join(", ")));
    }
    if !extra.is_empty() {
        details.push(format!("extra {}", extra.join(", ")));
    }
    Err(format!("{where_}: {}", details.join("; ")))
}

/// In-memory records keyed by pull-request key.
#[derive(Default)]
pub struct PullRequestMetadataCache {
    records: BTreeMap<PullRequestKey, PullRequestMetadata>,
}

impl PullRequestMetadataCache {
    /// Cached record for a key.
    pub fn get(&self, key: &PullRequestKey) -> Option<&PullRequestMetadata> {
        self.records.get(key)
    }

    /// Insert or replace one record.
    pub fn put(&mut self, record: PullRequestMetadata) {
        self.records.insert(record.key.clone(), record);
    }

    /// Complete versioned cache as JSON.
    pub fn to_json_obj(&self) -> Value {
        let mut records = Map::new();
        for record in self.records.values() {
            records.insert(record.key.cache_key(), record.to_json_obj());
        }
        json!({"schema_version": SCHEMA_VERSION, "records": records})
    }

    /// Validate and load a complete cache object.
    pub fn from_json_obj(value: &Value) -> Result<PullRequestMetadataCache, String> {
        let root = as_object(value, "GitHub metadata cache")?;
        exact_keys(
            root,
            &["schema_version", "records"],
            "GitHub metadata cache",
        )?;
        let version = req_integer(root, "schema_version", "GitHub metadata cache")?;
        if version != SCHEMA_VERSION {
            return Err(format!(
                "unsupported GitHub metadata cache schema {version}"
            ));
        }
        let raw = as_object(field(root, "records"), "GitHub metadata cache.records")?;
        let mut keys: Vec<&String> = raw.keys().collect();
        keys.sort();
        let mut cache = PullRequestMetadataCache::default();
        for cache_key in keys {
            let record = PullRequestMetadata::from_json_obj(
                &raw[cache_key],
                &format!("GitHub metadata cache.records['{cache_key}']"),
            )?;
            if *cache_key != record.key.cache_key() {
                return Err(format!(
                    "GitHub metadata cache key does not match its record: '{cache_key}' != '{}'",
                    record.key.cache_key()
                ));
            }
            cache.put(record);
        }
        Ok(cache)
    }
}

/// Load a cache; an absent file is an empty first run.
pub fn load_pull_request_metadata_cache(path: &Path) -> Result<PullRequestMetadataCache, String> {
    if !path.is_file() {
        return Ok(PullRequestMetadataCache::default());
    }
    PullRequestMetadataCache::from_json_obj(&read_json(path)?)
}

/// Atomically save deterministic JSON; return whether bytes changed.
pub fn save_pull_request_metadata_cache(
    path: &Path,
    cache: &PullRequestMetadataCache,
) -> Result<bool, String> {
    crate::transcript_io::write_json_if_changed(path, &cache.to_json_obj())
}

/// Transport-neutral HTTP response.
pub struct HttpResponse {
    /// Status code.
    pub status: i64,
    /// Lower-cased headers.
    pub headers: BTreeMap<String, String>,
    /// Body bytes.
    pub body: Vec<u8>,
}

fn validate_api_url(url: &str) -> Result<String, String> {
    let path = url
        .strip_prefix("https://api.github.com")
        .filter(|rest| !rest.contains(['?', '#', '@']))
        .ok_or_else(|| {
            "GitHub metadata requests must use https://api.github.com/repos/".to_string()
        })?;
    let parts: Vec<&str> = path.split('/').collect();
    let ok = parts.len() == 6
        && parts[1] == "repos"
        && valid_repository(&format!("{}/{}", parts[2], parts[3]))
        && parts[4] == "pulls"
        && !parts[5].is_empty()
        && parts[5].chars().all(|c| c.is_ascii_digit())
        && !parts[5].starts_with('0');
    if !ok {
        return Err("GitHub metadata requests must use https://api.github.com/repos/".to_string());
    }
    Ok(path.to_string())
}

fn which(program: &str, path_env: &str) -> Option<PathBuf> {
    for directory in path_env.split(':') {
        if directory.is_empty() {
            continue;
        }
        let candidate = Path::new(directory).join(program);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::metadata(path)
            .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

fn source_root() -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent()?.parent()?;
    if root.join("rs/gh-paced/Cargo.toml").is_file() {
        return Some(root.to_path_buf());
    }
    None
}

fn built_gh_paced(root: &Path) -> Option<PathBuf> {
    let target = root.join("rs/target");
    let direct = target.join("release/gh-paced");
    if is_executable(&direct) {
        return Some(direct);
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&target) {
        for entry in entries.flatten() {
            candidates.push(entry.path().join("release/gh-paced"));
        }
    }
    candidates.sort();
    candidates.into_iter().find(|path| is_executable(path))
}

fn wraps_gh_paced(path: &Path) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return true;
    };
    let mut head = vec![0u8; 64 * 1024];
    let read = file.read(&mut head).unwrap_or(0);
    head.truncate(read);
    head.starts_with(b"#!") && head.windows(8).any(|window| window == b"gh-paced")
}

fn real_gh(path_env: &str, paced: &Path) -> Option<String> {
    if let Ok(configured) = std::env::var("GH_PACED_REAL_GH") {
        let configured = configured.trim();
        if !configured.is_empty() {
            return Some(configured.to_string());
        }
    }
    for directory in path_env.split(':') {
        if directory.is_empty() {
            continue;
        }
        let candidate = Path::new(directory).join("gh");
        if !is_executable(&candidate) {
            continue;
        }
        let resolved = std::fs::canonicalize(&candidate).unwrap_or(candidate.clone());
        let paced_resolved = std::fs::canonicalize(paced).unwrap_or_else(|_| paced.to_path_buf());
        if resolved == paced_resolved || wraps_gh_paced(&resolved) {
            continue;
        }
        return Some(resolved.display().to_string());
    }
    None
}

fn active_login(real: &str) -> Option<String> {
    let done = std::process::Command::new(real)
        .args(["config", "get", "user", "-h", "github.com"])
        .output()
        .ok()?;
    let login = String::from_utf8_lossy(&done.stdout).trim().to_string();
    if done.status.success() && !login.is_empty() {
        return Some(login);
    }
    None
}

fn split_command(text: &str) -> Vec<String> {
    // Shell-word split sufficient for resolver overrides (quotes honored).
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for ch in text.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
        } else if ch == '\\' && quote != Some('\'') {
            escaped = true;
        } else if let Some(active) = quote {
            if ch == active {
                quote = None;
            } else {
                current.push(ch);
            }
        } else if ch == '\'' || ch == '"' {
            quote = Some(ch);
        } else if ch.is_whitespace() {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Choose the command run as the GitHub CLI (gh-paced preferred).
pub fn resolve_gh() -> Result<Vec<String>, String> {
    if let Ok(override_) = std::env::var("AGENT_UTILS_GH") {
        let override_ = override_.trim();
        if !override_.is_empty() {
            let command = split_command(override_);
            if command.is_empty() {
                return Err("AGENT_UTILS_GH names no command".into());
            }
            return Ok(command);
        }
    }
    let path_env = std::env::var("PATH").unwrap_or_default();
    let paced = source_root()
        .and_then(|root| built_gh_paced(&root))
        .or_else(|| which("gh-paced", &path_env));
    if let Some(paced) = paced {
        let real = real_gh(&path_env, &paced);
        let account = std::env::var("GH_PACED_ACCOUNT")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .or_else(|| real.as_deref().and_then(active_login));
        let Some(account) = account else {
            return Err(format!(
                "gh-paced is available ({}) but no GitHub login to pace was found; set GH_PACED_ACCOUNT, sign the real gh in, or set AGENT_UTILS_GH=gh to run GitHub unpaced",
                paced.display()
            ));
        };
        let mut command = vec![paced.display().to_string(), "--account".into(), account];
        if let Some(real) = real {
            command.push("--real-gh".into());
            command.push(real);
        }
        command.push("--".into());
        return Ok(command);
    }
    Ok(vec!["gh".into()])
}

/// Fetch through `gh api --include`; gh supplies the credential.
pub fn gh_api_get(
    url: &str,
    headers: &[(String, String)],
    timeout_seconds: f64,
) -> Result<HttpResponse, String> {
    let path = validate_api_url(url)?;
    let command = resolve_gh()?;
    let mut argv: Vec<String> = command;
    argv.extend([
        "api".into(),
        "--include".into(),
        "--method".into(),
        "GET".into(),
    ]);
    for (name, value) in headers {
        if name.eq_ignore_ascii_case("authorization") {
            return Err(
                "gh supplies the GitHub credential; refusing to forward a Bearer credential".into(),
            );
        }
        argv.push("-H".into());
        argv.push(format!("{name}: {value}"));
    }
    argv.push(path.trim_start_matches('/').to_string());
    let mut child = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| format!("gh api could not run: {error}"))?;
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs_f64(timeout_seconds.max(0.0));
    let output = loop {
        match child.try_wait() {
            Ok(Some(_)) => break child.wait_with_output(),
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("gh api timed out".into());
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(error) => return Err(format!("gh api could not run: {error}")),
        }
    }
    .map_err(|error| format!("gh api could not run: {error}"))?;
    if output.stdout.len() > MAX_API_RESPONSE_BYTES + 64 * 1024 {
        return Err("GitHub API response exceeds size limit".into());
    }
    let Some(response) = parse_included_response(&output.stdout) else {
        let detail: String = String::from_utf8_lossy(&output.stderr)
            .trim()
            .chars()
            .take(300)
            .collect();
        return Err(format!(
            "gh api failed (exit {}): {}",
            output.status.code().unwrap_or(-1),
            if detail.is_empty() {
                "no response"
            } else {
                &detail
            }
        ));
    };
    if response.body.len() > MAX_API_RESPONSE_BYTES {
        return Err("GitHub API response exceeds size limit".into());
    }
    Ok(response)
}

fn parse_included_response(output: &[u8]) -> Option<HttpResponse> {
    let text = String::from_utf8_lossy(output);
    let mut lines = text.split_inclusive('\n');
    let status_line = lines.next()?.trim_end_matches(['\r', '\n']);
    let status = parse_status_line(status_line)?;
    let mut headers = BTreeMap::new();
    let mut body_start = status_line.len() + 1;
    for line in lines {
        body_start += line.len();
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        let (name, value) = trimmed.split_once(':')?;
        headers.insert(name.trim().to_lowercase(), value.trim().to_string());
    }
    if body_start > output.len() {
        return None;
    }
    Some(HttpResponse {
        status,
        headers,
        body: output[body_start..].to_vec(),
    })
}

fn parse_status_line(line: &str) -> Option<i64> {
    let rest = line.strip_prefix("HTTP/")?;
    let (version, tail) = rest.split_once(' ')?;
    if version.is_empty() || !version.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return None;
    }
    let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.len() != 3 {
        return None;
    }
    let status: i64 = digits.parse().ok()?;
    if !(100..=599).contains(&status) {
        return None;
    }
    Some(status)
}

/// A fetched record plus whether GitHub returned HTTP 304.
pub struct PullRequestFetchResult {
    /// Record.
    pub metadata: PullRequestMetadata,
    /// HTTP 304.
    pub not_modified: bool,
}

/// The transport seam: fetch one URL.
pub trait HttpTransport {
    /// Fetch one URL without following it to another host.
    fn get(
        &self,
        url: &str,
        headers: &[(String, String)],
        timeout_seconds: f64,
    ) -> Result<HttpResponse, String>;
}

/// Default transport through the gh CLI.
pub struct GhApiTransport;
impl HttpTransport for GhApiTransport {
    fn get(
        &self,
        url: &str,
        headers: &[(String, String)],
        timeout_seconds: f64,
    ) -> Result<HttpResponse, String> {
        gh_api_get(url, headers, timeout_seconds)
    }
}

/// Fetch one PR, conditionally using a cached ETag when available.
pub fn fetch_pull_request(
    key: &PullRequestKey,
    cached: Option<&PullRequestMetadata>,
    fetched_at: &str,
    timeout_seconds: f64,
    transport: &dyn HttpTransport,
) -> Result<PullRequestFetchResult, String> {
    if let Some(cached) = cached {
        if cached.key != *key {
            return Err("cached pull-request metadata has a different key".into());
        }
    }
    if !timeout_seconds.is_finite() || timeout_seconds <= 0.0 {
        return Err("HTTP timeout must be finite and positive".into());
    }
    if fetched_at.is_empty() {
        return Err("fetched_at must not be empty".into());
    }
    let mut headers: Vec<(String, String)> = vec![
        ("Accept".into(), "application/vnd.github+json".into()),
        ("User-Agent".into(), "wrkviz".into()),
        ("X-GitHub-Api-Version".into(), "2022-11-28".into()),
    ];
    if let Some(cached) = cached {
        if let Some(etag) = &cached.etag {
            headers.push(("If-None-Match".into(), etag.clone()));
        }
    }
    let url = key.api_url();
    validate_api_url(&url)?;
    let response = transport.get(&url, &headers, timeout_seconds)?;
    if response.body.len() > MAX_API_RESPONSE_BYTES {
        return Err("GitHub API response exceeds size limit".into());
    }
    if response.status == 304 {
        let Some(cached) = cached else {
            return Err("GitHub returned 304 without a cached record".into());
        };
        return Ok(PullRequestFetchResult {
            metadata: PullRequestMetadata {
                key: cached.key.clone(),
                title: cached.title.clone(),
                state: cached.state.clone(),
                draft: cached.draft,
                merged_at: cached.merged_at.clone(),
                body_excerpt: cached.body_excerpt.clone(),
                base_ref: cached.base_ref.clone(),
                head_label: cached.head_label.clone(),
                author: cached.author.clone(),
                updated_at: cached.updated_at.clone(),
                etag: cached.etag.clone(),
                fetched_at: cached.fetched_at.clone(),
            },
            not_modified: true,
        });
    }
    if response.status != 200 {
        return Err(format!("GitHub API returned HTTP {}", response.status));
    }
    let metadata = metadata_from_response(key, &response, fetched_at)
        .map_err(|_| "GitHub API returned malformed pull metadata".to_string())?;
    Ok(PullRequestFetchResult {
        metadata,
        not_modified: false,
    })
}

fn metadata_from_response(
    key: &PullRequestKey,
    response: &HttpResponse,
    fetched_at: &str,
) -> Result<PullRequestMetadata, String> {
    let text = std::str::from_utf8(&response.body)
        .map_err(|_| "GitHub API returned invalid JSON".to_string())?;
    let raw: Value =
        serde_json::from_str(text).map_err(|_| "GitHub API returned invalid JSON".to_string())?;
    let root = as_object(&raw, "GitHub pull response")?;
    let body_value = field(root, "body");
    let body = if body_value.is_null() {
        String::new()
    } else {
        as_string(body_value, "GitHub pull response.body")?.to_string()
    };
    let base = as_object(field(root, "base"), "GitHub pull response.base")?;
    let head = as_object(field(root, "head"), "GitHub pull response.head")?;
    let user_value = field(root, "user");
    let author = if user_value.is_null() {
        None
    } else {
        let user = as_object(user_value, "GitHub pull response.user")?;
        Some(as_string(field(user, "login"), "GitHub pull response.user.login")?.to_string())
    };
    let etag = response
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("etag"))
        .map(|(_, value)| valid_header_value(value, "etag", MAX_ETAG_CHARS))
        .transpose()?;
    let record = PullRequestMetadata {
        key: key.clone(),
        title: as_string(field(root, "title"), "GitHub pull response.title")?.to_string(),
        state: as_string(field(root, "state"), "GitHub pull response.state")?.to_string(),
        draft: field(root, "draft")
            .as_bool()
            .ok_or("GitHub pull response.draft: expected a boolean")?,
        merged_at: opt_string(root, "merged_at", "GitHub pull response")?,
        body_excerpt: body.chars().take(MAX_BODY_EXCERPT_CHARS).collect(),
        base_ref: as_string(field(base, "ref"), "GitHub pull response.base.ref")?.to_string(),
        head_label: as_string(field(head, "label"), "GitHub pull response.head.label")?.to_string(),
        author,
        updated_at: as_string(field(root, "updated_at"), "GitHub pull response.updated_at")?
            .to_string(),
        etag,
        fetched_at: fetched_at.to_string(),
    };
    record.validate()?;
    Ok(record)
}
