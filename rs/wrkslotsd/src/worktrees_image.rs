//! Pure worktrees-image core (P8, mu-rsimg, unwired until dispatch).
//!
//! Mirrors the testable half of Python `worktreesimage.py`: settings
//! validation (`settings_from_obj`), mount classification
//! (`classify_mount`), the migration `listing_signature`, and the
//! `.worktrees-image.json` bind records. Privileged btrfs/mount
//! operations stay behind a runner seam in the writer step; nothing
//! here runs sudo, mounts, or touches a live image.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::image_mounts::MountEntry;

pub(crate) const SCHEMA: &str = "wrkslots-worktrees-image/v1";
pub(crate) const MIN_SIZE_BYTES: i64 = 1_073_741_824;
pub(crate) const IMAGE_MOUNT_NAME: &str = ".slots-image";
pub(crate) const RECORD_NAME: &str = ".worktrees-image.json";

pub(crate) const MOUNTED: &str = "mounted";
pub(crate) const NOT_MOUNTED: &str = "not-mounted";
pub(crate) const WRONG_MOUNT: &str = "wrong-mount";
pub(crate) const NOT_EMPTY_UNMOUNTED: &str = "not-empty-unmounted";
pub(crate) const MISSING: &str = "missing";

/// Configuration for the project worktrees image, mirroring
/// Python `WorktreesImageSettings`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorktreesImageSettings {
    pub(crate) relative_path: String,
    pub(crate) size_bytes: i64,
    pub(crate) filesystem: String,
    pub(crate) mount: String,
    pub(crate) slot_quota_bytes: Option<i64>,
    pub(crate) machines: Option<Vec<String>>,
    pub(crate) mount_at: String,
    pub(crate) quota_mode: String,
}

impl WorktreesImageSettings {
    pub(crate) fn image_file(&self, root: &Path) -> PathBuf {
        root.join(&self.relative_path)
    }

    pub(crate) fn direct(&self) -> bool {
        self.mount_at == "worktrees_dir"
    }

    /// The mount point under the configured layout, mirroring
    /// Python `mount_point`: beside control for binds, or the
    /// slots directory itself when direct.
    pub(crate) fn mount_point(&self, control: &Path, worktrees: &Path) -> PathBuf {
        if self.direct() {
            worktrees.to_path_buf()
        } else {
            image_mount_dir(control)
        }
    }

    /// Whether this section applies on the host named `machine`,
    /// mirroring Python `WorktreesImageSettings.applies_on`.
    pub(crate) fn applies_on(&self, machine: &str) -> bool {
        self.machines
            .as_ref()
            .is_none_or(|machines| machines.iter().any(|name| name == machine))
    }

    /// Why this section is skipped on `machine`, or None when it
    /// applies, with Python `_parse_worktrees_image`'s exact text.
    pub(crate) fn skipped_reason(&self, machine: &str) -> Option<String> {
        if self.applies_on(machine) {
            return None;
        }
        let named = self
            .machines
            .as_ref()
            .map(|machines| machines.join(", "))
            .unwrap_or_default();
        Some(format!(
            "configuration.worktrees_image applies on machines [{named}] only; this host is {machine}"
        ))
    }
}

/// The one host-scope decision, mirroring Python
/// `_parse_worktrees_image`: validate on every host, then return
/// the settings for this host or `(None, skip reason)`. A section
/// whose image path lives inside the slots directory it mounts is
/// refused before the scope decision, as in the oracle.
pub(crate) fn parse_worktrees_image(
    value: Option<&Value>,
    root: &Path,
    worktrees: &Path,
    machine: &str,
) -> Result<(Option<WorktreesImageSettings>, Option<String>), String> {
    let Some(value) = value else {
        return Ok((None, None));
    };
    let settings = settings_from_obj(value)?;
    let image = crate::image_mounts::normalize_absolute(&settings.image_file(root));
    let slots = crate::image_mounts::normalize_absolute(worktrees);
    if image.starts_with(&slots) {
        return Err(
            "configuration.worktrees_image.path must live outside the slots directory it mounts"
                .to_string(),
        );
    }
    if let Some(reason) = settings.skipped_reason(machine) {
        return Ok((None, Some(reason)));
    }
    Ok((Some(settings), None))
}

fn py_repr(value: &Value) -> String {
    match value {
        Value::String(text) => format!("'{}'", text.replace('\\', "\\\\").replace('\'', "\\'")),
        other => other.to_string(),
    }
}

/// Validate one `worktrees_image` section, mirroring Python
/// `settings_from_obj` with its error strings.
pub(crate) fn settings_from_obj(value: &Value) -> Result<WorktreesImageSettings, String> {
    let Some(map) = value.as_object() else {
        return Err("configuration.worktrees_image must be a mapping".to_string());
    };
    let known = [
        "path",
        "size_bytes",
        "fs",
        "mount",
        "slot_quota_bytes",
        "machines",
        "mount_at",
        "quota_mode",
    ];
    let mut unknown: Vec<&String> = map
        .keys()
        .filter(|key| !known.contains(&key.as_str()))
        .collect();
    unknown.sort();
    if !unknown.is_empty() {
        let names: Vec<&str> = unknown.iter().map(|key| key.as_str()).collect();
        return Err(format!(
            "configuration.worktrees_image has unknown keys: {}",
            names.join(", ")
        ));
    }
    let raw_path = map.get("path").and_then(Value::as_str).unwrap_or_default();
    if raw_path.trim().is_empty() {
        return Err("configuration.worktrees_image.path must be a non-empty string".to_string());
    }
    let candidate = Path::new(raw_path);
    if candidate.is_absolute() || candidate.components().any(|part| part.as_os_str() == "..") {
        return Err(
            "configuration.worktrees_image.path must be a project-relative path without '..'"
                .to_string(),
        );
    }
    let size = map.get("size_bytes").and_then(Value::as_i64);
    if size.map(|value| value < MIN_SIZE_BYTES).unwrap_or(true) {
        return Err(format!(
            "configuration.worktrees_image.size_bytes must be an integer of at least {MIN_SIZE_BYTES}"
        ));
    }
    let filesystem = map.get("fs").and_then(Value::as_str).unwrap_or("btrfs");
    if filesystem != "btrfs" {
        return Err(format!(
            "configuration.worktrees_image.fs must be one of btrfs, not {}",
            py_repr(&json!(filesystem))
        ));
    }
    let mount = map.get("mount").and_then(Value::as_str).unwrap_or("kernel");
    if mount == "fuse" {
        return Err(
            "configuration.worktrees_image.mount 'fuse' is unavailable: btrfs has no unprivileged FUSE driver (no fuse2fs equivalent); use 'kernel' with passwordless sudo, or do not enable worktrees_image on this host"
                .to_string(),
        );
    }
    if !matches!(mount, "kernel" | "auto") {
        return Err(format!(
            "configuration.worktrees_image.mount must be one of kernel, auto, not {}",
            py_repr(&json!(mount))
        ));
    }
    let quota = map.get("slot_quota_bytes");
    if let Some(quota) = quota.filter(|value| !value.is_null()) {
        if quota.as_i64().map(|value| value < 1).unwrap_or(true) {
            return Err(
                "configuration.worktrees_image.slot_quota_bytes must be a positive integer"
                    .to_string(),
            );
        }
    }
    let mount_at = map
        .get("mount_at")
        .and_then(Value::as_str)
        .unwrap_or("binds");
    if !matches!(mount_at, "binds" | "worktrees_dir") {
        return Err(format!(
            "configuration.worktrees_image.mount_at must be one of binds, worktrees_dir, not {}",
            py_repr(&json!(mount_at))
        ));
    }
    let quota_mode = map
        .get("quota_mode")
        .and_then(Value::as_str)
        .unwrap_or("simple");
    if !matches!(quota_mode, "simple" | "full") {
        return Err(format!(
            "configuration.worktrees_image.quota_mode must be one of simple, full, not {}",
            py_repr(&json!(quota_mode))
        ));
    }
    // Validated on every host, named or not: a malformed scope
    // must never silently read as applies-everywhere or nowhere.
    let machines = match map.get("machines") {
        None => None,
        Some(value) => {
            let Some(names) = value.as_array() else {
                return Err(
                    "configuration.worktrees_image.machines must be a non-empty list of unique non-empty machine names"
                        .to_string(),
                );
            };
            let mut parsed: Vec<String> = Vec::new();
            for name in names {
                let Some(name) = name.as_str() else {
                    return Err(
                        "configuration.worktrees_image.machines must be a non-empty list of unique non-empty machine names"
                            .to_string(),
                    );
                };
                if name.trim().is_empty() || parsed.iter().any(|seen| seen == name) {
                    return Err(
                        "configuration.worktrees_image.machines must be a non-empty list of unique non-empty machine names"
                            .to_string(),
                    );
                }
                parsed.push(name.to_string());
            }
            if parsed.is_empty() {
                return Err(
                    "configuration.worktrees_image.machines must be a non-empty list of unique non-empty machine names"
                        .to_string(),
                );
            }
            Some(parsed)
        }
    };
    Ok(WorktreesImageSettings {
        relative_path: raw_path.replace('\\', "/"),
        size_bytes: size.unwrap_or_default(),
        filesystem: filesystem.to_string(),
        mount: mount.to_string(),
        slot_quota_bytes: quota.and_then(Value::as_i64),
        machines,
        mount_at: mount_at.to_string(),
        quota_mode: quota_mode.to_string(),
    })
}

