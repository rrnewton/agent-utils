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
}

impl WorktreesImageSettings {
    pub(crate) fn image_file(&self, root: &Path) -> PathBuf {
        root.join(&self.relative_path)
    }
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
    let known = ["path", "size_bytes", "fs", "mount", "slot_quota_bytes"];
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
    if let Some(quota) = quota {
        if quota.as_i64().map(|value| value < 1).unwrap_or(true) {
            return Err(
                "configuration.worktrees_image.slot_quota_bytes must be a positive integer"
                    .to_string(),
            );
        }
    }
    Ok(WorktreesImageSettings {
        relative_path: raw_path.replace('\\', "/"),
        size_bytes: size.unwrap_or_default(),
        filesystem: filesystem.to_string(),
        mount: mount.to_string(),
        slot_quota_bytes: quota.and_then(Value::as_i64),
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

/// Persist bind records atomically, mirroring `save_bind_records`
/// (sorted keys, indent 2, trailing newline, temp-then-replace).
pub(crate) fn save_bind_records(
    control: &Path,
    records: &BTreeMap<String, BindRecord>,
) -> Result<(), String> {
    let path = record_path(control);
    let temporary = control.join(format!("{RECORD_NAME}.tmp"));
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
    std::fs::write(&temporary, text)
        .map_err(|error| format!("cannot write worktrees image record {temporary:?}: {error}"))?;
    std::fs::rename(&temporary, &path)
        .map_err(|error| format!("cannot replace worktrees image record {path:?}: {error}"))
}

/// Sorted `(relative path, kind, size)` for every migratable entry
/// under a temp/fixture tree, mirroring Python `listing_signature`.
/// Special files are skipped, as in the oracle.
pub(crate) fn listing_signature(root: &Path) -> Vec<(String, String, u64)> {
    let mut signature = Vec::new();
    fn walk(root: &Path, current: &Path, out: &mut Vec<(String, String, u64)>) {
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
            let file_type = entry.symlink_metadata().map(|meta| meta.file_type());
            match file_type {
                Ok(kind) if kind.is_symlink() => {
                    let size = std::fs::read_link(&entry)
                        .map(|target| target.as_os_str().len() as u64)
                        .unwrap_or(0);
                    out.push((relative, "link".to_string(), size));
                }
                Ok(kind) if kind.is_dir() => {
                    out.push((relative.clone(), "dir".to_string(), 0));
                    walk(root, &entry, out);
                }
                Ok(kind) if kind.is_file() => {
                    let size = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
                    out.push((relative, "file".to_string(), size));
                }
                _ => {}
            }
        }
    }
    walk(root, root, &mut signature);
    signature.sort();
    signature
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

fn wt_settings(
    settings: Option<WorktreesImageSettings>,
) -> Result<WorktreesImageSettings, HandlerOutcome> {
    settings.ok_or_else(|| {
        wt_refusal(
            "no worktrees_image configured; add a worktrees_image section (path, size_bytes, fs: btrfs, mount: kernel) to enable it",
        )
    })
}

fn run_one(
    runner: &dyn crate::image_ops::PrivilegedRunner,
    spec: &CommandSpec,
) -> Result<crate::image_ops::RunOutcome, HandlerOutcome> {
    match runner.run(spec) {
        Ok(outcome) if outcome.returncode == 0 => Ok(outcome),
        Ok(outcome) => {
            let detail = if outcome.stderr.trim().is_empty() {
                outcome.stdout.trim()
            } else {
                outcome.stderr.trim()
            };
            Err(wt_refusal(&format!(
                "{} failed: {detail}",
                spec.argv.join(" ")
            )))
        }
        Err(message) => Err(wt_refusal(&message)),
    }
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
    use std::os::unix::fs::MetadataExt as _;
    let image_file = settings.image_file(ctx.root);
    let mount_dir = image_mount_dir(ctx.control);
    let (exists, empty) = dir_facts(&mount_dir);
    let state = classify_mount(&mount_dir, ctx.entries, exists, empty);
    let mut report = serde_json::Map::new();
    report.insert("image_file".into(), json!(image_file.to_string_lossy()));
    report.insert("mount_point".into(), json!(mount_dir.to_string_lossy()));
    report.insert("configured_size_bytes".into(), json!(settings.size_bytes));
    report.insert("state".into(), json!(state));
    let is_file = image_file.is_file();
    report.insert("image_exists".into(), json!(is_file));
    let binds: serde_json::Map<String, Value> = load_bind_records(ctx.control)
        .unwrap_or_default()
        .into_iter()
        .map(|(slot, record)| (slot, json!(record.subvolume)))
        .collect();
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
    let settings = match wt_settings(settings) {
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
        "configured_size_bytes",
        "state",
        "image_exists",
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
    let settings = match wt_settings(settings) {
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
    let settings = match wt_settings(settings) {
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
    // ensure_binds at plan level: bind each recorded slot whose
    // target is not in the injected mount table.
    if let Ok(mut records) = load_bind_records(ctx.control) {
        let mounted_points: Vec<PathBuf> = ctx
            .entries
            .iter()
            .map(|e| crate::image_mounts::normalize_absolute(&e.mount_point))
            .collect();
        let mut changed = false;
        for (slot, record) in records.clone() {
            let target = record
                .target
                .clone()
                .map(PathBuf::from)
                .unwrap_or_else(|| ctx.worktrees.join(&slot));
            if mounted_points.contains(&crate::image_mounts::normalize_absolute(&target)) {
                continue;
            }
            let source = mount_dir.join(&record.subvolume);
            if !source.is_dir() {
                return wt_refusal(&format!("slot subvolume is missing: {}", source.display()));
            }
            let _ = std::fs::create_dir_all(&target);
            for spec in bind_plan(&source, &target) {
                if let Err(outcome) = run_one(runner, &spec) {
                    return outcome;
                }
            }
            records.insert(
                slot.clone(),
                BindRecord {
                    subvolume: record.subvolume,
                    mount_id: mount_id(&target),
                    target: Some(target.to_string_lossy().into_owned()),
                },
            );
            changed = true;
        }
        if changed {
            let _ = save_bind_records(ctx.control, &records);
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
    let settings = match wt_settings(settings) {
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
    let settings = match wt_settings(settings) {
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
    let settings = match wt_settings(settings) {
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
        assert!(signature.contains(&("sub".to_string(), "dir".to_string(), 0)));
        assert!(signature.contains(&("sub/file.txt".to_string(), "file".to_string(), 3)));
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
        };
        let ctx = WtCtx {
            root: &root,
            control: &control,
            worktrees: &worktrees,
            entries: &[],
        };

        // Unconfigured refuses for every handler entry point.
        assert_eq!(wt_status(&ctx, None, "human").exit, 3);

        // Status on a sparse temp image, unmounted (empty mount dir).
        std::fs::create_dir_all(image_mount_dir(&control)).expect("mount dir");
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
}
