//! Evidence-bounded project and execution-host identity:
//! `py/wrkviz/identity.py` at parity.

use serde_json::{Map, Value};

use crate::json::{as_array, as_object, as_string, json_eq_one, py_repr_str};

/// One repository/project associated with an agent team.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectIdentity {
    /// Display label.
    pub label: String,
    /// Canonical repository URL.
    pub repository_url: Option<String>,
    /// Whether this is the primary project.
    pub primary: bool,
    /// Where the identity came from.
    pub source: String,
}

impl ProjectIdentity {
    /// The JSON record.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("label".into(), Value::String(self.label.clone()));
        out.insert(
            "repository_url".into(),
            self.repository_url
                .clone()
                .map_or(Value::Null, Value::String),
        );
        out.insert("primary".into(), Value::Bool(self.primary));
        out.insert("source".into(), Value::String(self.source.clone()));
        Value::Object(out)
    }
}

/// One machine on which an agent team ran.
#[derive(Clone, Debug, PartialEq)]
pub struct HostIdentity {
    /// Validated hostname.
    pub hostname: String,
    /// Where the identity came from.
    pub source: String,
}

impl HostIdentity {
    /// The JSON record.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("hostname".into(), Value::String(self.hostname.clone()));
        out.insert("source".into(), Value::String(self.source.clone()));
        Value::Object(out)
    }
}

/// Durable display identity kept separately from normalization.
#[derive(Clone, Debug, PartialEq)]
pub struct SiteIdentity {
    /// Team slug.
    pub team_slug: String,
    /// Projects.
    pub projects: Vec<ProjectIdentity>,
    /// Hosts.
    pub hosts: Vec<HostIdentity>,
    /// Display timezone.
    pub display_timezone: String,
    /// Timezone provenance.
    pub display_timezone_source: String,
}

impl SiteIdentity {
    /// The versioned JSON record.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("schema_version".into(), 1.into());
        out.insert("team_slug".into(), Value::String(self.team_slug.clone()));
        out.insert(
            "projects".into(),
            Value::Array(self.projects.iter().map(ProjectIdentity::to_json).collect()),
        );
        out.insert(
            "hosts".into(),
            Value::Array(self.hosts.iter().map(HostIdentity::to_json).collect()),
        );
        out.insert(
            "display_timezone".into(),
            Value::String(self.display_timezone.clone()),
        );
        out.insert(
            "display_timezone_source".into(),
            Value::String(self.display_timezone_source.clone()),
        );
        Value::Object(out)
    }
}

/// Explicit ingest identity values and timezone provenance.
#[derive(Clone, Debug, Default)]
pub struct IdentityOverrides {
    /// Explicit projects.
    pub projects: Vec<ProjectIdentity>,
    /// Explicit hosts.
    pub hosts: Vec<HostIdentity>,
    /// Timezone provenance.
    pub display_timezone_source: String,
}

fn valid_tz(name: &str) -> bool {
    name.parse::<chrono_tz::Tz>().is_ok()
}

/// Split a URL into (scheme, authority, path); `None` when it
/// has no `://`.
fn split_url(value: &str) -> Option<(String, String, String)> {
    let (scheme, rest) = value.split_once("://")?;
    let (authority, path) = match rest.find('/') {
        Some(index) => (rest[..index].to_string(), rest[index..].to_string()),
        None => (rest.to_string(), String::new()),
    };
    Some((scheme.to_lowercase(), authority, path))
}