/// Classify the slots directory's mount state from an injected
/// mount table, mirroring Python `classify_mount`.
pub(crate) fn classify_mount(
    slots_dir: &Path,
    entries: &[MountEntry],
    slots_dir_exists: bool,
    slots_dir_empty: bool,
) -> &'static str {
    if !slots_dir_exists {
        return MISSING;
    }
    let target = crate::image_mounts::normalize_absolute(slots_dir);
    let top = entries
        .iter()
        .rev()
        .find(|entry| crate::image_mounts::normalize_absolute(&entry.mount_point) == target);
    let Some(top) = top else {
        return if slots_dir_empty {
            NOT_MOUNTED
        } else {
            NOT_EMPTY_UNMOUNTED
        };
    };
    if top.fstype != "btrfs" {
        return WRONG_MOUNT;
    }
    MOUNTED
}

pub(crate) fn image_mount_dir(control: &Path) -> PathBuf {
    control.join(IMAGE_MOUNT_NAME)
}

pub(crate) fn record_path(control: &Path) -> PathBuf {
    control.join(RECORD_NAME)
}

/// One slot's recorded bind, mirroring Python `BindRecord`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BindRecord {
    pub(crate) subvolume: String,
    pub(crate) mount_id: Option<i64>,
    pub(crate) target: Option<String>,
}

/// Load `.worktrees-image.json`, accepting the earlier string form,
/// mirroring Python `load_bind_records`.
pub(crate) fn load_bind_records(control: &Path) -> Result<BTreeMap<String, BindRecord>, String> {
    let path = record_path(control);
    if !path.is_file() {
        return Ok(BTreeMap::new());
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("cannot read worktrees image record {path:?}: {error}"))?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|error| format!("cannot read worktrees image record {path:?}: {error}"))?;
    let Some(binds) = value.get("binds").and_then(Value::as_object) else {
        return Err(format!(
            "malformed worktrees image record: {}",
            path.display()
        ));
    };
    let mut records = BTreeMap::new();
    for (slot, item) in binds {
        if let Some(subvolume) = item.as_str() {
            records.insert(
                slot.clone(),
                BindRecord {
                    subvolume: subvolume.to_string(),
                    mount_id: None,
                    target: None,
                },
            );
        } else if let Some(subvolume) = item.get("subvolume").and_then(Value::as_str) {
            records.insert(
                slot.clone(),
                BindRecord {
                    subvolume: subvolume.to_string(),
                    mount_id: item.get("mount_id").and_then(Value::as_i64),
                    target: item
                        .get("target")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                },
            );
        } else {
            return Err(format!(
                "malformed worktrees image record: {}",
                path.display()
            ));
        }
    }
    Ok(records)
}

pub(crate) const RECORD_LOCK_NAME: &str = ".worktrees-image.json.lock";

fn write_bind_records_locked(
    control: &Path,
    records: &BTreeMap<String, BindRecord>,
) -> Result<(), String> {
    use std::io::Write as _;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let path = record_path(control);
    let nonce = format!(
        "{}.{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0),
        NONCE.fetch_add(1, Ordering::Relaxed)
    );
    let temporary = control.join(format!(
        "{RECORD_NAME}.tmp.{}.{}",
        std::process::id(),
        nonce
    ));
    let binds: serde_json::Map<String, Value> = records
        .iter()
        .map(|(slot, record)| {
            (
                slot.clone(),
                json!({
                    "subvolume": record.subvolume,
                    "mount_id": record.mount_id,
                    "target": record.target,
                }),
            )
        })
        .collect();
    let payload = json!({ "binds": binds });
    let text = format!(
        "{}\n",
        serde_json::to_string_pretty(&payload).unwrap_or_default()
    );
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| format!("cannot write worktrees image record {temporary:?}: {error}"))?;
    if let Err(error) = file
        .write_all(text.as_bytes())
        .and_then(|()| file.sync_all())
    {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!(
            "cannot write worktrees image record {temporary:?}: {error}"
        ));
    }
    drop(file);
    if let Err(error) = std::fs::rename(&temporary, &path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!(
            "cannot replace worktrees image record {path:?}: {error}"
        ));
    }
    std::fs::File::open(control)
        .and_then(|dir| dir.sync_all())
        .map_err(|error| format!("cannot sync worktrees image directory {control:?}: {error}"))
}

/// The only bind-record mutation path, mirroring Python
/// `_update_bind_records`: hold the one flock, load, mutate, and
/// write only if something changed. Mounts stay outside the lock.
pub(crate) fn update_bind_records(
    control: &Path,
    mutate: impl FnOnce(BTreeMap<String, BindRecord>) -> BTreeMap<String, BindRecord>,
) -> Result<BTreeMap<String, BindRecord>, String> {
    let _ = std::fs::create_dir_all(control);
    let lock_path = control.join(RECORD_LOCK_NAME);
    let _lock = crate::ensure_states::acquire_lock(&lock_path, &lock_path)?;
    let before = load_bind_records(control)?;
    let after = mutate(before.clone());
    if after != before {
        write_bind_records_locked(control, &after)?;
    }
    Ok(after)
}

/// Persist bind records atomically beside the registry, under the
/// same records flock as every other bind-record writer.
pub(crate) fn save_bind_records(
    control: &Path,
    records: &BTreeMap<String, BindRecord>,
) -> Result<(), String> {
    let replacement = records.clone();
    update_bind_records(control, move |_| replacement)?;
    Ok(())
}

/// Sorted `(path, kind, size, mode, proof)` for every migratable
/// entry under a temp/fixture tree, mirroring Python
/// `listing_signature`: proof is the link target for a symlink and
/// the SHA-256 of the contents for a file. Special files skipped.
pub(crate) fn listing_signature(root: &Path) -> Vec<(String, String, u64, u32, String)> {
    use std::os::unix::fs::MetadataExt as _;
    let mut signature = Vec::new();
    fn content_hash(path: &Path) -> String {
        use sha2::{Digest, Sha256};
        use std::io::Read as _;
        let mut digest = Sha256::new();
        if let Ok(mut file) = std::fs::File::open(path) {
            let mut buffer = vec![0_u8; 1024 * 1024];
            loop {
                match file.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => digest.update(&buffer[..count]),
                }
            }
        }
        format!("{:x}", digest.finalize())
    }
    fn walk(
        root: &Path,
        current: &Path,
        out: &mut Vec<(String, String, u64, u32, String)>,
        hash: &dyn Fn(&Path) -> String,
    ) {
        let Ok(entries) = std::fs::read_dir(current) else {
            return;
        };
        let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        paths.sort();
        for entry in paths {
            let relative = entry
                .strip_prefix(root)
                .unwrap_or(&entry)
                .to_string_lossy()
                .replace('\\', "/");
            let Ok(meta) = entry.symlink_metadata() else {
                continue;
            };
            let mode = meta.mode() & 0o7777;
            let kind = meta.file_type();
            if kind.is_symlink() {
                let proof = std::fs::read_link(&entry)
                    .map(|target| target.to_string_lossy().into_owned())
                    .unwrap_or_default();
                out.push((relative, "link".to_string(), 0, mode, proof));
            } else if kind.is_dir() {
                out.push((relative.clone(), "dir".to_string(), 0, mode, String::new()));
                walk(root, &entry, out, hash);
            } else if kind.is_file() {
                out.push((relative, "file".to_string(), meta.len(), mode, hash(&entry)));
            }
        }
    }
    walk(root, root, &mut signature, &content_hash);
    signature.sort();
    signature
}

