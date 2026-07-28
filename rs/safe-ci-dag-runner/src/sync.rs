//! Pluggable UPLOAD + DOWNLOAD of the mergeable profile SUMMARY — the piece that closes the
//! profiling feedback loop on EPHEMERAL CI.
//!
//! Port of `py/safe_ci_dag_runner/sync.py`. On a persistent box the profile store accumulates so the
//! planner improves over time; on ephemeral CI each runner starts empty, so nothing is fed back.
//! This module lets the runner DOWNLOAD the accumulated summary at start (seeding the planner) and
//! UPLOAD this run's contribution at the end, behind a small pluggable [`SyncBackend`] — the same
//! "code against an abstraction" pattern as the cgroup / metrics protocols.
//!
//! Backends: [`LocalDirBackend`] (`local:<dir>`), [`GitBranchBackend`]
//! (`git:<url>#<branch>[#<subdir>]`, the ATOMIC reference with retry-on-conflict RMW),
//! [`GitHubArtifactsBackend`] (`github-artifacts:<name>[#<owner/repo>]`, NON-atomic — a concurrent
//! contribution can occasionally be dropped, acceptable for a statistical summary), and
//! [`S3Backend`] (`s3:<bucket>[/<prefix>]`, a documented STUB — the protocol seam is clean, a real
//! S3/R2 client is a scoped follow-on).
//!
//! Each backend is scoped, like the CSV store, to ONE `(machine_id, container_class)` identity (the
//! summary object it reads/writes is named per identity), so a heterogeneous fleet keeps one summary
//! per homogeneous runner class.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::summary::{self, Summary};

/// A sync backend could not complete an upload/download; raised LOUDLY so the caller degrades
/// visibly rather than silently losing the feedback loop.
#[derive(Debug)]
pub struct SyncError(pub String);

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for SyncError {}

fn err<T>(msg: String) -> Result<T, SyncError> {
    Err(SyncError(msg))
}

/// How many times [`GitBranchBackend`] re-fetches + re-merges + re-pushes on a rejected push.
const DEFAULT_GIT_RETRIES: usize = 6;

/// The per-identity summary object name a backend reads/writes (mirrors the CSV store's per-machine
/// + per-container file naming).
pub fn summary_object_name(machine_id: &str, container_class: &str) -> String {
    format!("summary_{machine_id}_{container_class}.json")
}

/// Download + upload of the mergeable summary, behind one pluggable seam. Mirrors Python's
/// `SyncBackend` protocol.
pub trait SyncBackend {
    /// A short human label for logs (No Silent Failure: the caller prints where it synced).
    fn describe(&self) -> String;
    /// The current stored summary for the identity, or an EMPTY summary when none exists.
    fn download(&self, machine_id: &str, container_class: &str) -> Result<Summary, SyncError>;
    /// Merge `delta` into the stored summary of its identity and return the merged summary.
    fn publish(
        &self,
        delta: &Summary,
        reservoir_cap: usize,
        max_buckets: usize,
    ) -> Result<Summary, SyncError>;
}

// --------------------------------------------------------------------------- local directory

/// Store the summary as a file in a local directory (`local:<dir>`). `publish` is a read-merge-write
/// serialized by a best-effort `O_EXCL` lock file so concurrent local runs do not lose a
/// contribution.
pub struct LocalDirBackend {
    root: PathBuf,
}

impl LocalDirBackend {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn path(&self, machine_id: &str, container_class: &str) -> PathBuf {
        self.root
            .join(summary_object_name(machine_id, container_class))
    }
}

impl SyncBackend for LocalDirBackend {
    fn describe(&self) -> String {
        format!("local:{}", self.root.display())
    }

    fn download(&self, machine_id: &str, container_class: &str) -> Result<Summary, SyncError> {
        let path = self.path(machine_id, container_class);
        if !path.is_file() {
            return Ok(summary::empty(machine_id, container_class));
        }
        let text = fs::read_to_string(&path).map_err(|e| {
            SyncError(format!(
                "local backend: cannot read {}: {e}",
                path.display()
            ))
        })?;
        summary::from_json(&text)
            .map_err(|e| SyncError(format!("local backend: malformed {}: {e}", path.display())))
    }