/// Normalize an HTTP(S) or Git SCP-style repository remote to
/// a browser URL.
pub fn canonical_repository_url(raw: &str) -> Result<String, String> {
    let mut value = raw.trim().to_string();
    // SCP-style: [user@]host:path with no scheme.
    if !value.contains("://") {
        if let Some((host, path)) = scp_parts(&value) {
            value = format!("https://{host}/{path}");
        }
    }
    let Some((scheme, authority, mut path)) = split_url(&value) else {
        return Err(format!(
            "repository URL is not an HTTP(S) or Git SSH remote: {}",
            py_repr_str(raw)
        ));
    };
    if (scheme != "http" && scheme != "https") || authority.is_empty() {
        return Err(format!(
            "repository URL is not an HTTP(S) or Git SSH remote: {}",
            py_repr_str(raw)
        ));
    }
    // Authority may carry userinfo; credentials refuse.
    let hostport = match authority.rfind('@') {
        Some(_) => {
            return Err("repository URL must not contain credentials".to_string());
        }
        None => authority.clone(),
    };
    if let Some(cut) = path.find(|c| ['?', '#'].contains(&c)) {
        path.truncate(cut);
    }
    while path.ends_with('/') {
        path.pop();
    }
    if path.ends_with(".git") {
        path.truncate(path.len() - 4);
    }
    if path.is_empty() || path == "/" || path.split('/').any(|part| part == "." || part == "..") {
        return Err(format!(
            "repository URL has no safe project path: {}",
            py_repr_str(raw)
        ));
    }
    Ok(format!("{scheme}://{}{path}", hostport.to_lowercase()))
}

fn scp_parts(value: &str) -> Option<(String, String)> {
    // (?:[^@/\s]+@)?(host):(path) fullmatch; host [A-Za-z0-9.-]+,
    // path \S+.
    let (before, path) = value.split_once(':')?;
    if path.is_empty() || path.chars().any(char::is_whitespace) {
        return None;
    }
    if before.matches('@').count() > 1 {
        return None;
    }
    let host = before.rsplit('@').next()?;
    if host.is_empty()
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
        || before.contains('/')
        || before.chars().any(char::is_whitespace)
    {
        return None;
    }
    Some((host.to_string(), path.to_string()))
}

/// Return a compact default label for a canonical repository
/// URL.
pub fn project_label_from_url(repository_url: &str) -> Result<String, String> {
    let path = split_url(repository_url)
        .map(|(_, _, path)| path)
        .unwrap_or_else(|| repository_url.to_string());
    let trimmed = path.trim_end_matches('/');
    let label = trimmed.rsplit('/').next().unwrap_or_default();
    if label.is_empty() {
        return Err(format!(
            "repository URL has no project label: {}",
            py_repr_str(repository_url)
        ));
    }
    Ok(label.to_string())
}

fn validate_label(raw: &str, what: &str, maximum: usize) -> Result<String, String> {
    let value = raw.trim();
    if value.is_empty() || value.chars().count() > maximum || value.chars().any(|c| (c as u32) < 32)
    {
        return Err(format!("invalid {what} {}", py_repr_str(raw)));
    }
    Ok(value.to_string())
}

/// Parse `LABEL=REPOSITORY_URL` supplied by a user.
pub fn explicit_project(raw: &str, primary: bool) -> Result<ProjectIdentity, String> {
    match raw.split_once('=') {
        None => {
            let canonical = canonical_repository_url(raw)?;
            let label = project_label_from_url(&canonical)?;
            Ok(ProjectIdentity {
                label,
                repository_url: Some(canonical),
                primary,
                source: "explicit".to_string(),
            })
        }
        Some((label, remote)) => {
            let label = validate_label(label, "project label", 120)?;
            let canonical = canonical_repository_url(remote)?;
            Ok(ProjectIdentity {
                label,
                repository_url: Some(canonical),
                primary,
                source: "explicit".to_string(),
            })
        }
    }
}

fn casefold(text: &str) -> String {
    // CPython `str.casefold` for the inputs identity keys see:
    // full lowercase plus the sharp-s and final-sigma folds.
    let mut out = String::new();
    for c in text.chars() {
        match c {
            'ß' | 'ẞ' => out.push_str("ss"),
            'ς' => out.push('σ'),
            _ => out.extend(c.to_lowercase()),
        }
    }
    out
}

fn valid_hostname(hostname: &str) -> bool {
    if hostname.len() > 253 || hostname.is_empty() {
        return false;
    }
    hostname.split('.').all(|label| {
        let bytes = label.as_bytes();
        !label.is_empty()
            && label.len() <= 63
            && bytes[0].is_ascii_alphanumeric()
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            && (label.len() == 1 || bytes[bytes.len() - 1].is_ascii_alphanumeric())
    })
}