/// Sorted `(checkout, HEAD, status)` per Git checkout under root,
/// mirroring Python `git_state_signature` (status with
/// `--untracked-files=no`, both reads `--no-optional-locks`).
pub(crate) fn git_state_signature(root: &Path) -> Vec<(String, String, String)> {
    let mut states = Vec::new();
    fn git_value(directory: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .arg("--no-optional-locks")
            .arg("-C")
            .arg(directory)
            .args(args)
            .output();
        match output {
            Ok(out) if out.status.success() => {
                String::from_utf8_lossy(&out.stdout).trim().to_string()
            }
            Ok(out) => {
                let text = if out.stderr.is_empty() {
                    String::from_utf8_lossy(&out.stdout).trim().to_string()
                } else {
                    String::from_utf8_lossy(&out.stderr).trim().to_string()
                };
                format!("unreadable: {text}")
            }
            Err(error) => format!("unreadable: {error}"),
        }
    }
    fn walk(root: &Path, current: &Path, out: &mut Vec<(String, String, String)>) {
        let Ok(entries) = std::fs::read_dir(current) else {
            return;
        };
        let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        paths.sort();
        let has_git = paths
            .iter()
            .any(|path| path.file_name().map(|name| name == ".git").unwrap_or(false));
        if has_git {
            let relative = current
                .strip_prefix(root)
                .unwrap_or(current)
                .to_string_lossy()
                .replace('\\', "/");
            let relative = if relative.is_empty() {
                ".".to_string()
            } else {
                relative
            };
            out.push((
                relative,
                git_value(current, &["rev-parse", "HEAD"]),
                git_value(
                    current,
                    &["status", "--porcelain=v1", "--untracked-files=no"],
                ),
            ));
        }
        for entry in paths {
            let is_git = entry
                .file_name()
                .map(|name| name == ".git")
                .unwrap_or(false);
            if !is_git
                && entry.is_dir()
                && !entry
                    .symlink_metadata()
                    .map(|m| m.file_type().is_symlink())
                    .unwrap_or(false)
            {
                walk(root, &entry, out);
            }
        }
    }
    walk(root, root, &mut states);
    states.sort();
    states
}

// ------------------------------------------------------------- ops (S6)
//
// Privileged btrfs/mount argv construction, mirroring Python
// `worktreesimage._run` (`sudo -n` prefix, failure string naming the
// whole command). Effects go through `image_ops::PrivilegedRunner`;
// tests use a recording fake and no command ever runs here.

use crate::image_ops::{CommandSpec, PrivilegedRunner};
use std::time::Duration;

fn sudo_argv(argv: &[String]) -> Vec<String> {
    let mut out = vec!["sudo".to_string(), "-n".to_string()];
    out.extend(argv.iter().cloned());
    out
}

fn spec(argv: Vec<String>) -> CommandSpec {
    CommandSpec::new(argv, Duration::from_secs(120))
}

/// Run specs in order through the runner, mirroring Python `_run`'s
/// failure: `"<whole command> failed: <stderr or stdout>"`.
pub(crate) fn run_sequence(
    runner: &impl PrivilegedRunner,
    specs: &[CommandSpec],
) -> Result<Vec<String>, String> {
    let mut stdouts = Vec::new();
    for item in specs {
        let outcome = runner.run(item)?;
        if outcome.returncode != 0 {
            let detail = if outcome.stderr.trim().is_empty() {
                outcome.stdout.trim()
            } else {
                outcome.stderr.trim()
            };
            return Err(format!("{} failed: {detail}", item.argv.join(" ")));
        }
        stdouts.push(outcome.stdout);
    }
    Ok(stdouts)
}

/// The format steps of `create_image` (the caller creates the
/// zero-byte file first and truncates after NOCOW, as the oracle's
/// load-bearing order requires): `chattr +C`, then `mkfs.btrfs`.
pub(crate) fn create_format_plan(image_file: &Path) -> Vec<CommandSpec> {
    let file = image_file.to_string_lossy().into_owned();
    vec![
        spec(vec!["chattr".to_string(), "+C".to_string(), file.clone()]),
        spec(sudo_argv(&[
            "mkfs.btrfs".to_string(),
            "-f".to_string(),
            "-m".to_string(),
            "single".to_string(),
            "-d".to_string(),
            "single".to_string(),
            "-L".to_string(),
            "wrkslots-worktrees".to_string(),
            file,
        ])),
    ]
}

/// Whether mounting may proceed, mirroring `mount_image`'s refusals.
pub(crate) fn mount_refusal(state: &str, mount_point: &Path) -> Option<String> {
    match state {
        MOUNTED => None,
        WRONG_MOUNT | NOT_EMPTY_UNMOUNTED => Some(format!(
            "refusing to mount over {}: state is {state}; inspect the directory and any existing mount first",
            mount_point.display()
        )),
        _ => None,
    }
}

/// The mount steps: `mount -o loop,...` then `btrfs quota enable`.
pub(crate) fn mount_plan(image_file: &Path, mount_point: &Path) -> Vec<CommandSpec> {
    let point = mount_point.to_string_lossy().into_owned();
    vec![
        spec(sudo_argv(&[
            "mount".to_string(),
            "-o".to_string(),
            "loop,compress=zstd:3,noatime".to_string(),
            image_file.to_string_lossy().into_owned(),
            point.clone(),
        ])),
        spec(sudo_argv(&[
            "btrfs".to_string(),
            "quota".to_string(),
            "enable".to_string(),
            point,
        ])),
    ]
}

pub(crate) fn unmount_plan(mount_point: &Path) -> Vec<CommandSpec> {
    vec![spec(sudo_argv(&[
        "umount".to_string(),
        mount_point.to_string_lossy().into_owned(),
    ]))]
}

pub(crate) fn trim_plan(mount_point: &Path) -> Vec<CommandSpec> {
    vec![spec(sudo_argv(&[
        "fstrim".to_string(),
        "-v".to_string(),
        mount_point.to_string_lossy().into_owned(),
    ]))]
}

/// Loop devices holding `image_file`, parsed from `losetup -j`
/// output lines (`/dev/loopN: [..]:..`), for the grow refresh.
pub(crate) fn losetup_devices(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter_map(|line| line.split(':').next())
        .map(|device| device.trim().to_string())
        .filter(|device| !device.is_empty())
        .collect()
}

/// The grow refresh steps after the caller truncates: one
/// `losetup -c` per holder, then `btrfs filesystem resize max`.
pub(crate) fn grow_plan(devices: &[String], mount_point: &Path) -> Vec<CommandSpec> {
    let mut specs: Vec<CommandSpec> = devices
        .iter()
        .map(|device| {
            spec(sudo_argv(&[
                "losetup".to_string(),
                "-c".to_string(),
                device.clone(),
            ]))
        })
        .collect();
    specs.push(spec(sudo_argv(&[
        "btrfs".to_string(),
        "filesystem".to_string(),
        "resize".to_string(),
        "max".to_string(),
        mount_point.to_string_lossy().into_owned(),
    ])));
    specs
}

/// The grow ceiling refusal, mirroring `grow_image`.
pub(crate) fn grow_refusal(current_bytes: u64, new_size_bytes: u64) -> Option<String> {
    if new_size_bytes <= current_bytes {
        Some(format!(
            "new size {new_size_bytes} does not exceed current apparent size {current_bytes}"
        ))
    } else {
        None
    }
}

/// `btrfs subvolume create` plus `chown`, and the optional qgroup
/// limit, mirroring `create_slot_subvolume`.
pub(crate) fn subvolume_plan(
    path: &Path,
    quota_bytes: Option<i64>,
    uid: u32,
    gid: u32,
) -> Vec<CommandSpec> {
    let target = path.to_string_lossy().into_owned();
    let mut specs = vec![
        spec(sudo_argv(&[
            "btrfs".to_string(),
            "subvolume".to_string(),
            "create".to_string(),
            target.clone(),
        ])),
        spec(sudo_argv(&[
            "chown".to_string(),
            format!("{uid}:{gid}"),
            target.clone(),
        ])),
    ];
    if let Some(quota) = quota_bytes {
        specs.push(spec(sudo_argv(&[
            "btrfs".to_string(),
            "qgroup".to_string(),
            "limit".to_string(),
            quota.to_string(),
            target,
        ])));
    }
    specs
}

pub(crate) fn bind_plan(source: &Path, target: &Path) -> Vec<CommandSpec> {
    vec![spec(sudo_argv(&[
        "mount".to_string(),
        "--bind".to_string(),
        source.to_string_lossy().into_owned(),
        target.to_string_lossy().into_owned(),
    ]))]
}