    fn publish(
        &self,
        delta: &Summary,
        reservoir_cap: usize,
        max_buckets: usize,
    ) -> Result<Summary, SyncError> {
        fs::create_dir_all(&self.root).map_err(|e| {
            SyncError(format!(
                "local backend: cannot create {}: {e}",
                self.root.display()
            ))
        })?;
        let path = self.path(&delta.machine_id, &delta.container_class);
        let lock_path = path.with_extension("json.lock");
        let _lock = LockFile::acquire(&lock_path);
        let base = self.download(&delta.machine_id, &delta.container_class)?;
        let merged = summary::merge(&base, delta, reservoir_cap, max_buckets)
            .map_err(|e| SyncError(format!("local backend: {e}")))?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, summary::to_json(&merged))
            .map_err(|e| SyncError(format!("local backend: write failed: {e}")))?;
        fs::rename(&tmp, &path)
            .map_err(|e| SyncError(format!("local backend: rename failed: {e}")))?;
        Ok(merged)
    }
}

/// A best-effort cross-process lock via `O_EXCL` create, spinning briefly then proceeding (so a stale
/// lock never wedges a run). Released (unlinked) on drop.
struct LockFile {
    path: PathBuf,
    held: bool,
}

impl LockFile {
    fn acquire(path: &Path) -> Self {
        for _ in 0..200 {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
            {
                Ok(_) => {
                    return Self {
                        path: path.to_path_buf(),
                        held: true,
                    }
                }
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(10)),
            }
        }
        // Proceed best-effort after ~2s rather than wedging (matches the degrade-not-fail posture).
        Self {
            path: path.to_path_buf(),
            held: false,
        }
    }
}

impl Drop for LockFile {
    fn drop(&mut self) {
        if self.held {
            let _ = fs::remove_file(&self.path);
        }
    }
}

// --------------------------------------------------------------------------- git branch (atomic)

type BeforePush = Box<dyn Fn(usize) + Send + Sync>;

/// Store the summary on a dedicated git branch (`git:<url>#<branch>[#<subdir>]`) — the ATOMIC
/// reference backend. `publish` works in a private throwaway checkout per attempt, fetches the
/// branch, merges the remote summary with this run's `delta`, commits, and pushes; a REJECTED push
/// retries from a fresh fetch of the NEW tip and re-merges the SAME delta (no clobber, no
/// double-count). Mirrors Python's `GitBranchBackend`.
pub struct GitBranchBackend {
    url: String,
    branch: String,
    subdir: String,
    retries: usize,
    before_push: Option<BeforePush>,
}

impl GitBranchBackend {
    pub fn new(
        url: impl Into<String>,
        branch: impl Into<String>,
        subdir: impl Into<String>,
    ) -> Self {
        let sd: String = subdir.into();
        let sd = sd.trim_matches('/').to_string();
        Self {
            url: url.into(),
            branch: branch.into(),
            subdir: if sd.is_empty() { ".".to_string() } else { sd },
            retries: DEFAULT_GIT_RETRIES,
            before_push: None,
        }
    }

    /// Test hook invoked with the attempt index immediately BEFORE each push, so a test can inject a
    /// conflicting push to exercise retry-on-conflict deterministically.
    pub fn with_before_push(mut self, hook: BeforePush) -> Self {
        self.before_push = Some(hook);
        self
    }

    fn rel(&self, machine_id: &str, container_class: &str) -> String {
        let name = summary_object_name(machine_id, container_class);
        if self.subdir == "." {
            name
        } else {
            format!("{}/{name}", self.subdir)
        }
    }

    fn git(&self, args: &[&str], cwd: &Path) -> Result<std::process::Output, SyncError> {
        Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .map_err(|e| SyncError(format!("git {}: {e}", args.join(" "))))
    }