/// Validate one explicitly supplied execution hostname.
pub fn explicit_host(raw: &str) -> Result<HostIdentity, String> {
    let hostname = raw.trim().trim_end_matches('.').to_lowercase();
    if hostname.len() > 253 || !valid_hostname(&hostname) {
        return Err(format!("invalid hostname {}", py_repr_str(raw)));
    }
    Ok(HostIdentity {
        hostname,
        source: "explicit".to_string(),
    })
}

/// Parse repeatable CLI identity overrides, preserving their
/// display order.
pub fn parse_identity_overrides(
    project_values: &[String],
    host_values: &[String],
) -> Result<(Vec<ProjectIdentity>, Vec<HostIdentity>), String> {
    let mut projects: Vec<ProjectIdentity> = Vec::new();
    let mut seen_projects: Vec<String> = Vec::new();
    for raw in project_values {
        let project = explicit_project(raw, projects.is_empty())?;
        let key = project
            .repository_url
            .clone()
            .unwrap_or_else(|| project.label.to_lowercase());
        if seen_projects.contains(&key) {
            continue;
        }
        seen_projects.push(key);
        projects.push(project);
    }
    let mut hosts: Vec<HostIdentity> = Vec::new();
    for raw in host_values {
        let host = explicit_host(raw)?;
        if hosts.iter().any(|held| held.hostname == host.hostname) {
            continue;
        }
        hosts.push(host);
    }
    Ok((projects, hosts))
}

fn boolean_of(value: Option<&Value>, where_: &str) -> Result<bool, String> {
    match value {
        Some(Value::Bool(flag)) => Ok(*flag),
        _ => Err(format!("{where_}: expected a boolean")),
    }
}

/// Validate a standalone `site-identity.json` record.
pub fn site_identity_from_json(raw: &Value, where_: &str) -> Result<SiteIdentity, String> {
    let root = as_object(raw, where_)?;
    if !json_eq_one(root.get("schema_version")) {
        return Err(format!("{where_}: unsupported site identity schema"));
    }
    let project_values = as_array(
        root.get("projects").unwrap_or(&Value::Null),
        &format!("{where_}.projects"),
    )?;
    let mut projects = Vec::new();
    for (index, value) in project_values.iter().enumerate() {
        let item_where = format!("{where_}.projects[{index}]");
        let item = as_object(value, &item_where)?;
        let label = validate_label(
            as_string(
                item.get("label").unwrap_or(&Value::Null),
                &format!("{item_where}.label"),
            )?,
            "project label",
            120,
        )?;
        let remote = match item.get("repository_url") {
            None | Some(Value::Null) => None,
            Some(value) => Some(canonical_repository_url(as_string(
                value,
                &format!("{item_where}.repository_url"),
            )?)?),
        };
        projects.push(ProjectIdentity {
            label,
            repository_url: remote,
            primary: boolean_of(item.get("primary"), &format!("{item_where}.primary"))?,
            source: as_string(
                item.get("source").unwrap_or(&Value::Null),
                &format!("{item_where}.source"),
            )?
            .to_string(),
        });
    }
    let host_values = as_array(
        root.get("hosts").unwrap_or(&Value::Null),
        &format!("{where_}.hosts"),
    )?;
    let mut hosts = Vec::new();
    for (index, value) in host_values.iter().enumerate() {
        let item_where = format!("{where_}.hosts[{index}]");
        let item = as_object(value, &item_where)?;
        let validated = explicit_host(as_string(
            item.get("hostname").unwrap_or(&Value::Null),
            &format!("{item_where}.hostname"),
        )?)?;
        hosts.push(HostIdentity {
            hostname: validated.hostname,
            source: as_string(
                item.get("source").unwrap_or(&Value::Null),
                &format!("{item_where}.source"),
            )?
            .to_string(),
        });
    }
    let timezone_name = as_string(
        root.get("display_timezone").unwrap_or(&Value::Null),
        &format!("{where_}.display_timezone"),
    )?;
    if let Some(refusal) = crate::window::tz_refusal(timezone_name) {
        return Err(refusal);
    }
    if !valid_tz(timezone_name) {
        return Err(format!(
            "{where_}: unknown IANA timezone {}",
            py_repr_str(timezone_name)
        ));
    }
    let primary_count = projects.iter().filter(|p| p.primary).count();
    if !projects.is_empty() && primary_count != 1 {
        return Err(format!(
            "{where_}: projects must contain exactly one primary entry"
        ));
    }
    Ok(SiteIdentity {
        team_slug: as_string(
            root.get("team_slug").unwrap_or(&Value::Null),
            &format!("{where_}.team_slug"),
        )?
        .to_string(),
        projects,
        hosts,
        display_timezone: timezone_name.to_string(),
        display_timezone_source: as_string(
            root.get("display_timezone_source").unwrap_or(&Value::Null),
            &format!("{where_}.display_timezone_source"),
        )?
        .to_string(),
    })
}