// ---------------------------------------------------------- handlers (#1141)
//
// Dispatch-shaped worktrees-image handlers. Each returns the
// shared HandlerOutcome byte-identical to the Python edition on
// post-load inputs; privileged effects go through the
// PrivilegedRunner seam and bind/mount facts are injected.

use crate::imagecmd::HandlerOutcome;

/// Inputs the dispatch glue supplies for this family.
pub(crate) struct WtCtx<'a> {
    pub(crate) root: &'a Path,
    pub(crate) control: &'a Path,
    pub(crate) worktrees: &'a Path,
    pub(crate) entries: &'a [MountEntry],
    /// Why the configured section is skipped on this host, when it is.
    pub(crate) skipped: Option<&'a str>,
}

fn wt_refusal(message: &str) -> HandlerOutcome {
    HandlerOutcome {
        exit: 3,
        stdout: String::new(),
        stderr: format!(
            "REFUSED: {message}\nREMEDY: run 'wrkslots worktrees-image --help', correct the named condition, then rerun 'wrkslots worktrees-image' with the required arguments\n"
        ),
    }
}

fn wt_refusal_for(message: &str, command: &str) -> HandlerOutcome {
    HandlerOutcome {
        exit: 3,
        stdout: String::new(),
        stderr: format!(
            "REFUSED: {message}\nREMEDY: run 'wrkslots {command} --help', correct the named condition, then rerun 'wrkslots {command}' with the required arguments\n"
        ),
    }
}

fn wt_settings_for(
    settings: Option<WorktreesImageSettings>,
    skipped: Option<&str>,
    command: &str,
) -> Result<WorktreesImageSettings, HandlerOutcome> {
    if let Some(settings) = settings {
        return Ok(settings);
    }
    if let Some(reason) = skipped {
        return Err(wt_refusal_for(
            &format!("worktrees_image does not apply on this host: {reason}"),
            command,
        ));
    }
    Err(wt_refusal_for(
        "no worktrees_image configured; add a worktrees_image section (path, size_bytes, fs: btrfs, mount: kernel) to enable it",
        command,
    ))
}

fn wt_settings(
    settings: Option<WorktreesImageSettings>,
    skipped: Option<&str>,
) -> Result<WorktreesImageSettings, HandlerOutcome> {
    wt_settings_for(settings, skipped, "worktrees-image")
}

fn run_one_for(
    runner: &dyn crate::image_ops::PrivilegedRunner,
    spec: &CommandSpec,
    command: &str,
) -> Result<crate::image_ops::RunOutcome, HandlerOutcome> {
    match runner.run(spec) {
        Ok(outcome) if outcome.returncode == 0 => Ok(outcome),
        Ok(outcome) => {
            let detail = if outcome.stderr.trim().is_empty() {
                outcome.stdout.trim()
            } else {
                outcome.stderr.trim()
            };
            Err(wt_refusal_for(
                &format!("{} failed: {detail}", spec.argv.join(" ")),
                command,
            ))
        }
        Err(message) => Err(wt_refusal_for(&message, command)),
    }
}

fn run_one(
    runner: &dyn crate::image_ops::PrivilegedRunner,
    spec: &CommandSpec,
) -> Result<crate::image_ops::RunOutcome, HandlerOutcome> {
    run_one_for(runner, spec, "worktrees-image")
}

fn dir_facts(path: &Path) -> (bool, bool) {
    let exists = path.is_dir();
    let empty = if exists {
        std::fs::read_dir(path)
            .map(|mut it| it.next().is_none())
            .unwrap_or(false)
    } else {
        true
    };
    (exists, empty)
}

/// The status report value, mirroring Python `status_report`.
pub(crate) fn status_report(
    ctx: &WtCtx<'_>,
    settings: &WorktreesImageSettings,
) -> serde_json::Value {
    status_report_with_quota(ctx, settings, None)
}

/// `status_report` with the active quota mode injected (the
/// caller reads it from the mounted file system at dispatch).
pub(crate) fn status_report_with_quota(
    ctx: &WtCtx<'_>,
    settings: &WorktreesImageSettings,
    active_quota_mode: Option<&str>,
) -> serde_json::Value {
    use std::os::unix::fs::MetadataExt as _;
    let image_file = settings.image_file(ctx.root);
    // Python status_report: beside the slots directory for binds,
    // the slots directory itself when direct.
    let mount_dir = if settings.direct() {
        ctx.worktrees.to_path_buf()
    } else {
        ctx.worktrees
            .parent()
            .map(|parent| parent.join(IMAGE_MOUNT_NAME))
            .unwrap_or_default()
    };
    let (exists, empty) = dir_facts(&mount_dir);
    let state = classify_mount(&mount_dir, ctx.entries, exists, empty);
    let mut report = serde_json::Map::new();
    report.insert("image_file".into(), json!(image_file.to_string_lossy()));
    report.insert("mount_point".into(), json!(mount_dir.to_string_lossy()));
    report.insert("mount_at".into(), json!(settings.mount_at));
    report.insert("configured_size_bytes".into(), json!(settings.size_bytes));
    report.insert("state".into(), json!(state));
    let is_file = image_file.is_file();
    report.insert("image_exists".into(), json!(is_file));
    report.insert(
        "quota_mode".into(),
        json!(if state == MOUNTED {
            active_quota_mode.unwrap_or("unknown")
        } else {
            "unknown"
        }),
    );
    report.insert("configured_quota_mode".into(), json!(settings.quota_mode));
    let binds: serde_json::Map<String, Value> = if settings.direct() {
        serde_json::Map::new()
    } else {
        let binds_root = ctx.worktrees.parent().unwrap_or(ctx.worktrees);
        load_bind_records(binds_root)
            .unwrap_or_default()
            .into_iter()
            .map(|(slot, record)| (slot, json!(record.subvolume)))
            .collect()
    };
    report.insert("binds".into(), Value::Object(binds));
    if is_file {
        if let Ok(meta) = std::fs::metadata(&image_file) {
            report.insert("apparent_bytes".into(), json!(meta.len()));
            report.insert("allocated_bytes".into(), json!(meta.blocks() * 512));
        }
    }
    Value::Object(report)
}

fn py_str(text: &str) -> String {
    // Python json.dumps default escaping (ensure_ascii=True).
    let mut out = String::from("\"");
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if (ch as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch if (ch as u32) < 0x7f => out.push(ch),
            ch => {
                let mut buf = [0_u16; 2];
                for unit in ch.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }
    }
    out.push('"');
    out
}

/// `wrkslots worktrees-image status [--format human|json]`.
pub(crate) fn wt_status(
    ctx: &WtCtx<'_>,
    settings: Option<WorktreesImageSettings>,
    format: &str,
) -> HandlerOutcome {
    let settings = match wt_settings(settings, ctx.skipped) {
        Ok(settings) => settings,
        Err(outcome) => return outcome,
    };
    let report = status_report(ctx, &settings);
    if format == "json" {
        return HandlerOutcome {
            exit: 0,
            stdout: format!("{}\n", render_report_json(&report)),
            stderr: String::new(),
        };
    }
    let mut out = format!(
        "image: {} (exists={})\n",
        report["image_file"].as_str().unwrap_or_default(),
        if report["image_exists"].as_bool().unwrap_or(false) {
            "True"
        } else {
            "False"
        }
    );
    out.push_str(&format!(
        "mount point: {} state={}\n",
        report["mount_point"].as_str().unwrap_or_default(),
        report["state"].as_str().unwrap_or_default()
    ));
    if report["image_exists"].as_bool().unwrap_or(false) {
        out.push_str(&format!(
            "apparent={} allocated={}\n",
            report["apparent_bytes"], report["allocated_bytes"]
        ));
    }
    if report["state"].as_str() != Some(MOUNTED) {
        out.push_str("remedy: wrkslots worktrees-image mount\n");
    }
    HandlerOutcome {
        exit: 0,
        stdout: out,
        stderr: String::new(),
    }
}

fn render_report_json(report: &Value) -> String {
    let keys = [
        "image_file",
        "mount_point",
        "mount_at",
        "configured_size_bytes",
        "state",
        "image_exists",
        "quota_mode",
        "configured_quota_mode",
        "binds",
        "apparent_bytes",
        "allocated_bytes",
    ];
    let parts: Vec<String> = keys
        .iter()
        .filter_map(|key| {
            report.get(key).map(|value| {
                let rendered = match value {
                    Value::Object(map) => {
                        if map.is_empty() {
                            return "{}".to_string();
                        }
                        let inner: Vec<String> = map
                            .iter()
                            .map(|(k, v)| {
                                format!(
                                    "    {}: {}",
                                    py_str(k),
                                    py_str(v.as_str().unwrap_or_default())
                                )
                            })
                            .collect();
                        format!("{{\n{}\n  }}", inner.join(",\n"))
                    }
                    Value::String(text) => py_str(text),
                    other => other.to_string(),
                };
                format!("  {}: {}", py_str(key), rendered)
            })
        })
        .collect();
    format!("{{\n{}\n}}", parts.join(",\n"))
}