    fn git_ok(&self, args: &[&str], cwd: &Path) -> Result<std::process::Output, SyncError> {
        let out = self.git(args, cwd)?;
        if !out.status.success() {
            return err(format!(
                "git {} exited {:?}: {}",
                args.join(" "),
                out.status.code(),
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(out)
    }

    fn fresh_checkout(&self) -> Result<PathBuf, SyncError> {
        let work = unique_tmp_dir("scdr-sync-git-")?;
        self.git_ok(&["init", "-q"], &work)?;
        self.git_ok(&["remote", "add", "origin", &self.url], &work)?;
        self.git_ok(
            &["config", "user.email", "safe-ci-dag-runner@example.invalid"],
            &work,
        )?;
        self.git_ok(&["config", "user.name", "safe-ci-dag-runner"], &work)?;
        Ok(work)
    }

    fn fetch_branch(&self, work: &Path) -> Result<bool, SyncError> {
        let out = self.git(
            &["fetch", "-q", "--depth", "1", "origin", &self.branch],
            work,
        )?;
        Ok(out.status.success())
    }
}

impl SyncBackend for GitBranchBackend {
    fn describe(&self) -> String {
        if self.subdir == "." {
            format!("git-branch:{}#{}", self.url, self.branch)
        } else {
            format!("git-branch:{}#{}#{}", self.url, self.branch, self.subdir)
        }
    }

    fn download(&self, machine_id: &str, container_class: &str) -> Result<Summary, SyncError> {
        let work = self.fresh_checkout()?;
        let result = (|| {
            if !self.fetch_branch(&work)? {
                return Ok(summary::empty(machine_id, container_class));
            }
            let rel = self.rel(machine_id, container_class);
            let show = self.git(&["show", &format!("FETCH_HEAD:{rel}")], &work)?;
            if !show.status.success() {
                return Ok(summary::empty(machine_id, container_class));
            }
            let text = String::from_utf8_lossy(&show.stdout).to_string();
            summary::from_json(&text).map_err(|e| {
                SyncError(format!(
                    "git backend: malformed summary on {}: {e}",
                    self.branch
                ))
            })
        })();
        let _ = fs::remove_dir_all(&work);
        result
    }

    fn publish(
        &self,
        delta: &Summary,
        reservoir_cap: usize,
        max_buckets: usize,
    ) -> Result<Summary, SyncError> {
        let rel = self.rel(&delta.machine_id, &delta.container_class);
        let mut last_err = String::new();
        for attempt in 0..self.retries {
            let work = self.fresh_checkout()?;
            let result = (|| -> Result<Option<Summary>, SyncError> {
                let had = self.fetch_branch(&work)?;
                let base = if had {
                    self.git_ok(&["checkout", "-q", "-f", "FETCH_HEAD"], &work)?;
                    self.download(&delta.machine_id, &delta.container_class)?
                } else {
                    summary::empty(&delta.machine_id, &delta.container_class)
                };
                let merged = summary::merge(&base, delta, reservoir_cap, max_buckets)
                    .map_err(|e| SyncError(format!("git backend: {e}")))?;
                let target = work.join(&rel);
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|e| SyncError(format!("git backend: mkdir failed: {e}")))?;
                }
                fs::write(&target, summary::to_json(&merged))
                    .map_err(|e| SyncError(format!("git backend: write failed: {e}")))?;
                self.git_ok(&["add", &rel], &work)?;
                let msg = format!(
                    "profile-summary: {}/{}",
                    delta.machine_id, delta.container_class
                );
                self.git_ok(&["commit", "-q", "-m", &msg], &work)?;
                if let Some(hook) = &self.before_push {
                    hook(attempt);
                }
                let push =
                    self.git(&["push", "origin", &format!("HEAD:{}", self.branch)], &work)?;
                if push.status.success() {
                    return Ok(Some(merged));
                }
                Err(SyncError(
                    String::from_utf8_lossy(&push.stderr).trim().to_string(),
                ))
            })();
            let _ = fs::remove_dir_all(&work);
            match result {
                Ok(Some(merged)) => return Ok(merged),
                Ok(None) => {}
                Err(e) => last_err = e.0,
            }
        }
        err(format!(
            "git backend: push to {} rejected after {} attempts (last: {last_err})",
            self.branch, self.retries
        ))
    }
}

fn unique_tmp_dir(prefix: &str) -> Result<PathBuf, SyncError> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!("{prefix}{}-{nanos}", std::process::id()));
    fs::create_dir_all(&path)
        .map_err(|e| SyncError(format!("cannot create temp dir {}: {e}", path.display())))?;
    Ok(path)
}

// --------------------------------------------------------------------------- github actions artifacts