/// Resolve explicit, newly inferred, and prior identity
/// without losing evidence.
#[allow(clippy::too_many_arguments)]
pub fn merge_site_identity(
    team_slug: &str,
    display_timezone: &str,
    display_timezone_source: &str,
    inferred_projects: &[ProjectIdentity],
    inferred_hosts: &[HostIdentity],
    explicit_projects: &[ProjectIdentity],
    explicit_hosts: &[HostIdentity],
    previous: Option<&SiteIdentity>,
) -> Result<SiteIdentity, String> {
    if let Some(refusal) = crate::window::tz_refusal(display_timezone) {
        return Err(refusal);
    }
    if !valid_tz(display_timezone) {
        return Err(format!(
            "unknown IANA timezone {}",
            py_repr_str(display_timezone)
        ));
    }
    let prior_projects: &[ProjectIdentity] = previous.map_or(&[], |held| held.projects.as_slice());
    let mut project_order: Vec<String> = Vec::new();
    let mut projects_by_key: Vec<(String, ProjectIdentity)> = Vec::new();
    for project in prior_projects
        .iter()
        .chain(inferred_projects.iter())
        .chain(explicit_projects.iter())
    {
        let key = project
            .repository_url
            .clone()
            .unwrap_or_else(|| format!("label:{}", casefold(&project.label)));
        if !project_order.contains(&key) {
            project_order.push(key.clone());
        }
        if let Some(entry) = projects_by_key.iter_mut().find(|(held, _)| *held == key) {
            entry.1 = project.clone();
        } else {
            projects_by_key.push((key, project.clone()));
        }
    }
    let selected_primary = explicit_projects
        .iter()
        .find(|p| p.primary)
        .or_else(|| prior_projects.iter().find(|p| p.primary))
        .or_else(|| inferred_projects.iter().find(|p| p.primary));
    let selected_key = selected_primary.map(|primary| {
        primary
            .repository_url
            .clone()
            .unwrap_or_else(|| format!("label:{}", casefold(&primary.label)))
    });
    let projects: Vec<ProjectIdentity> = project_order
        .iter()
        .map(|key| {
            let mut project = projects_by_key
                .iter()
                .find(|(held, _)| held == key)
                .unwrap()
                .1
                .clone();
            project.primary = selected_key.as_deref() == Some(key.as_str());
            project
        })
        .collect();
    let prior_hosts: &[HostIdentity] = previous.map_or(&[], |held| held.hosts.as_slice());
    let mut host_order: Vec<String> = Vec::new();
    let mut hosts_by_key: Vec<(String, HostIdentity)> = Vec::new();
    for host in prior_hosts
        .iter()
        .chain(inferred_hosts.iter())
        .chain(explicit_hosts.iter())
    {
        if !host_order.contains(&host.hostname) {
            host_order.push(host.hostname.clone());
        }
        if let Some(entry) = hosts_by_key
            .iter_mut()
            .find(|(held, _)| *held == host.hostname)
        {
            entry.1 = host.clone();
        } else {
            hosts_by_key.push((host.hostname.clone(), host.clone()));
        }
    }
    let hosts: Vec<HostIdentity> = host_order
        .iter()
        .map(|key| {
            hosts_by_key
                .iter()
                .find(|(held, _)| held == key)
                .unwrap()
                .1
                .clone()
        })
        .collect();
    let mut timezone_source = display_timezone_source.to_string();
    if let Some(previous) = previous {
        if previous.display_timezone == display_timezone
            && (display_timezone_source == "api" || display_timezone_source == "default")
        {
            timezone_source = previous.display_timezone_source.clone();
        }
    }
    Ok(SiteIdentity {
        team_slug: team_slug.to_string(),
        projects,
        hosts,
        display_timezone: display_timezone.to_string(),
        display_timezone_source: timezone_source,
    })
}