/// `wrkslots worktrees-image create`: zero-byte file, NOCOW, then
/// truncate, then mkfs — the oracle's load-bearing order. Any
/// failure deletes the partial file, as `create_image` does.
pub(crate) fn wt_create(
    ctx: &WtCtx<'_>,
    settings: Option<WorktreesImageSettings>,
    runner: &dyn crate::image_ops::PrivilegedRunner,
) -> HandlerOutcome {
    let settings = match wt_settings(settings, ctx.skipped) {
        Ok(settings) => settings,
        Err(outcome) => return outcome,
    };
    let image_file = settings.image_file(ctx.root);
    if image_file.exists() {
        return wt_refusal(&format!(
            "worktrees image already exists: {}",
            image_file.display()
        ));
    }
    if let Some(parent) = image_file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let created = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&image_file);
    if let Err(error) = created {
        return HandlerOutcome {
            exit: 1,
            stdout: String::new(),
            stderr: format!("{error}\n"),
        };
    }
    // Image files are owner-only from creation (#1151).
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(&image_file, std::fs::Permissions::from_mode(0o600));
    }
    let plan = create_format_plan(&image_file);
    // chattr first, then truncate, then mkfs.
    let result = run_one(runner, &plan[0]).and_then(|_| {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&image_file)
            .map_err(|error| HandlerOutcome {
                exit: 1,
                stdout: String::new(),
                stderr: format!("{error}\n"),
            })?;
        file.set_len(settings.size_bytes as u64)
            .map_err(|error| HandlerOutcome {
                exit: 1,
                stdout: String::new(),
                stderr: format!("{error}\n"),
            })?;
        run_one(runner, &plan[1])
    });
    if let Err(outcome) = result {
        let _ = std::fs::remove_file(&image_file);
        return outcome;
    }
    HandlerOutcome {
        exit: 0,
        stdout: format!(
            "created {} ({} bytes, sparse btrfs)\n",
            image_file.display(),
            settings.size_bytes
        ),
        stderr: String::new(),
    }
}

/// `wrkslots worktrees-image mount`: mount (idempotent) and
/// re-establish recorded binds whose targets are not mounted.
/// `mount_id` records the bind's live mount id for the record.
pub(crate) fn wt_mount(
    ctx: &WtCtx<'_>,
    settings: Option<WorktreesImageSettings>,
    mount_id: &dyn Fn(&Path) -> Option<i64>,
    runner: &dyn crate::image_ops::PrivilegedRunner,
) -> HandlerOutcome {
    let settings = match wt_settings(settings, ctx.skipped) {
        Ok(settings) => settings,
        Err(outcome) => return outcome,
    };
    let mount_dir = image_mount_dir(ctx.control);
    let _ = std::fs::create_dir_all(&mount_dir);
    let (exists, empty) = dir_facts(&mount_dir);
    let state = classify_mount(&mount_dir, ctx.entries, exists, empty);
    if let Some(message) = mount_refusal(state, &mount_dir) {
        return wt_refusal(&message);
    }
    if state != MOUNTED {
        if !settings.image_file(ctx.root).is_file() {
            return wt_refusal(&format!(
                "worktrees image file is missing: {}",
                settings.image_file(ctx.root).display()
            ));
        }
        for spec in mount_plan(&settings.image_file(ctx.root), &mount_dir) {
            if let Err(outcome) = run_one(runner, &spec) {
                return outcome;
            }
        }
    }
    // ensure_binds: fenced canonical slots and mounted targets
    // are left alone; a record whose subvolume is gone is skipped
    // without error or re-creation. Each bind mounts first, then
    // records under the records flock.
    if let Ok(records) = load_bind_records(ctx.control) {
        let mounted_points: Vec<PathBuf> = ctx
            .entries
            .iter()
            .map(|e| crate::image_mounts::normalize_absolute(&e.mount_point))
            .collect();
        for (slot, record) in records {
            let canonical = ctx.worktrees.join(&slot);
            let target = record
                .target
                .clone()
                .map(PathBuf::from)
                .unwrap_or_else(|| canonical.clone());
            if target == canonical {
                let fenced_prefix = format!(".{slot}.fenced.");
                let has_fenced = std::fs::read_dir(ctx.worktrees)
                    .map(|entries| {
                        entries.flatten().any(|entry| {
                            entry
                                .file_name()
                                .to_string_lossy()
                                .starts_with(&fenced_prefix)
                        })
                    })
                    .unwrap_or(false);
                if has_fenced {
                    continue;
                }
            }
            if mounted_points.contains(&crate::image_mounts::normalize_absolute(&target)) {
                continue;
            }
            let source = mount_dir.join(&record.subvolume);
            if !source.is_dir() {
                continue;
            }
            let _ = std::fs::create_dir_all(&target);
            for spec in bind_plan(&source, &target) {
                if let Err(outcome) = run_one(runner, &spec) {
                    return outcome;
                }
            }
            let recorded = BindRecord {
                subvolume: record.subvolume.clone(),
                mount_id: mount_id(&target),
                target: Some(target.to_string_lossy().into_owned()),
            };
            if let Err(message) = update_bind_records(ctx.control, move |mut current| {
                current.insert(slot.clone(), recorded);
                current
            }) {
                return wt_refusal(&message);
            }
        }
    }
    HandlerOutcome {
        exit: 0,
        stdout: format!(
            "mounted {} at {} (binds re-established)\n",
            settings.image_file(ctx.root).display(),
            mount_dir.display()
        ),
        stderr: String::new(),
    }
}

/// `wrkslots worktrees-image unmount`.
pub(crate) fn wt_unmount(
    ctx: &WtCtx<'_>,
    settings: Option<WorktreesImageSettings>,
    runner: &dyn crate::image_ops::PrivilegedRunner,
) -> HandlerOutcome {
    let settings = match wt_settings(settings, ctx.skipped) {
        Ok(settings) => settings,
        Err(outcome) => return outcome,
    };
    let _ = &settings;
    let mount_dir = image_mount_dir(ctx.control);
    for spec in unmount_plan(&mount_dir) {
        if let Err(outcome) = run_one(runner, &spec) {
            return outcome;
        }
    }
    HandlerOutcome {
        exit: 0,
        stdout: format!("unmounted {}\n", mount_dir.display()),
        stderr: String::new(),
    }
}

/// `wrkslots worktrees-image grow --to-size-bytes N`: truncate,
/// refresh each holder's loop capacity, resize to max.
pub(crate) fn wt_grow(
    ctx: &WtCtx<'_>,
    settings: Option<WorktreesImageSettings>,
    to_size_bytes: i64,
    runner: &dyn crate::image_ops::PrivilegedRunner,
) -> HandlerOutcome {
    let settings = match wt_settings(settings, ctx.skipped) {
        Ok(settings) => settings,
        Err(outcome) => return outcome,
    };
    let image_file = settings.image_file(ctx.root);
    let current = std::fs::metadata(&image_file).map(|m| m.len()).unwrap_or(0);
    if let Some(message) = grow_refusal(current, to_size_bytes as u64) {
        return wt_refusal(&message);
    }
    if let Ok(file) = std::fs::OpenOptions::new().write(true).open(&image_file) {
        let _ = file.set_len(to_size_bytes as u64);
    }
    // losetup -j through the runner, then the refresh plan.
    let list_spec = spec(sudo_argv(&[
        "losetup".to_string(),
        "-j".to_string(),
        image_file.to_string_lossy().into_owned(),
    ]));
    let devices = match run_one(runner, &list_spec) {
        Ok(outcome) => losetup_devices(&outcome.stdout),
        Err(outcome) => return outcome,
    };
    for spec_item in grow_plan(&devices, &image_mount_dir(ctx.control)) {
        if let Err(outcome) = run_one(runner, &spec_item) {
            return outcome;
        }
    }
    HandlerOutcome {
        exit: 0,
        stdout: format!("grew {} to {} bytes\n", image_file.display(), to_size_bytes),
        stderr: String::new(),
    }
}