/// Store the summary as a GitHub Actions artifact (`github-artifacts:<name>[#<owner/repo>]`).
/// Downloads the latest summary artifact via the `gh` CLI and merges it; `publish` writes the merged
/// summary to a local staging file for the workflow's `actions/upload-artifact` step (GitHub has no
/// in-run artifact write API). NON-ATOMIC by design — see the module docs. Mirrors Python's
/// `GitHubArtifactsBackend`.
pub struct GitHubArtifactsBackend {
    name: String,
    repo: Option<String>,
    staging_dir: PathBuf,
}

impl GitHubArtifactsBackend {
    pub fn new(name: impl Into<String>, repo: Option<String>) -> Self {
        let repo = repo.or_else(|| std::env::var("GITHUB_REPOSITORY").ok());
        Self {
            name: name.into(),
            repo,
            staging_dir: PathBuf::from("."),
        }
    }

    fn require_repo(&self) -> Result<&str, SyncError> {
        self.repo.as_deref().ok_or_else(|| {
            SyncError(
                "github-artifacts backend: no repo — pass github-artifacts:<name>#<owner/repo> or \
                 set $GITHUB_REPOSITORY"
                    .to_string(),
            )
        })
    }
}

impl SyncBackend for GitHubArtifactsBackend {
    fn describe(&self) -> String {
        match &self.repo {
            Some(r) => format!("github-artifacts:{}#{r}", self.name),
            None => format!("github-artifacts:{}", self.name),
        }
    }

    fn download(&self, machine_id: &str, container_class: &str) -> Result<Summary, SyncError> {
        let repo = self.require_repo()?;
        let listing = Command::new("gh")
            .args([
                "api",
                &format!("repos/{repo}/actions/artifacts"),
                "--paginate",
            ])
            .output()
            .map_err(|e| SyncError(format!("github-artifacts backend: `gh api` failed: {e}")))?;
        if !listing.status.success() {
            return err(format!(
                "github-artifacts backend: listing artifacts failed: {}",
                String::from_utf8_lossy(&listing.stderr).trim()
            ));
        }
        let payload: serde_json::Value = serde_json::from_slice(&listing.stdout)
            .map_err(|e| SyncError(format!("github-artifacts backend: bad artifacts JSON: {e}")))?;
        let arts = match payload.get("artifacts").and_then(|v| v.as_array()) {
            Some(a) => a,
            None => return Ok(summary::empty(machine_id, container_class)),
        };
        let latest = select_latest_artifact(arts, &self.name);
        let artifact_id = match latest.and_then(|a| a.get("id")).and_then(|v| v.as_i64()) {
            Some(id) => id,
            None => return Ok(summary::empty(machine_id, container_class)),
        };
        let work = unique_tmp_dir("scdr-sync-gha-")?;
        let result = (|| {
            let zip_path = work.join("artifact.zip");
            let zip = Command::new("gh")
                .args([
                    "api",
                    &format!("repos/{repo}/actions/artifacts/{artifact_id}/zip"),
                ])
                .output()
                .map_err(|e| {
                    SyncError(format!("github-artifacts backend: download failed: {e}"))
                })?;
            if !zip.status.success() {
                return err(format!(
                    "github-artifacts backend: download of artifact {artifact_id} failed: {}",
                    String::from_utf8_lossy(&zip.stderr).trim()
                ));
            }
            fs::write(&zip_path, &zip.stdout).map_err(|e| {
                SyncError(format!("github-artifacts backend: write zip failed: {e}"))
            })?;
            let unzip = Command::new("unzip")
                .args(["-o", "-q"])
                .arg(&zip_path)
                .arg("-d")
                .arg(&work)
                .output()
                .map_err(|e| SyncError(format!("github-artifacts backend: `unzip` failed: {e}")))?;
            if !unzip.status.success() {
                return err(format!(
                    "github-artifacts backend: unzip failed: {}",
                    String::from_utf8_lossy(&unzip.stderr).trim()
                ));
            }
            let member = work.join(summary_object_name(machine_id, container_class));
            if !member.is_file() {
                return Ok(summary::empty(machine_id, container_class));
            }
            let text = fs::read_to_string(&member)
                .map_err(|e| SyncError(format!("github-artifacts backend: read failed: {e}")))?;
            summary::from_json(&text)
                .map_err(|e| SyncError(format!("github-artifacts backend: malformed summary: {e}")))
        })();
        let _ = fs::remove_dir_all(&work);
        result
    }

