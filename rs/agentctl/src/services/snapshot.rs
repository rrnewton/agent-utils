//! Snapshots: compiled release binaries a team's services run, one directory per source commit.
//!
//! ```text
//! ROOT/snapshots/COMMIT12/
//!   snapshot.json                         what was built, from where, and each file's SHA-256
//!   bin/agentctl                          the service binary
//!   bin/agentctl-slack-outbound           the Slack outbound helper
//!   home/plugins/slack/manifest.json      a plugin home for AGENTCTL_HOME
//!   home/plugins/slack/agentctl-slack-plugin
//! ```
//!
//! A snapshot is written once, under a staging name, and renamed into place only after its
//! binary reports the commit it was built from and its plugin home passes plugin discovery. It is
//! never changed afterwards, so a running service's binary is never replaced under it.

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use super::host::Host;

/// Schema of `snapshot.json`.
pub const SNAPSHOT_SCHEMA: &str = "agentctl-services-snapshot/v1";
/// The service binary, relative to a snapshot.
pub const AGENTCTL: &str = "bin/agentctl";
/// The Slack outbound helper, relative to a snapshot.
pub const OUTBOUND: &str = "bin/agentctl-slack-outbound";
/// The plugin home, relative to a snapshot.
pub const HOME: &str = "home";
/// The Slack plugin executable, relative to a snapshot.
pub const PLUGIN: &str = "home/plugins/slack/agentctl-slack-plugin";
/// The Slack plugin manifest, relative to a snapshot.
pub const MANIFEST: &str = "home/plugins/slack/manifest.json";

/// The binaries a build copies, as (cargo output name, place in the snapshot).
const BINARIES: [(&str, &str); 3] = [
    ("agentctl", AGENTCTL),
    ("agentctl-slack-outbound", OUTBOUND),
    ("agentctl-slack-plugin", PLUGIN),
];

/// One snapshot on disk.
#[derive(Clone, Debug)]
pub struct Snapshot {
    /// Its directory.
    pub path: PathBuf,
    /// The full commit it was built from.
    pub commit: String,
    /// When it was built, in Unix seconds.
    pub built_at: u64,
    /// The parsed `snapshot.json`.
    pub document: Value,
}

impl Snapshot {
    /// The service binary.
    #[must_use]
    pub fn agentctl(&self) -> PathBuf {
        self.path.join(AGENTCTL)
    }

    /// The plugin home for `AGENTCTL_HOME`.
    #[must_use]
    pub fn home(&self) -> PathBuf {
        self.path.join(HOME)
    }

    /// The SHA-256 `snapshot.json` records for a file of the snapshot.
    #[must_use]
    pub fn sha256(&self, relative: &str) -> Option<&str> {
        self.document["files"][relative]["sha256"].as_str()
    }

    /// The first 8 characters of the commit.
    #[must_use]
    pub fn short(&self) -> &str {
        self.commit.get(..8).unwrap_or(&self.commit)
    }
}

/// The directory holding every snapshot.
#[must_use]
pub fn snapshots_dir(root: &Path) -> PathBuf {
    root.join("snapshots")
}

/// Every complete snapshot under `root`, oldest build first.
#[must_use]
pub fn list(root: &Path) -> Vec<Snapshot> {
    let mut snapshots = Vec::new();
    let Ok(entries) = fs::read_dir(snapshots_dir(root)) else {
        return snapshots;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        // Staging directories start with a dot and are never a snapshot.
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        if let Some(snapshot) = read(&entry.path()) {
            snapshots.push(snapshot);
        }
    }
    // Newest by commit time, so building an older commit later never makes it the default.
    snapshots.sort_by_key(|snapshot| {
        (
            snapshot.document["source"]["committed_at"]
                .as_u64()
                .unwrap_or(0),
            snapshot.built_at,
            snapshot.commit.clone(),
        )
    });
    snapshots
}

/// Read one snapshot directory; `None` when it has no readable `snapshot.json`.
#[must_use]
pub fn read(path: &Path) -> Option<Snapshot> {
    let bytes = fs::read(path.join("snapshot.json")).ok()?;
    let document: Value = serde_json::from_slice(&bytes).ok()?;
    if document["schema"] != SNAPSHOT_SCHEMA {
        return None;
    }
    let commit = document["commit"].as_str()?.to_owned();
    if commit.len() != 40
        || !commit
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return None;
    }
    Some(Snapshot {
        path: path.to_path_buf(),
        commit,
        built_at: document["built_at"].as_u64()?,
        document,
    })
}