/// `wrkslots worktrees-image trim`: fstrim output or "trimmed".
pub(crate) fn wt_trim(
    ctx: &WtCtx<'_>,
    settings: Option<WorktreesImageSettings>,
    runner: &dyn crate::image_ops::PrivilegedRunner,
) -> HandlerOutcome {
    let settings = match wt_settings(settings, ctx.skipped) {
        Ok(settings) => settings,
        Err(outcome) => return outcome,
    };
    let _ = &settings;
    let plan = trim_plan(&image_mount_dir(ctx.control));
    match run_one(runner, &plan[0]) {
        Ok(outcome) => {
            let text = outcome.stdout.trim();
            HandlerOutcome {
                exit: 0,
                stdout: format!("{}\n", if text.is_empty() { "trimmed" } else { text }),
                stderr: String::new(),
            }
        }
        Err(outcome) => outcome,
    }
}

/// Whether any process's cwd is inside `path`, mirroring Python
/// `process_inside` (a /proc scan; unreadable /proc counts as use).
pub(crate) fn process_inside(path: &Path) -> bool {
    let target = path.to_string_lossy().into_owned();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return true;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.bytes().all(|b| b.is_ascii_digit()) || name.is_empty() {
            continue;
        }
        if let Ok(cwd) = std::fs::read_link(entry.path().join("cwd")) {
            let cwd = cwd.to_string_lossy().into_owned();
            if cwd == target || cwd.starts_with(&format!("{target}/")) {
                return true;
            }
        }
    }
    false
}