    fn publish(
        &self,
        delta: &Summary,
        reservoir_cap: usize,
        max_buckets: usize,
    ) -> Result<Summary, SyncError> {
        let base = self.download(&delta.machine_id, &delta.container_class)?;
        let merged = summary::merge(&base, delta, reservoir_cap, max_buckets)
            .map_err(|e| SyncError(format!("github-artifacts backend: {e}")))?;
        let path = self.staging_dir.join(summary_object_name(
            &delta.machine_id,
            &delta.container_class,
        ));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                SyncError(format!(
                    "github-artifacts backend: staging mkdir failed: {e}"
                ))
            })?;
        }
        let mut file = fs::File::create(&path).map_err(|e| {
            SyncError(format!(
                "github-artifacts backend: staging write failed: {e}"
            ))
        })?;
        file.write_all(summary::to_json(&merged).as_bytes())
            .map_err(|e| {
                SyncError(format!(
                    "github-artifacts backend: staging write failed: {e}"
                ))
            })?;
        Ok(merged)
    }
}

/// Pick the most-recently-created, non-expired artifact whose `name` matches. Pure (unit-testable
/// without a live runner). Mirrors Python's `_select_latest_artifact`.
pub fn select_latest_artifact<'a>(
    artifacts: &'a [serde_json::Value],
    name: &str,
) -> Option<&'a serde_json::Value> {
    let mut best: Option<&serde_json::Value> = None;
    let mut best_created = String::new();
    for art in artifacts {
        if art.get("name").and_then(|v| v.as_str()) != Some(name) {
            continue;
        }
        if art
            .get("expired")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            continue;
        }
        let created = art.get("created_at").and_then(|v| v.as_str()).unwrap_or("");
        if best.is_none() || created > best_created.as_str() {
            best = Some(art);
            best_created = created.to_string();
        }
    }
    best
}

// --------------------------------------------------------------------------- s3 / r2 stub

/// A DOCUMENTED STUB for object-store (S3 / Cloudflare R2) sync (`s3:<bucket>[/<prefix>]`). The
/// protocol seam is clean; a real client is a scoped follow-on. Raises a clear error rather than
/// pretending to sync. Mirrors Python's `S3Backend`.
pub struct S3Backend {
    bucket: String,
    prefix: String,
}

impl S3Backend {
    pub fn new(bucket: impl Into<String>, prefix: impl Into<String>) -> Self {
        let p: String = prefix.into();
        Self {
            bucket: bucket.into(),
            prefix: p.trim_matches('/').to_string(),
        }
    }

    fn unimplemented<T>(&self) -> Result<T, SyncError> {
        err(
            "s3/r2 backend is a documented follow-on stub — not yet implemented. Use \
             'local:<dir>' or 'git:<url>#<branch>' today. The SyncBackend seam is where a real \
             client (get-latest / merge / put, optionally with a version-id compare-and-swap for \
             atomic read-modify-write) drops in."
                .to_string(),
        )
    }
}

impl SyncBackend for S3Backend {
    fn describe(&self) -> String {
        if self.prefix.is_empty() {
            format!("s3:{} (stub)", self.bucket)
        } else {
            format!("s3:{}/{} (stub)", self.bucket, self.prefix)
        }
    }

    fn download(&self, _machine_id: &str, _container_class: &str) -> Result<Summary, SyncError> {
        self.unimplemented()
    }

    fn publish(&self, _delta: &Summary, _cap: usize, _max: usize) -> Result<Summary, SyncError> {
        self.unimplemented()
    }
}

// --------------------------------------------------------------------------- spec parsing