fn inferred_project(
    metadata: &serde_json::Map<String, Value>,
    primary: bool,
) -> Option<ProjectIdentity> {
    let git = map_of(metadata.get("git"));
    let remote =
        str_of(git.get("repository_url")).or_else(|| str_of(metadata.get("repository_url")));
    if let Some(remote) = remote {
        if let Ok(canonical) = canonical_repository_url(&remote) {
            if let Ok(label) = project_label_from_url(&canonical) {
                return Some(ProjectIdentity {
                    label,
                    repository_url: Some(canonical),
                    primary,
                    source: "session_metadata".to_string(),
                });
            }
        }
    }
    let cwd = str_of(metadata.get("cwd"))?;
    let cwd = cwd.trim();
    let label = cwd
        .rsplit('/')
        .find(|part| !part.is_empty() && *part != ".")
        .unwrap_or_default()
        .to_string();
    if label.is_empty() {
        return None;
    }
    Some(ProjectIdentity {
        label,
        repository_url: None,
        primary,
        source: "session_metadata".to_string(),
    })
}

fn inferred_host(metadata: &serde_json::Map<String, Value>) -> Option<HostIdentity> {
    let environment = map_of(metadata.get("environment"));
    let raw = str_of(metadata.get("hostname"))
        .or_else(|| str_of(metadata.get("host_name")))
        .or_else(|| str_of(environment.get("hostname")))?;
    let validated = explicit_host(&raw).ok()?;
    Some(HostIdentity {
        hostname: validated.hostname,
        source: "session_metadata".to_string(),
    })
}

/// Infer identity only from structured, provider-owned
/// metadata fields.
pub fn infer_structured_identity(
    records: &[serde_json::Map<String, Value>],
) -> (Vec<ProjectIdentity>, Vec<HostIdentity>) {
    let mut projects: Vec<ProjectIdentity> = Vec::new();
    let mut project_keys: Vec<String> = Vec::new();
    let mut hosts: Vec<HostIdentity> = Vec::new();
    for metadata in records {
        if let Some(project) = inferred_project(metadata, projects.is_empty()) {
            let key = project
                .repository_url
                .clone()
                .unwrap_or_else(|| casefold(&project.label));
            let label_match = projects.iter().position(|prior| {
                casefold(&prior.label) == casefold(&project.label)
                    && (prior.repository_url.is_none() || project.repository_url.is_none())
            });
            match label_match {
                Some(index) => {
                    let prior = projects[index].clone();
                    if prior.repository_url.is_none() && project.repository_url.is_some() {
                        project_keys.retain(|held| *held != casefold(&prior.label));
                        let mut replacement = project.clone();
                        replacement.primary = prior.primary || project.primary;
                        projects[index] = replacement;
                        project_keys.push(key);
                    }
                }
                None => {
                    if !project_keys.contains(&key) {
                        project_keys.push(key);
                        projects.push(project);
                    }
                }
            }
        }
        if let Some(host) = inferred_host(metadata) {
            if !hosts.iter().any(|held| held.hostname == host.hostname) {
                hosts.push(host);
            }
        }
    }
    (projects, hosts)
}

fn str_of(value: Option<&Value>) -> Option<String> {
    // The Python `_string`: stripped, with empty reading as
    // absent.
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn map_of(value: Option<&Value>) -> serde_json::Map<String, Value> {
    value
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}