fn copy_tree_skip_special(source: &Path, destination: &Path) -> Result<(), String> {
    // Mirrors shutil.copytree(symlinks=True, ignore=_skip_special):
    // links are recreated, special files are skipped.
    std::fs::create_dir_all(destination).map_err(|e| {
        format!(
            "cannot copy {} to {}: {e}",
            source.display(),
            destination.display()
        )
    })?;
    let mut entries: Vec<PathBuf> = std::fs::read_dir(source)
        .map_err(|e| format!("cannot copy {}: {e}", source.display()))?
        .flatten()
        .map(|e| e.path())
        .collect();
    entries.sort();
    for entry in entries {
        let target = destination.join(entry.file_name().expect("name"));
        let meta = entry.symlink_metadata().map_err(|e| e.to_string())?;
        let kind = meta.file_type();
        if kind.is_symlink() {
            let pointed = std::fs::read_link(&entry).map_err(|e| e.to_string())?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(&pointed, &target).map_err(|e| e.to_string())?;
        } else if kind.is_dir() {
            copy_tree_skip_special(&entry, &target)?;
        } else if kind.is_file() {
            std::fs::copy(&entry, &target).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// `wrkslots migrate SLOT` (#1161): copy one idle slot into the
/// worktrees image and bind it at the same path. `state` is the
/// injected mount classification of the image mount directory;
/// `mount_id` records a bind's live id. The registry/lock side
/// stays with the dispatch glue, as in the other handlers.
/// Unbind one slot, mirroring Python `unbind_slot`: unmount
/// first, then drop the record under the records flock, so an
/// interrupted remove retries from the bound state.
pub(crate) fn unbind_slot(
    ctx: &WtCtx<'_>,
    slot: &str,
    slot_path: &Path,
    runner: &dyn crate::image_ops::PrivilegedRunner,
) -> HandlerOutcome {
    let spec = spec(sudo_argv(&[
        "umount".to_string(),
        slot_path.to_string_lossy().into_owned(),
    ]));
    if let Err(outcome) = run_one(runner, &spec) {
        return outcome;
    }
    let slot_name = slot.to_string();
    if let Err(message) = update_bind_records(ctx.control, move |mut records| {
        records.remove(&slot_name);
        records
    }) {
        return wt_refusal(&message);
    }
    HandlerOutcome {
        exit: 0,
        stdout: String::new(),
        stderr: String::new(),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn wt_migrate(
    ctx: &WtCtx<'_>,
    settings: Option<WorktreesImageSettings>,
    slot: &str,
    slot_path: &Path,
    state: &str,
    process_in_use: bool,
    owner_ids: (u32, u32),
    aside_suffix: &str,
    displaced_unused: &dyn Fn(&Path) -> Result<(), String>,
    mount_id: &dyn Fn(&Path) -> Option<i64>,
    runner: &dyn crate::image_ops::PrivilegedRunner,
) -> HandlerOutcome {
    let settings = match wt_settings_for(settings, ctx.skipped, "migrate") {
        Ok(settings) => settings,
        Err(outcome) => return outcome,
    };
    if state != MOUNTED {
        return wt_refusal_for(
            &format!("worktrees image is {state}; run 'wrkslots worktrees-image mount' first"),
            "migrate",
        );
    }
    if !slot_path.is_dir() {
        return wt_refusal_for(
            &format!("slot directory is missing: {}", slot_path.display()),
            "migrate",
        );
    }
    let records = match load_bind_records(ctx.control) {
        Ok(records) => records,
        Err(message) => return wt_refusal_for(&message, "migrate"),
    };
    if records.contains_key(slot) {
        return HandlerOutcome {
            exit: 0,
            stdout: format!("{slot} is already bound from the worktrees image\n"),
            stderr: String::new(),
        };
    }
    if process_in_use {
        return wt_refusal_for(
            &format!(
                "a process is working inside {}; migrate only a stopped slot",
                slot_path.display()
            ),
            "migrate",
        );
    }
    let before = listing_signature(slot_path);
    let mount_dir = image_mount_dir(ctx.control);
    let subvolume = mount_dir.join(slot);
    if subvolume.exists() {
        return wt_refusal_for(
            &format!(
                "image subvolume already exists: {}; inspect it first",
                subvolume.display()
            ),
            "migrate",
        );
    }
    for spec in subvolume_plan(
        &subvolume,
        settings.slot_quota_bytes,
        owner_ids.0,
        owner_ids.1,
    ) {
        if let Err(outcome) = run_one_for(runner, &spec, "migrate") {
            return outcome;
        }
    }
    if let Err(message) = copy_tree_skip_special(slot_path, &subvolume) {
        // A failed copy leaves no partial subvolume behind.
        let delete = spec(sudo_argv(&[
            "btrfs".to_string(),
            "subvolume".to_string(),
            "delete".to_string(),
            subvolume.to_string_lossy().into_owned(),
        ]));
        let _ = runner.run(&delete);
        return HandlerOutcome {
            exit: 1,
            stdout: String::new(),
            stderr: format!("{message}\n"),
        };
    }
    let before_git = git_state_signature(slot_path);
    if listing_signature(&subvolume) != before || git_state_signature(&subvolume) != before_git {
        let delete = spec(sudo_argv(&[
            "btrfs".to_string(),
            "subvolume".to_string(),
            "delete".to_string(),
            subvolume.to_string_lossy().into_owned(),
        ]));
        let _ = runner.run(&delete);
        return wt_refusal_for(
            "migrated copy does not match the slot file by file; original kept",
            "migrate",
        );
    }
    // Reclaim (#1149): move the verified original aside, bind at
    // the original path, prove the displaced tree unused, delete.
    let displaced = slot_path
        .parent()
        .map(|parent| parent.join(format!(".{slot}.premigrate-{aside_suffix}")))
        .unwrap_or_default();
    if let Err(error) = std::fs::rename(slot_path, &displaced) {
        return HandlerOutcome {
            exit: 1,
            stdout: String::new(),
            stderr: format!("{error}\n"),
        };
    }
    let bind_result = bind_plan(&mount_dir.join(slot), slot_path)
        .iter()
        .try_for_each(|item| run_one_for(runner, item, "migrate").map(|_| ()));
    if let Err(outcome) = bind_result {
        if slot_path.is_dir()
            && std::fs::read_dir(slot_path)
                .map(|mut it| it.next().is_none())
                .unwrap_or(false)
        {
            let _ = std::fs::remove_dir(slot_path);
        }
        let _ = std::fs::rename(&displaced, slot_path);
        let delete = spec(sudo_argv(&[
            "btrfs".to_string(),
            "subvolume".to_string(),
            "delete".to_string(),
            subvolume.to_string_lossy().into_owned(),
        ]));
        let _ = runner.run(&delete);
        return outcome;
    }
    let recorded = BindRecord {
        subvolume: slot.to_string(),
        mount_id: mount_id(slot_path),
        target: Some(slot_path.to_string_lossy().into_owned()),
    };
    let slot_name = slot.to_string();
    let _ = update_bind_records(ctx.control, move |mut current| {
        current.insert(slot_name, recorded);
        current
    });
    if let Err(reason) = displaced_unused(&displaced) {
        return wt_refusal_for(
            &format!(
                "migrated {slot} into the worktrees image, bound at {}, but the verified original at {} could not be proven unused for reclaim: {reason}; delete it once inspected",
                slot_path.display(),
                displaced.display()
            ),
            "migrate",
        );
    }
    if let Err(error) = std::fs::remove_dir_all(&displaced) {
        return wt_refusal_for(
            &format!(
                "migrated {slot} into the worktrees image, bound at {}, but the verified original remains at {}: {error}; delete it once inspected",
                slot_path.display(),
                displaced.display()
            ),
            "migrate",
        );
    }
    HandlerOutcome {
        exit: 0,
        stdout: format!(
            "migrated {slot} into the worktrees image, bound at {} (original reclaimed)\n",
            slot_path.display()
        ),
        stderr: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_root(tag: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "wrkslotsd-wt-image-{tag}-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn settings_validation_matches_python() {
        let ok =
            settings_from_obj(&json!({"path": "wt.img", "size_bytes": 1073741824})).expect("ok");
        assert_eq!(ok.filesystem, "btrfs");
        assert_eq!(ok.mount, "kernel");
        assert_eq!(ok.mount_at, "binds");
        assert_eq!(ok.quota_mode, "simple");
        assert!(!ok.direct());
        let direct = settings_from_obj(
            &json!({"path": "wt.img", "size_bytes": 1073741824, "mount_at": "worktrees_dir", "quota_mode": "full"}),
        )
        .expect("direct ok");
        assert!(direct.direct());
        assert_eq!(direct.quota_mode, "full");
        assert!(settings_from_obj(
            &json!({"path": "wt.img", "size_bytes": 1073741824, "mount_at": "elsewhere"})
        )
        .is_err());
        assert!(settings_from_obj(
            &json!({"path": "wt.img", "size_bytes": 1073741824, "quota_mode": "none"})
        )
        .is_err());
        assert!(
            settings_from_obj(&json!({"path": "../wt.img", "size_bytes": 1073741824})).is_err()
        );
        assert!(settings_from_obj(&json!({"path": "wt.img", "size_bytes": 5})).is_err());
        let fuse = settings_from_obj(
            &json!({"path": "wt.img", "size_bytes": 1073741824, "mount": "fuse"}),
        )
        .expect_err("fuse refused");
        assert!(fuse.contains("no unprivileged FUSE driver"), "{fuse}");
    }

    #[test]
    fn classify_and_records_and_signature() {
        let root = temp_root("classify");
        let slots = root.join("slots");
        assert_eq!(classify_mount(&slots, &[], false, true), MISSING);
        assert_eq!(classify_mount(&slots, &[], true, true), NOT_MOUNTED);
        assert_eq!(
            classify_mount(&slots, &[], true, false),
            NOT_EMPTY_UNMOUNTED
        );
        let btrfs = [MountEntry {
            mount_point: slots.clone(),
            fstype: "btrfs".to_string(),
            source: "/dev/loop0".to_string(),
            device: "0:7".to_string(),
            root: "/".to_string(),
        }];
        assert_eq!(classify_mount(&slots, &btrfs, true, false), MOUNTED);

        let control = root.join("control");
        std::fs::create_dir_all(&control).expect("control");
        let mut records = BTreeMap::new();
        records.insert(
            "slot-a".to_string(),
            BindRecord {
                subvolume: "slot-a".to_string(),
                mount_id: Some(42),
                target: None,
            },
        );
        save_bind_records(&control, &records).expect("save");
        assert_eq!(load_bind_records(&control).expect("load"), records);

        std::fs::create_dir_all(root.join("tree/sub")).expect("tree");
        std::fs::write(root.join("tree/sub/file.txt"), "abc").expect("file");
        let signature = listing_signature(&root.join("tree"));
        assert!(signature.iter().any(|row| row.0 == "sub" && row.1 == "dir"));
        assert!(signature.iter().any(|row| row.0 == "sub/file.txt"
            && row.1 == "file"
            && row.2 == 3
            && row.4.len() == 64));
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn ops_plans_match_python_argv() {
        use crate::image_ops::RunOutcome;
        use std::cell::RefCell;

        struct Fake {
            calls: RefCell<Vec<Vec<String>>>,
            returncode: i32,
        }
        impl PrivilegedRunner for Fake {
            fn run(&self, spec: &CommandSpec) -> Result<RunOutcome, String> {
                self.calls.borrow_mut().push(spec.argv.clone());
                Ok(RunOutcome {
                    returncode: self.returncode,
                    stdout: String::new(),
                    stderr: "boom".to_string(),
                })
            }
        }

        let file = Path::new("/proj/wt.img");
        let point = Path::new("/proj/worktrees/.slots-image");
        let format = create_format_plan(file);
        assert_eq!(format[0].argv, ["chattr", "+C", "/proj/wt.img"]);
        assert_eq!(format[1].argv[0..3], ["sudo", "-n", "mkfs.btrfs"]);
        let mount = mount_plan(file, point);
        assert!(mount[0]
            .argv
            .contains(&"loop,compress=zstd:3,noatime".to_string()));
        assert_eq!(mount[1].argv[2], "btrfs");
        assert!(mount_refusal(WRONG_MOUNT, point).is_some());
        assert!(mount_refusal(MOUNTED, point).is_none());
        assert_eq!(
            losetup_devices("/dev/loop3: [0806]:1 (/x)\n/dev/loop4: [0806]:2 (/x)\n"),
            ["/dev/loop3", "/dev/loop4"]
        );
        let grow = grow_plan(&["/dev/loop3".to_string()], point);
        assert_eq!(grow.len(), 2);
        assert!(grow_refusal(10, 10).is_some());

        let fake = Fake {
            calls: RefCell::new(Vec::new()),
            returncode: 0,
        };
        run_sequence(&fake, &mount).expect("sequence");
        assert_eq!(fake.calls.borrow().len(), 2);
        let failing = Fake {
            calls: RefCell::new(Vec::new()),
            returncode: 1,
        };
        let error = run_sequence(&failing, &mount).expect_err("failure string");
        assert!(error.contains("failed: boom"), "{error}");
    }

    #[test]
    fn handlers_match_python_outputs() {
        use crate::image_ops::RunOutcome;
        use std::cell::RefCell;
        struct Fake {
            calls: RefCell<Vec<Vec<String>>>,
        }
        impl crate::image_ops::PrivilegedRunner for Fake {
            fn run(&self, spec: &CommandSpec) -> Result<RunOutcome, String> {
                self.calls.borrow_mut().push(spec.argv.clone());
                let stdout = if spec.argv.iter().any(|a| a == "-j") {
                    "/dev/loop3: [0806]:1 (/x)\n".to_string()
                } else {
                    String::new()
                };
                Ok(RunOutcome {
                    returncode: 0,
                    stdout,
                    stderr: String::new(),
                })
            }
        }
        let root = temp_root("handlers");
        let control = root.join("control");
        let worktrees = root.join("worktrees");
        std::fs::create_dir_all(&control).expect("control");
        std::fs::create_dir_all(&worktrees).expect("worktrees");
        let settings = WorktreesImageSettings {
            relative_path: "wt.img".to_string(),
            size_bytes: 2 * 1_073_741_824,
            filesystem: "btrfs".to_string(),
            mount: "kernel".to_string(),
            slot_quota_bytes: None,
            machines: None,
            mount_at: "binds".to_string(),
            quota_mode: "simple".to_string(),
        };
        let ctx = WtCtx {
            root: &root,
            control: &control,
            worktrees: &worktrees,
            entries: &[],
            skipped: None,
        };

        // Unconfigured refuses for every handler entry point.
        assert_eq!(wt_status(&ctx, None, "human").exit, 3);

        // Status on a sparse temp image, unmounted (empty mount dir).
        std::fs::create_dir_all(root.join(IMAGE_MOUNT_NAME)).expect("mount dir");
        std::fs::File::create(settings.image_file(&root))
            .expect("img")
            .set_len(2 * 1_073_741_824)
            .expect("size");
        let human = wt_status(&ctx, Some(settings.clone()), "human");
        assert!(
            human.stdout.contains("state=not-mounted"),
            "{}",
            human.stdout
        );
        assert!(
            human
                .stdout
                .contains("remedy: wrkslots worktrees-image mount"),
            "{}",
            human.stdout
        );
        let json_out = wt_status(&ctx, Some(settings.clone()), "json");
        assert!(
            json_out.stdout.contains("\"state\": \"not-mounted\""),
            "{}",
            json_out.stdout
        );

        let fake = Fake {
            calls: RefCell::new(Vec::new()),
        };
        let created = wt_create(&ctx, Some(settings.clone()), &fake);
        assert_eq!(created.exit, 3); // already exists from the status fixture
        std::fs::remove_file(settings.image_file(&root)).expect("remove");
        let created = wt_create(&ctx, Some(settings.clone()), &fake);
        assert_eq!(created.exit, 0, "{}", created.stderr);
        assert!(created.stdout.starts_with("created "));

        let fake = Fake {
            calls: RefCell::new(Vec::new()),
        };
        let mounted = wt_mount(&ctx, Some(settings.clone()), &|_| Some(42), &fake);
        assert_eq!(mounted.exit, 0, "{}", mounted.stderr);
        let fake = Fake {
            calls: RefCell::new(Vec::new()),
        };
        let grown = wt_grow(&ctx, Some(settings.clone()), 4 * 1_073_741_824, &fake);
        assert_eq!(grown.exit, 0, "{}", grown.stderr);
        assert!(
            grown.stdout.contains("to 4294967296 bytes"),
            "{}",
            grown.stdout
        );
        let trimmed = wt_trim(&ctx, Some(settings.clone()), &fake);
        assert_eq!(trimmed.stdout, "trimmed\n");
        let unmounted = wt_unmount(&ctx, Some(settings), &fake);
        assert_eq!(unmounted.exit, 0);
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn unbind_drops_record_only_after_successful_unmount() {
        use crate::image_ops::RunOutcome;
        struct Fake {
            returncode: i32,
        }
        impl crate::image_ops::PrivilegedRunner for Fake {
            fn run(&self, _spec: &CommandSpec) -> Result<RunOutcome, String> {
                Ok(RunOutcome {
                    returncode: self.returncode,
                    stdout: String::new(),
                    stderr: "unmount failed".to_string(),
                })
            }
        }
        let root = temp_root("unbind");
        let control = root.join("control");
        let worktrees = root.join("worktrees");
        std::fs::create_dir_all(&control).expect("control");
        std::fs::create_dir_all(&worktrees).expect("worktrees");
        let slot_path = worktrees.join("slot-a");
        std::fs::create_dir_all(&slot_path).expect("slot");
        let mut records = BTreeMap::new();
        records.insert(
            "slot-a".to_string(),
            BindRecord {
                subvolume: "slot-a".to_string(),
                mount_id: Some(9),
                target: Some(slot_path.to_string_lossy().into_owned()),
            },
        );
        save_bind_records(&control, &records).expect("seed record");
        let ctx = WtCtx {
            root: &root,
            control: &control,
            worktrees: &worktrees,
            entries: &[],
            skipped: None,
        };
        let failed = unbind_slot(&ctx, "slot-a", &slot_path, &Fake { returncode: 1 });
        assert_eq!(failed.exit, 3);
        assert!(
            load_bind_records(&control)
                .expect("records after failed unmount")
                .contains_key("slot-a"),
            "failed unmount must keep the bind record"
        );
        let done = unbind_slot(&ctx, "slot-a", &slot_path, &Fake { returncode: 0 });
        assert_eq!(done.exit, 0, "{}", done.stderr);
        assert!(
            !load_bind_records(&control)
                .expect("records after unmount")
                .contains_key("slot-a"),
            "successful unmount must drop the bind record"
        );
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn migrate_handlers_match_contract() {
        use crate::image_ops::RunOutcome;
        use std::cell::RefCell;
        struct Fake {
            calls: RefCell<Vec<Vec<String>>>,
        }
        impl crate::image_ops::PrivilegedRunner for Fake {
            fn run(&self, spec: &CommandSpec) -> Result<RunOutcome, String> {
                self.calls.borrow_mut().push(spec.argv.clone());
                Ok(RunOutcome {
                    returncode: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                })
            }
        }
        let root = temp_root("migrate");
        let control = root.join("control");
        let worktrees = root.join("worktrees");
        let mount_dir = image_mount_dir(&control);
        std::fs::create_dir_all(&mount_dir).expect("mount dir");
        let slot_path = worktrees.join("slot-a");
        std::fs::create_dir_all(slot_path.join("sub")).expect("slot");
        std::fs::write(slot_path.join("sub/file.txt"), "abc").expect("file");
        let settings = WorktreesImageSettings {
            relative_path: "wt.img".into(),
            size_bytes: 1073741824,
            filesystem: "btrfs".into(),
            mount: "kernel".into(),
            slot_quota_bytes: None,
            machines: None,
            mount_at: "binds".to_string(),
            quota_mode: "simple".to_string(),
        };
        let ctx = WtCtx {
            root: &root,
            control: &control,
            worktrees: &worktrees,
            entries: &[],
            skipped: None,
        };
        let fake = Fake {
            calls: RefCell::new(Vec::new()),
        };
        let outcome = wt_migrate(
            &ctx,
            Some(settings.clone()),
            "slot-a",
            &slot_path,
            MOUNTED,
            false,
            (1000, 1000),
            "0123abcd0123abcd",
            &|_: &Path| Ok(()),
            &|_| Some(9),
            &fake,
        );
        assert_eq!(outcome.exit, 0, "{}", outcome.stderr);
        assert!(outcome.stdout.contains("original reclaimed"));
        assert!(!worktrees
            .join(".slot-a.premigrate-0123abcd0123abcd")
            .exists());
        assert!(mount_dir.join("slot-a/sub/file.txt").exists());
        std::fs::create_dir_all(&slot_path).expect("bound slot dir");
        let again = wt_migrate(
            &ctx,
            Some(settings.clone()),
            "slot-a",
            &slot_path,
            MOUNTED,
            false,
            (1000, 1000),
            "0123abcd0123abcd",
            &|_: &Path| Ok(()),
            &|_| Some(9),
            &fake,
        );
        assert!(again.stdout.contains("already bound"));
        let unmounted = wt_migrate(
            &ctx,
            Some(settings),
            "slot-b",
            &slot_path,
            NOT_MOUNTED,
            false,
            (1000, 1000),
            "0123abcd0123abcd",
            &|_: &Path| Ok(()),
            &|_| None,
            &fake,
        );
        assert_eq!(unmounted.exit, 3);
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn machines_scope_matches_python() {
        let base = json!({"path": "wt.img", "size_bytes": 1073741824});
        let mut scoped = base.clone();
        scoped["machines"] = json!(["image-host"]);
        let settings = settings_from_obj(&scoped).expect("scoped");
        assert!(settings.applies_on("image-host"));
        assert!(!settings.applies_on("donor-host"));
        assert_eq!(
            settings.skipped_reason("donor-host").as_deref(),
            Some("configuration.worktrees_image applies on machines [image-host] only; this host is donor-host")
        );
        let root = Path::new("/proj");
        let worktrees = Path::new("/proj/worktrees");
        let (named, skipped) =
            parse_worktrees_image(Some(&scoped), root, worktrees, "image-host").expect("named");
        assert!(named.is_some() && skipped.is_none());
        let (unnamed, reason) =
            parse_worktrees_image(Some(&scoped), root, worktrees, "donor-host").expect("unnamed");
        assert!(unnamed.is_none());
        assert!(reason.expect("reason").contains("image-host"));
        let (unscoped, skipped) =
            parse_worktrees_image(Some(&base), root, worktrees, "any-host").expect("unscoped");
        assert!(unscoped.is_some() && skipped.is_none());
        for machines in [
            json!([]),
            json!("image-host"),
            json!([""]),
            json!(["  "]),
            json!([7]),
            json!(["a", "a"]),
            json!(["a", 1]),
        ] {
            let mut bad = base.clone();
            bad["machines"] = machines;
            assert!(settings_from_obj(&bad).is_err(), "{bad}");
        }
        // Handler refusal on an unlisted host has Python's wording.
        let root = temp_root("scope");
        let control = root.join("control");
        let worktrees = root.join("worktrees");
        let ctx = WtCtx {
            root: &root,
            control: &control,
            worktrees: &worktrees,
            entries: &[],
            skipped: Some("configuration.worktrees_image applies on machines [image-host] only; this host is donor-host"),
        };
        let outcome = wt_status(&ctx, None, "human");
        assert_eq!(outcome.exit, 3);
        assert!(outcome.stderr.contains("worktrees_image does not apply on this host: configuration.worktrees_image applies on machines [image-host] only"), "{}", outcome.stderr);
        std::fs::remove_dir_all(&root).expect("cleanup");
    }
}