/// Construct a [`SyncBackend`] from a `--profile-sync` spec (mirrors Python's `parse_backend`)::
///
///   local:<dir> | git:<url>#<branch>[#<subdir>] | github-artifacts:<name>[#<repo>] | s3:<bucket>[/<prefix>]
pub fn parse_backend(spec: &str) -> Result<Box<dyn SyncBackend>, SyncError> {
    let (scheme, rest) = match spec.split_once(':') {
        Some(x) => x,
        None => {
            return err(format!(
                "invalid --profile-sync spec {spec:?}: expected '<scheme>:<...>' \
                 (local:, git:, github-artifacts:, s3:)"
            ))
        }
    };
    match scheme {
        "local" => {
            if rest.is_empty() {
                return err("invalid local spec: expected 'local:<dir>'".to_string());
            }
            Ok(Box::new(LocalDirBackend::new(rest)))
        }
        "git" => {
            let parts: Vec<&str> = rest.split('#').collect();
            if parts.len() < 2 || parts[0].is_empty() || parts[1].is_empty() {
                return err(
                    "invalid git spec: expected 'git:<url>#<branch>[#<subdir>]'".to_string()
                );
            }
            let subdir = if parts.len() >= 3 && !parts[2].is_empty() {
                parts[2]
            } else {
                "."
            };
            Ok(Box::new(GitBranchBackend::new(parts[0], parts[1], subdir)))
        }
        "github-artifacts" => {
            let (name, repo) = match rest.split_once('#') {
                Some((n, r)) => (n, Some(r.to_string())),
                None => (rest, None),
            };
            if name.is_empty() {
                return err(
                    "invalid github-artifacts spec: expected 'github-artifacts:<name>[#<owner/repo>]'"
                        .to_string(),
                );
            }
            Ok(Box::new(GitHubArtifactsBackend::new(name, repo)))
        }
        "s3" => {
            if rest.is_empty() {
                return err("invalid s3 spec: expected 's3:<bucket>[/<prefix>]'".to_string());
            }
            let (bucket, prefix) = match rest.split_once('/') {
                Some((b, p)) => (b, p),
                None => (rest, ""),
            };
            Ok(Box::new(S3Backend::new(bucket, prefix)))
        }
        other => err(format!(
            "unknown --profile-sync scheme {other:?}: supported schemes are \
             local:, git:, github-artifacts:, s3:"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::summary::{self, DEFAULT_MAX_BUCKETS, DEFAULT_RESERVOIR_K};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    const MID: &str = "m";
    const CC: &str = "affinity8_cpu-max-max";

    fn unique_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let p =
            std::env::temp_dir().join(format!("scdr-test-{tag}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn delta(step: &str, elapsed: f64) -> Summary {
        let mut r = HashMap::new();
        r.insert("step".to_string(), step.to_string());
        r.insert("inner_jobs".to_string(), "1".to_string());
        r.insert("elapsed_s".to_string(), format!("{elapsed:.3}"));
        r.insert("peak_bytes".to_string(), "1000".to_string());
        r.insert("pct_other".to_string(), "0.000".to_string());
        summary::summary_from_rows(
            &[r],
            MID,
            CC,
            Some(8),
            DEFAULT_RESERVOIR_K,
            DEFAULT_MAX_BUCKETS,
        )
    }

    fn pub_delta(b: &dyn SyncBackend, step: &str, elapsed: f64) -> Summary {
        b.publish(
            &delta(step, elapsed),
            DEFAULT_RESERVOIR_K,
            DEFAULT_MAX_BUCKETS,
        )
        .unwrap()
    }

    #[test]
    fn parse_backend_dispatch_and_rejects() {
        assert_eq!(
            parse_backend("local:/tmp/x").unwrap().describe(),
            "local:/tmp/x"
        );
        assert!(parse_backend("git:/tmp/repo#data").is_ok());
        assert!(parse_backend("git:/tmp/repo#data#sub/dir").is_ok());
        assert!(parse_backend("github-artifacts:sum#o/r").is_ok());
        assert!(parse_backend("s3:bucket/prefix").is_ok());
        for spec in [
            "",
            "nonsense",
            "local:",
            "git:/tmp/repo",
            "git:#branch",
            "s3:",
            "bogus:x",
        ] {
            assert!(parse_backend(spec).is_err(), "{spec} should be rejected");
        }
    }

    #[test]
    fn s3_backend_is_a_loud_stub() {
        let b = parse_backend("s3:bucket/prefix").unwrap();
        assert!(b.download(MID, CC).is_err());
        assert!(b
            .publish(&delta("g.a", 1.0), DEFAULT_RESERVOIR_K, DEFAULT_MAX_BUCKETS)
            .is_err());
    }

    #[test]
    fn select_latest_artifact_picks_newest_non_expired() {
        let arts: Vec<serde_json::Value> = serde_json::from_str(
            r#"[
            {"name":"sum","id":1,"created_at":"2026-01-01T00:00:00Z","expired":false},
            {"name":"sum","id":2,"created_at":"2026-02-01T00:00:00Z","expired":false},
            {"name":"sum","id":3,"created_at":"2026-03-01T00:00:00Z","expired":true},
            {"name":"other","id":4,"created_at":"2026-04-01T00:00:00Z","expired":false}]"#,
        )
        .unwrap();
        let latest = select_latest_artifact(&arts, "sum").unwrap();
        assert_eq!(latest.get("id").unwrap().as_i64(), Some(2));
        assert!(select_latest_artifact(&arts, "missing").is_none());
    }

    #[test]
    fn local_backend_roundtrip_and_accumulate() {
        let dir = unique_dir("local");
        let b = LocalDirBackend::new(dir.join("store"));
        assert_eq!(summary::summary_stats(&b.download(MID, CC).unwrap()).0, 0);
        pub_delta(&b, "g.a", 1.0);
        pub_delta(&b, "g.b", 2.0);
        let (buckets, total, _) = summary::summary_stats(&b.download(MID, CC).unwrap());
        assert_eq!((buckets, total), (2, 2));
        let _ = fs::remove_dir_all(&dir);
    }

    fn init_bare(path: &Path) -> String {
        Command::new("git")
            .args(["init", "--bare", "-q"])
            .arg(path)
            .output()
            .unwrap();
        path.to_string_lossy().to_string()
    }

    #[test]
    fn git_backend_roundtrip_and_subdir() {
        let dir = unique_dir("git");
        let url = init_bare(&dir.join("bare.git"));
        let b = GitBranchBackend::new(url.clone(), "profile-data", ".");
        assert_eq!(summary::summary_stats(&b.download(MID, CC).unwrap()).0, 0);
        pub_delta(&b, "g.a", 1.0);
        assert_eq!(
            summary::summary_stats(&b.download(MID, CC).unwrap()),
            (1, 1, 1)
        );
        pub_delta(&b, "g.b", 2.0);
        assert_eq!(
            summary::summary_stats(&b.download(MID, CC).unwrap()),
            (2, 2, 1)
        );

        let url2 = init_bare(&dir.join("bare2.git"));
        let sub = GitBranchBackend::new(url2, "profile-data", "nested/profiles");
        pub_delta(&sub, "g.a", 1.0);
        assert_eq!(
            summary::summary_stats(&sub.download(MID, CC).unwrap()),
            (1, 1, 1)
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn git_backend_retry_on_conflict_does_not_clobber() {
        let dir = unique_dir("gitconf");
        let url = init_bare(&dir.join("bare.git"));
        let url_for_hook = url.clone();
        let fired = Arc::new(AtomicBool::new(false));
        let fired_hook = Arc::clone(&fired);
        let hook: BeforePush = Box::new(move |_attempt| {
            // Simulate a concurrent contributor landing on the branch before our first push.
            if !fired_hook.swap(true, Ordering::SeqCst) {
                let concurrent = GitBranchBackend::new(url_for_hook.clone(), "profile-data", ".");
                pub_delta(&concurrent, "g.concurrent", 9.0);
            }
        });
        let ours = GitBranchBackend::new(url.clone(), "profile-data", ".").with_before_push(hook);
        let merged = pub_delta(&ours, "g.ours", 1.0);
        let steps: std::collections::BTreeSet<String> =
            merged.buckets.keys().map(|(s, _)| s.clone()).collect();
        assert_eq!(
            steps,
            ["g.concurrent".to_string(), "g.ours".to_string()]
                .into_iter()
                .collect()
        );
        assert!(fired.load(Ordering::SeqCst));
        let final_ = GitBranchBackend::new(url, "profile-data", ".")
            .download(MID, CC)
            .unwrap();
        let final_steps: std::collections::BTreeSet<String> =
            final_.buckets.keys().map(|(s, _)| s.clone()).collect();
        assert_eq!(final_steps.len(), 2);
        let _ = fs::remove_dir_all(&dir);
    }
}