/// Find a snapshot by a commit prefix of at least 7 characters, or the newest with `None`.
pub fn select(root: &Path, commit: Option<&str>) -> Result<Snapshot, String> {
    let snapshots = list(root);
    match commit {
        None => snapshots.into_iter().next_back().ok_or_else(|| {
            format!(
                "no snapshot under {}; run `agentctl services build` first",
                snapshots_dir(root).display()
            )
        }),
        Some(prefix) => {
            let prefix = prefix.to_ascii_lowercase();
            if prefix.len() < 7 {
                return Err("--snapshot needs at least 7 characters of the commit".to_owned());
            }
            let mut matching: Vec<Snapshot> = snapshots
                .into_iter()
                .filter(|snapshot| snapshot.commit.starts_with(&prefix))
                .collect();
            match matching.len() {
                1 => Ok(matching.remove(0)),
                0 => Err(format!(
                    "no snapshot of commit {prefix} under {}",
                    snapshots_dir(root).display()
                )),
                _ => Err(format!("commit prefix {prefix} matches several snapshots")),
            }
        }
    }
}

/// What `services build` needs.
#[derive(Clone, Debug)]
pub struct BuildOptions {
    /// Where snapshots live.
    pub root: PathBuf,
    /// The Git checkout to build from.
    pub checkout: PathBuf,
    /// The revision to build (default HEAD).
    pub revision: String,
    /// The cargo executable.
    pub cargo: PathBuf,
    /// Parent of the temporary source and target directories.
    pub work_dir: PathBuf,
    /// Keep the temporary directory after the build, for inspection.
    pub keep_work: bool,
}

fn private_dir(path: &Path) -> io::Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

fn git(checkout: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(checkout)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("cannot run git: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git -C {} {} failed: {}",
            checkout.display(),
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// The full commit a revision names in a checkout.
pub fn resolve_commit(checkout: &Path, revision: &str) -> Result<String, String> {
    let commit = git(
        checkout,
        &["rev-parse", "--verify", &format!("{revision}^{{commit}}")],
    )?;
    if commit.len() != 40 || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("git named {commit:?} for {revision}"));
    }
    Ok(commit.to_ascii_lowercase())
}

/// Extract `commit` from `checkout` into `destination` with `git archive | tar -x`: the
/// committed tree only, never the working tree's edits or untracked files.
fn archive(checkout: &Path, commit: &str, destination: &Path) -> Result<(), String> {
    private_dir(destination).map_err(|error| error.to_string())?;
    let mut git = Command::new("git")
        .arg("-C")
        .arg(checkout)
        .args(["archive", "--format=tar", commit])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot run git archive: {error}"))?;
    let stdout = git.stdout.take().expect("piped");
    let tar = Command::new("tar")
        .arg("-x")
        .arg("-C")
        .arg(destination)
        .stdin(stdout)
        .status()
        .map_err(|error| format!("cannot run tar: {error}"))?;
    let archived = git
        .wait()
        .map_err(|error| format!("git archive: {error}"))?;
    if !archived.success() || !tar.success() {
        return Err(format!(
            "git archive {commit} | tar -x failed (git {archived}, tar {tar})"
        ));
    }
    Ok(())
}

/// Copy `source` to `destination` (created new) with `mode`.
fn install(source: &Path, destination: &Path, mode: u32) -> Result<(), String> {
    let bytes = fs::read(source).map_err(|error| format!("{}: {error}", source.display()))?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(destination)
        .map_err(|error| format!("{}: {error}", destination.display()))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("{}: {error}", destination.display()))?;
    fs::set_permissions(destination, fs::Permissions::from_mode(mode))
        .map_err(|error| format!("{}: {error}", destination.display()))
}

fn unix_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// Install compiled binaries into a new snapshot: stage, verify, describe, and rename into place.
/// `release` holds the cargo outputs; `manifest` is the Slack plugin manifest's bytes.
pub fn install_snapshot(
    host: &dyn Host,
    root: &Path,
    commit: &str,
    release: &Path,
    manifest: &[u8],
    source: &Value,
) -> Result<Snapshot, String> {
    let snapshots = snapshots_dir(root);
    private_dir(&snapshots).map_err(|error| format!("{}: {error}", snapshots.display()))?;
    let destination = snapshots.join(&commit[..12]);
    if destination.exists() {
        return Err(format!(
            "{} already exists and is not a complete snapshot; remove it and build again",
            destination.display()
        ));
    }
    let staging = snapshots.join(format!(".staging-{}-{}", &commit[..12], std::process::id()));
    let result = stage(host, &staging, commit, release, manifest, source).and_then(|()| {
        fs::rename(&staging, &destination).map_err(|error| {
            format!(
                "cannot rename {} to {}: {error}",
                staging.display(),
                destination.display()
            )
        })
    });
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result?;
    read(&destination).ok_or_else(|| format!("{} is unreadable", destination.display()))
}

fn stage(
    host: &dyn Host,
    staging: &Path,
    commit: &str,
    release: &Path,
    manifest: &[u8],
    source: &Value,
) -> Result<(), String> {
    for directory in ["bin", "home/plugins/slack"] {
        private_dir(&staging.join(directory)).map_err(|error| error.to_string())?;
    }
    for (name, place) in BINARIES {
        install(&release.join(name), &staging.join(place), 0o500)?;
    }
    let manifest_path = staging.join(MANIFEST);
    fs::write(&manifest_path, manifest)
        .and_then(|()| fs::set_permissions(&manifest_path, fs::Permissions::from_mode(0o400)))
        .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
    let reported = host.binary_commit(&staging.join(AGENTCTL));
    if reported.as_deref() != Some(commit) {
        return Err(format!(
            "the built agentctl reports commit {} instead of {commit}",
            reported.as_deref().unwrap_or("(none)")
        ));
    }
    let inventory = crate::plugins::discover_at(staging.join(HOME));
    if !inventory.refused.is_empty()
        || !inventory
            .discovered
            .iter()
            .any(|plugin| plugin.name == "slack")
    {
        return Err(format!(
            "plugin discovery refuses the snapshot's plugin home: {}",
            inventory
                .refused
                .iter()
                .map(|refusal| format!("{}: {}", refusal.entry, refusal.detail))
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    let mut files = serde_json::Map::new();
    for relative in [AGENTCTL, OUTBOUND, PLUGIN, MANIFEST] {
        let path = staging.join(relative);
        let bytes = fs::metadata(&path).map(|metadata| metadata.len()).ok();
        files.insert(
            relative.to_owned(),
            json!({"sha256": host.file_sha256(&path), "bytes": bytes}),
        );
    }
    let built_at = unix_seconds(host.now());
    let document = json!({
        "schema": SNAPSHOT_SCHEMA,
        "commit": commit,
        "built_at": built_at,
        "built_at_utc": super::utc(built_at),
        "source": source,
        "files": files,
    });
    let path = staging.join("snapshot.json");
    let mut bytes = serde_json::to_vec_pretty(&document).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    fs::write(&path, bytes)
        .and_then(|()| fs::set_permissions(&path, fs::Permissions::from_mode(0o400)))
        .map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(())
}

/// Build a snapshot of `options.revision`: `git archive` into a fresh directory, then a normal
/// `cargo build --release --locked` of agentctl and the Slack plugin package with the commit
/// recorded in the binary. Returns the snapshot and whether it was built now (an existing complete
/// snapshot of the same commit is reused).
pub fn build(host: &dyn Host, options: &BuildOptions) -> Result<(Snapshot, bool), String> {
    let commit = resolve_commit(&options.checkout, &options.revision)?;
    if let Some(existing) = read(&snapshots_dir(&options.root).join(&commit[..12])) {
        return Ok((existing, false));
    }
    let work = options.work_dir.join(format!(
        "agentctl-services-build-{}-{}",
        &commit[..12],
        std::process::id()
    ));
    DirBuilder::new()
        .mode(0o700)
        .create(&work)
        .map_err(|error| format!("{}: {error}", work.display()))?;
    let result = build_in(host, options, &commit, &work);
    if !options.keep_work {
        let _ = fs::remove_dir_all(&work);
    }
    result.map(|snapshot| (snapshot, true))
}

fn build_in(
    host: &dyn Host,
    options: &BuildOptions,
    commit: &str,
    work: &Path,
) -> Result<Snapshot, String> {
    let source = work.join("src");
    archive(&options.checkout, commit, &source)?;
    let target = work.join("target");
    // Cargo's progress goes to standard error; standard output stays for the JSON report.
    let progress = io::stderr()
        .as_fd()
        .try_clone_to_owned()
        .map_err(|error| error.to_string())?;
    let status = Command::new(&options.cargo)
        .current_dir(source.join("rs"))
        .env("CARGO_INCREMENTAL", "0")
        .env("AGENTCTL_SOURCE_COMMIT", commit)
        .env("AGENTCTL_SOURCE_DIRTY", "0")
        .env_remove("AGENTCTL_SOURCE_FP")
        .args(["build", "--release", "--locked", "--manifest-path"])
        .arg(source.join("rs/Cargo.toml"))
        .arg("--target-dir")
        .arg(&target)
        .args(["-p", "agentctl", "-p", "chat-subscription-slack", "--bins"])
        .stdin(Stdio::null())
        .stdout(Stdio::from(progress))
        .status()
        .map_err(|error| format!("cannot run {}: {error}", options.cargo.display()))?;
    if !status.success() {
        return Err(format!("cargo build failed ({status})"));
    }
    let manifest_path = source.join("rs/chat-subscription-slack/manifest.json");
    let manifest = fs::read(&manifest_path)
        .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
    let cargo_version = Command::new(&options.cargo)
        .arg("--version")
        .output()
        .ok()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned());
    let committed_at = git(&options.checkout, &["log", "-1", "--format=%ct", commit])
        .ok()
        .and_then(|seconds| seconds.parse::<u64>().ok());
    let description = json!({
        "committed_at": committed_at,
        "checkout": options.checkout,
        "revision": options.revision,
        "cargo": cargo_version,
        "command": "cargo build --release --locked -p agentctl -p chat-subscription-slack --bins",
    });
    install_snapshot(
        host,
        &options.root,
        commit,
        &target.join("release"),
        &manifest,
        &description,
    )
}
