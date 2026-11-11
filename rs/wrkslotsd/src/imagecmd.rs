//! Dispatch-shaped handlers for the `image` family (#1095, mu-rsimg).
//!
//! Each handler takes injected values plus the privileged seam and
//! returns `(exit, stdout, stderr)` byte-identical to the Python
//! edition on the same inputs, so the shared core can wire it with
//! one dispatch arm. Handlers are unwired until that arm lands.
//! Status needs no privilege: its runner argument is accepted for
//! the family shape and never invoked. Config loading is the
//! caller's: the Rust loader never activates (mounts) images, so
//! unlike the Python CLI path this handler never mounts as a side
//! effect of reading status (the deliberate divergence recorded in
//! the milestone-1 inventory); on identical post-load inputs its
//! bytes match the Python handler.
//!
//! Handoff: the CLI-level `test_image_status_rust.py` differential
//! (both editions through the binary) lands with the dispatch
//! wiring; until then the oracle differential in this module's
//! tests (Python driven as a subprocess on the same fixture) is
//! the byte-parity proof.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::image_mounts::MountEntry;
use crate::image_ops::{PrivilegedRunner, RunOutcome};
use crate::image_record::{all_images, ImageRecord};
use crate::image_status::{usage_report, VfsFacts};

/// One handler result: process exit code and the exact bytes the
/// Python edition would print to stdout and stderr.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HandlerOutcome {
    pub(crate) exit: i32,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

/// Resolves, per image file and mount point, the mounted backend
/// (`kernel`/`fuse`) and the statvfs facts when mounted.
pub(crate) type MountFactsFn<'a> = dyn Fn(&Path, &Path) -> (Option<String>, Option<VfsFacts>) + 'a;

fn refusal(message: &str, command: &str) -> HandlerOutcome {
    HandlerOutcome {
        exit: 3,
        stdout: String::new(),
        stderr: format!(
            "REFUSED: {message}\nREMEDY: run 'wrkslots {command} --help', correct the named condition, then rerun 'wrkslots {command}' with the required arguments\n"
        ),
    }
}

/// Escape a string the way Python `json.dumps` does by default
/// (`ensure_ascii=True`): quotes, backslash, control characters,
/// and every non-ASCII scalar as `\uXXXX` (with surrogate pairs).
fn py_json_string(text: &str) -> String {
    let mut out = String::from("\"");
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            ch if (ch as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch if (ch as u32) < 0x7f => out.push(ch),
            ch => {
                let mut buffer = [0_u16; 2];
                for unit in ch.encode_utf16(&mut buffer) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }
    }
    out.push('"');
    out
}

fn py_json_value(value: &Value, indent: usize) -> String {
    // Only the shapes `usage_report` produces reach this renderer:
    // flat objects with string keys and scalar/array/object values.
    match value {
        Value::Object(map) => {
            if map.is_empty() {
                return "{}".to_string();
            }
            let pad = "  ".repeat(indent + 1);
            let close_pad = "  ".repeat(indent);
            let parts: Vec<String> = map
                .iter()
                .map(|(key, item)| {
                    format!(
                        "{pad}{}: {}",
                        py_json_string(key),
                        py_json_value(item, indent + 1)
                    )
                })
                .collect();
            format!("{{\n{}\n{close_pad}}}", parts.join(",\n"))
        }
        Value::Array(items) => {
            if items.is_empty() {
                return "[]".to_string();
            }
            let pad = "  ".repeat(indent + 1);
            let close_pad = "  ".repeat(indent);
            let parts: Vec<String> = items
                .iter()
                .map(|item| format!("{pad}{}", py_json_value(item, indent + 1)))
                .collect();
            format!("[\n{}\n{close_pad}]", parts.join(",\n"))
        }
        Value::String(text) => py_json_string(text),
        other => other.to_string(),
    }
}

/// The JSON rendering of `image status`, in the Python dict's
/// insertion order (top level and per-report), which a sorted map
/// would not reproduce.
fn render_wt_report(report: &Value) -> String {
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
                            .map(|(name, item)| {
                                format!(
                                    "      {}: {}",
                                    py_json_string(name),
                                    py_json_value(item, 3)
                                )
                            })
                            .collect();
                        format!("{{\n{}\n    }}", inner.join(",\n"))
                    }
                    other => py_json_value(other, 2),
                };
                format!("    {}: {}", py_json_string(key), rendered)
            })
        })
        .collect();
    format!("{{\n{}\n  }}", parts.join(",\n"))
}

fn render_status_json_ordered(
    new_slot_representation: &str,
    reports: &[Value],
    wt_report: Option<&Value>,
) -> String {
    let image_parts: Vec<String> = reports
        .iter()
        .map(|report| {
            let entry = |label: &str| -> String {
                let Some(entry) = report.get(label) else {
                    return "null".to_string();
                };
                let keys = [
                    "image",
                    "ceiling_bytes",
                    "host_allocated_bytes",
                    "error",
                    "mounted",
                    "backend",
                    "used_bytes",
                    "free_bytes",
                    "used_inodes",
                ];
                let parts: Vec<String> = keys
                    .iter()
                    .filter_map(|key| {
                        entry.get(key).map(|value| {
                            format!("        {}: {}", py_json_string(key), py_json_value(value, 4))
                        })
                    })
                    .collect();
                format!("{{\n{}\n      }}", parts.join(",\n"))
            };
            format!(
                "    {{\n      \"name\": {},\n      \"slot_type\": {},\n      \"location\": {},\n      \"phase\": {},\n      \"slot\": {},\n      \"state\": {}\n    }}",
                py_json_value(report.get("name").unwrap_or(&Value::Null), 1),
                py_json_value(report.get("slot_type").unwrap_or(&Value::Null), 1),
                py_json_value(report.get("location").unwrap_or(&Value::Null), 1),
                py_json_value(report.get("phase").unwrap_or(&Value::Null), 1),
                entry("slot"),
                entry("state"),
            )
        })
        .collect();
    let wt_part = match wt_report {
        Some(report) => format!(",\n  \"worktrees_image\": {}", render_wt_report(report)),
        None => String::new(),
    };
    format!(
        "{{\n  \"new_slot_representation\": {},\n  \"images\": [{}]{}\n}}\n",
        py_json_string(new_slot_representation),
        if image_parts.is_empty() {
            String::new()
        } else {
            format!("\n{}\n  ", image_parts.join(",\n"))
        },
        wt_part
    )
}

/// `wrkslots image status [--format human|json]`, mirroring Python
/// `_cmd_image_status` (`imagecmd.py:865`). `facts` resolves, per
/// image file and mount point, the mounted backend and statvfs
/// facts; production derives them from the live mount table, tests
/// inject fixture facts. `_runner` keeps the family signature;
/// status never invokes it.
pub(crate) fn image_status(
    control: &Path,
    slot_path: &dyn Fn(&str, &str) -> PathBuf,
    new_slot_representation: &str,
    format: &str,
    _entries: &[MountEntry],
    facts: &MountFactsFn<'_>,
    _runner: &dyn PrivilegedRunner,
) -> HandlerOutcome {
    image_status_wt(
        control,
        slot_path,
        new_slot_representation,
        format,
        _entries,
        facts,
        _runner,
        None,
    )
}

/// `image status` with the conditional worktrees-image report
/// (#1279): present only when the section applies on this host.
#[allow(clippy::too_many_arguments)]
pub(crate) fn image_status_wt(
    control: &Path,
    slot_path: &dyn Fn(&str, &str) -> PathBuf,
    new_slot_representation: &str,
    format: &str,
    _entries: &[MountEntry],
    facts: &MountFactsFn<'_>,
    _runner: &dyn PrivilegedRunner,
    wt_report: Option<&Value>,
) -> HandlerOutcome {
    let images: Vec<ImageRecord> = match all_images(control, slot_path) {
        Ok(images) => images,
        Err(message) => return refusal(&message, "image"),
    };
    let reports: Vec<Value> = images
        .iter()
        .map(|record| {
            // The core takes backend and vfs closures separately;
            // both resolve through the one injected facts closure,
            // keyed by the record's own image/mount pairs.
            let pairs = [
                (record.image_file(), record.location.clone()),
                (record.state_image(), record.state_mount()),
            ];
            usage_report(
                record,
                &|image_file: &Path, mount_point: &Path| facts(image_file, mount_point).0,
                &|mount_point: &Path| {
                    pairs
                        .iter()
                        .find(|(_, mount)| mount == mount_point)
                        .and_then(|(image_file, mount)| facts(image_file, mount).1)
                },
            )
        })
        .collect();
    if format == "json" {
        return HandlerOutcome {
            exit: 0,
            stdout: render_status_json_ordered(new_slot_representation, &reports, wt_report),
            stderr: String::new(),
        };
    }
    let mut human = crate::image_status::render_status_human(new_slot_representation, &reports);
    if let Some(report) = wt_report {
        let line = format!(
            "worktrees image: {} at {} (mount_at={})\n",
            report
                .get("state")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            report
                .get("mount_point")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            report
                .get("mount_at")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
        if let Some(end) = human.find('\n') {
            human.insert_str(end + 1, &line);
        }
    }
    HandlerOutcome {
        exit: 0,
        stdout: human,
        stderr: String::new(),
    }
}

/// Load one slot's image record for the mount family, mirroring
/// Python `_load_image`: a bad record or a non-image slot refuses.
fn load_image(
    control: &Path,
    slot_path: &dyn Fn(&str, &str) -> PathBuf,
    slot_type: &str,
    slot: &str,
) -> Result<ImageRecord, HandlerOutcome> {
    let expected = slot_path(slot_type, slot);
    match crate::image_record::load_slot_image(control, slot_type, slot, &expected) {
        Ok(Some(record)) => Ok(record),
        Ok(None) => Err(refusal(
            &format!("{slot_type} slot '{slot}' is not image-backed"),
            "image",
        )),
        Err(message) => Err(refusal(&message, "image")),
    }
}

/// `wrkslots image set-default {worktree|image}` (#1116): the pure
/// config decision. Returns the outcome and the updated raw config
/// object for the caller's locked config writer (which preserves
/// the file's format and backup, as Python `_write_config` does).
pub(crate) fn image_set_default(
    representation: &str,
    raw: &Value,
) -> (HandlerOutcome, Option<Value>) {
    let Some(mut updated) = raw.as_object().cloned() else {
        return (refusal("configuration must be an object", "image"), None);
    };
    if representation == "worktree" {
        updated.remove("slot_representation");
    } else {
        updated.insert(
            "slot_representation".to_string(),
            Value::String(representation.to_string()),
        );
    }
    (
        HandlerOutcome {
            exit: 0,
            stdout: format!(
                "new slots will use the {representation} representation; existing slots are unchanged (see `wrkslots image convert`)\n"
            ),
            stderr: String::new(),
        },
        Some(Value::Object(updated)),
    )
}

/// Shared handler inputs for the mount family.
pub(crate) struct MountCtx<'a> {
    pub(crate) control: &'a Path,
    pub(crate) slot_path: &'a dyn Fn(&str, &str) -> PathBuf,
    pub(crate) tools: &'a crate::image_ops::ToolPaths,
}

/// `wrkslots image trim` (#1116): one line per mounted image,
/// mirroring `_cmd_image_trim`. Facts resolve the mounted backend
/// per image file and mount point.
pub(crate) fn image_trim(
    ctx: &MountCtx<'_>,
    slot_type: &str,
    slot: &str,
    facts: &MountFactsFn<'_>,
    runner: &dyn PrivilegedRunner,
) -> HandlerOutcome {
    let (control, slot_path, tools) = (ctx.control, ctx.slot_path, ctx.tools);
    let record = match load_image(control, slot_path, slot_type, slot) {
        Ok(record) => record,
        Err(outcome) => return outcome,
    };
    let slot_backend = facts(&record.image_file(), &record.location).0;
    let state_backend = facts(&record.state_image(), &record.state_mount()).0;
    let plan = match crate::image_ops::trim_plan(
        tools,
        &record,
        slot_backend.as_deref(),
        state_backend.as_deref(),
        crate::image_ops::BoxKind::None,
    ) {
        Ok(plan) => plan,
        Err(message) => return refusal(&message, "image"),
    };
    match crate::image_ops::run_trim(runner, &plan) {
        Ok(lines) => HandlerOutcome {
            exit: 0,
            stdout: lines.iter().map(|line| format!("{line}\n")).collect(),
            stderr: String::new(),
        },
        Err(message) => refusal(&message, "image"),
    }
}

/// `wrkslots image grow --ceiling-gib N` (#1116), mirroring
/// `_cmd_image_grow` and `slotimage.grow`'s step order: shrink
/// refuses; a kernel image grows online (truncate, helper grow);
/// otherwise offline (truncate, e2fsck, resize2fs); the record is
/// rewritten with the new ceiling. Mount steps that Python's
/// ensure-mounted performs around the resize are the runner's
/// recorded commands in the plan built by the caller at dispatch.
pub(crate) fn image_grow(
    ctx: &MountCtx<'_>,
    slot_type: &str,
    slot: &str,
    ceiling_gib: i64,
    facts: &MountFactsFn<'_>,
    runner: &dyn PrivilegedRunner,
) -> HandlerOutcome {
    let (control, slot_path, tools) = (ctx.control, ctx.slot_path, ctx.tools);
    let record = match load_image(control, slot_path, slot_type, slot) {
        Ok(record) => record,
        Err(outcome) => return outcome,
    };
    let image_file = record.image_file();
    let current = match std::fs::metadata(&image_file) {
        Ok(meta) => meta.len(),
        Err(error) => {
            return HandlerOutcome {
                exit: 1,
                stdout: String::new(),
                stderr: format!("{error}\n"),
            };
        }
    };
    let new_ceiling = (ceiling_gib * 1_073_741_824) as u64;
    if let Some(message) = crate::image_ops::grow_refusal(current, new_ceiling) {
        return refusal(&message, "image");
    }
    let slot_backend = facts(&image_file, &record.location).0;
    let mode = if record.backend == "kernel" || slot_backend.as_deref() == Some("kernel") {
        crate::image_ops::GrowMode::OnlineKernel
    } else {
        crate::image_ops::GrowMode::Offline
    };
    // Python truncates before the resize steps in both modes.
    let file = match std::fs::OpenOptions::new().write(true).open(&image_file) {
        Ok(file) => file,
        Err(error) => {
            return HandlerOutcome {
                exit: 1,
                stdout: String::new(),
                stderr: format!("{error}\n"),
            };
        }
    };
    if let Err(error) = file.set_len(new_ceiling) {
        return HandlerOutcome {
            exit: 1,
            stdout: String::new(),
            stderr: format!("{error}\n"),
        };
    }
    drop(file);
    let plan =
        match crate::image_ops::grow_plan(tools, &record, mode, crate::image_ops::BoxKind::None) {
            Ok(plan) => plan,
            Err(message) => return refusal(&message, "image"),
        };
    for item in &plan {
        match runner.run(item) {
            Ok(outcome) if outcome.returncode == 0 => {}
            Ok(outcome) => {
                let detail = if outcome.stderr.trim().is_empty() {
                    outcome.stdout.trim()
                } else {
                    outcome.stderr.trim()
                };
                return refusal(
                    &format!("{} failed: {detail}", item.argv.join(" ")),
                    "image",
                );
            }
            Err(message) => return refusal(&message, "image"),
        }
    }
    let mut grown = record.clone();
    grown.ceiling_bytes = new_ceiling as i64;
    if let Err(message) = crate::image_record::write_record(&grown) {
        return refusal(&message, "image");
    }
    HandlerOutcome {
        exit: 0,
        stdout: format!(
            "{slot_type}/{slot} ceiling is now {} GiB\n",
            new_ceiling / 1_073_741_824
        ),
        stderr: String::new(),
    }
}

/// Mount-state facts for one image file and mount point:
/// `(state, detail, backend)` as `image_mounts::classify_mount`
/// and Python `mount_state` report them.
pub(crate) type MountStateFn<'a> = dyn Fn(&Path, &Path) -> (String, String, Option<String>) + 'a;

/// Why a discarding image is not mounted, mirroring Python
/// `discard_hold_message`.
fn discard_hold_message(record: &ImageRecord, held: &[String]) -> String {
    format!(
        "slot image {} was unmounted while a stale copy of it was being discarded{}; it is not mounted again: a mount would reuse a loop device that may hold the stale file system, and a stale copy released after the host's own file system may have written its old metadata into the image. remedy: stop the processes whose mount table holds that device; once no loop device holds the image, check the image file (e2fsck -fn) and run 'wrkslots image mount --coordinator-authorized', which mounts it from the file only",
        record.image_file().display(),
        if held.is_empty() {
            String::new()
        } else {
            format!(", and {} still holds it", held.join(", "))
        },
    )
}

fn run_or_refusal(
    runner: &dyn PrivilegedRunner,
    spec: &crate::image_ops::CommandSpec,
) -> Result<RunOutcome, HandlerOutcome> {
    match runner.run(spec) {
        Ok(outcome) if outcome.returncode == 0 => Ok(outcome),
        Ok(outcome) => {
            let detail = if outcome.stderr.trim().is_empty() {
                outcome.stdout.trim()
            } else {
                outcome.stderr.trim()
            };
            Err(refusal(
                &format!("{} failed: {detail}", spec.argv.join(" ")),
                "image",
            ))
        }
        Err(message) => Err(refusal(&message, "image")),
    }
}

/// `wrkslots image mount [--coordinator-authorized]` (#1116):
/// mount every unmounted image, collecting per-slot failures the
/// way `ensure_all_mounted` does (one failure does not stop the
/// others; any failure refuses after the rest were mounted).
/// `held` names loop devices still holding an image; `backend_for`
/// resolves a record's backend preference the way Python
/// `resolve_backend` does on this host.
#[allow(clippy::too_many_arguments)]
pub(crate) fn image_mount(
    ctx: &MountCtx<'_>,
    remount_discarded: bool,
    state: &MountStateFn<'_>,
    held: &dyn Fn(&Path) -> Vec<String>,
    backend_for: &dyn Fn(&ImageRecord) -> Result<String, String>,
    fuse_unit: &dyn Fn(&Path) -> (String, String),
    runner: &dyn PrivilegedRunner,
) -> HandlerOutcome {
    let images = match all_images(ctx.control, ctx.slot_path) {
        Ok(images) => images,
        Err(message) => return refusal(&message, "image"),
    };
    let mut actions: Vec<String> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    for record in &images {
        match ensure_image(
            ctx,
            record,
            remount_discarded,
            state,
            held,
            backend_for,
            fuse_unit,
            runner,
        ) {
            Ok(done) => actions.extend(done),
            Err(message) => {
                failures.push(format!("{}/{}: {message}", record.slot_type, record.slot))
            }
        }
    }
    if !failures.is_empty() {
        return refusal(
            &format!(
                "{} image-backed slot(s) could not be mounted; every other slot was: {}",
                failures.len(),
                failures.join("; ")
            ),
            "image",
        );
    }
    HandlerOutcome {
        exit: 0,
        stdout: format!(
            "{}\n",
            if actions.is_empty() {
                "no image-backed slot needed mounting".to_string()
            } else {
                actions.join("\n")
            }
        ),
        stderr: String::new(),
    }
}

#[allow(clippy::too_many_arguments)]
fn ensure_image(
    ctx: &MountCtx<'_>,
    record: &ImageRecord,
    remount_discarded: bool,
    state: &MountStateFn<'_>,
    held: &dyn Fn(&Path) -> Vec<String>,
    backend_for: &dyn Fn(&ImageRecord) -> Result<String, String>,
    fuse_unit: &dyn Fn(&Path) -> (String, String),
    runner: &dyn PrivilegedRunner,
) -> Result<Vec<String>, String> {
    let pairs = [
        (record.image_file(), record.location.clone()),
        (record.state_image(), record.state_mount()),
    ];
    if record.phase == crate::image_record::DISCARD_PHASE {
        let pending: Vec<&(PathBuf, PathBuf)> = pairs
            .iter()
            .filter(|(file, mount)| state(file, mount).2.is_none())
            .collect();
        if pending.is_empty() {
            return Ok(Vec::new());
        }
        let held_labels: Vec<String> = pending.iter().flat_map(|(file, _)| held(file)).collect();
        if !held_labels.is_empty() || !remount_discarded {
            return Err(discard_hold_message(record, &held_labels));
        }
        let mut actions = Vec::new();
        for (file, mount) in pending {
            if !file.exists() {
                return Err(format!("slot image file is missing: {}", file.display()));
            }
            mount_one(
                ctx,
                record,
                file,
                mount,
                true,
                state,
                backend_for,
                fuse_unit,
                runner,
            )?;
            actions.push(format!(
                "mounted {} at {} from the image file, which a stale-copy discard had left unmounted; a stale copy released since may have written its old metadata into it, so check its content before relying on it",
                file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                mount.display()
            ));
        }
        let mut ready = record.clone();
        ready.phase = "ready".to_string();
        crate::image_record::write_record(&ready)?;
        return Ok(actions);
    }
    if record.phase != "ready" {
        return Ok(Vec::new());
    }
    let mut actions = Vec::new();
    for (file, mount) in &pairs {
        if state(file, mount).2.is_some() {
            continue;
        }
        if !file.exists() {
            return Err(format!("slot image file is missing: {}", file.display()));
        }
        let reused = mount_one(
            ctx,
            record,
            file,
            mount,
            false,
            state,
            backend_for,
            fuse_unit,
            runner,
        )?;
        actions.push(
            format!(
                "mounted {} at {}",
                file.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                mount.display()
            ) + &reused
                .map(|dev| {
                    format!(" (reusing {dev}, which a copy in another mount namespace still holds)")
                })
                .unwrap_or_default(),
        );
    }
    Ok(actions)
}

#[allow(clippy::too_many_arguments)]
fn mount_one(
    ctx: &MountCtx<'_>,
    record: &ImageRecord,
    image_file: &Path,
    mount_point: &Path,
    fresh: bool,
    state: &MountStateFn<'_>,
    backend_for: &dyn Fn(&ImageRecord) -> Result<String, String>,
    fuse_unit: &dyn Fn(&Path) -> (String, String),
    runner: &dyn PrivilegedRunner,
) -> Result<Option<String>, String> {
    let (mount_state, detail, _backend) = state(image_file, mount_point);
    if mount_state != "absent" {
        return Err(format!(
            "{} already has a different file system mounted on it: {detail}",
            mount_point.display()
        ));
    }
    if mount_point.is_symlink() || !mount_point.is_dir() {
        return Err(format!(
            "image mount point is not a real directory: {}",
            mount_point.display()
        ));
    }
    let mut content: Vec<String> = std::fs::read_dir(mount_point)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    content.sort();
    if !content.is_empty() {
        return Err(format!(
            "image mount point is not empty: {} holds {}",
            mount_point.display(),
            content[..content.len().min(5)].join(", ")
        ));
    }
    let backend = backend_for(record)?;
    if backend == "kernel" {
        let mut args = vec![
            image_file.to_string_lossy().into_owned(),
            mount_point.to_string_lossy().into_owned(),
        ];
        if fresh {
            args.push("--fresh".to_string());
        }
        let outcome = run_or_refusal(
            runner,
            &crate::image_ops::CommandSpec::new(
                ctx.tools.helper_argv("mount", &args),
                std::time::Duration::from_secs(120),
            ),
        )
        .map_err(|outcome| {
            outcome
                .stderr
                .trim()
                .trim_start_matches("REFUSED: ")
                .lines()
                .next()
                .unwrap_or_default()
                .to_string()
        })?;
        let reused = outcome
            .stdout
            .lines()
            .find_map(|line| line.strip_prefix("reused "))
            .map(|dev| dev.trim().to_string());
        return Ok(reused);
    }
    let (unit, slice) = fuse_unit(image_file);
    let systemd_run = ctx
        .tools
        .systemd_run
        .clone()
        .unwrap_or_else(|| "systemd-run".to_string());
    run_or_refusal(
        runner,
        &crate::image_ops::fuse_mount_spec(&systemd_run, image_file, mount_point, &unit, &slice),
    )
    .map_err(|outcome| {
        outcome
            .stderr
            .trim()
            .trim_start_matches("REFUSED: ")
            .lines()
            .next()
            .unwrap_or_default()
            .to_string()
    })?;
    Ok(None)
}

/// `wrkslots image unmount` (#1116): unmount state then slot, and
/// refuse while a copy in another namespace still holds the image
/// (the residual labels the caller's facts report after unmount).
#[allow(clippy::too_many_arguments)]
pub(crate) fn image_unmount(
    ctx: &MountCtx<'_>,
    slot_type: &str,
    slot: &str,
    discard_stale: bool,
    state: &MountStateFn<'_>,
    held_after: &dyn Fn(&Path) -> Vec<String>,
    runner: &dyn PrivilegedRunner,
) -> HandlerOutcome {
    let record = match load_image(ctx.control, ctx.slot_path, slot_type, slot) {
        Ok(record) => record,
        Err(outcome) => return outcome,
    };
    let mut residual: Vec<String> = Vec::new();
    for (image_file, mount_point) in [
        (record.state_image(), record.state_mount()),
        (record.image_file(), record.location.clone()),
    ] {
        let (mount_state, detail, backend) = state(&image_file, &mount_point);
        if mount_state == "absent" {
            continue;
        }
        let Some(backend) = backend else {
            return refusal(
                &format!(
                    "{} has {detail}; refusing to treat {} as unmounted there",
                    mount_point.display(),
                    image_file.display()
                ),
                "image",
            );
        };
        if backend == "kernel" {
            let mut args = vec![
                image_file.to_string_lossy().into_owned(),
                mount_point.to_string_lossy().into_owned(),
            ];
            if discard_stale {
                args.push("--discard-stale-copies".to_string());
            }
            if let Err(outcome) = run_or_refusal(
                runner,
                &crate::image_ops::CommandSpec::new(
                    ctx.tools.helper_argv("umount", &args),
                    std::time::Duration::from_secs(120),
                ),
            ) {
                return outcome;
            }
            residual.extend(held_after(&image_file));
        } else {
            // FUSE: fstrim first (its result is reported, not fatal
            // in the oracle), then fusermount -u through the host.
            let _ = runner.run(&crate::image_ops::CommandSpec::new(
                vec![
                    "fstrim".to_string(),
                    mount_point.to_string_lossy().into_owned(),
                ],
                std::time::Duration::from_secs(120),
            ));
            if let Err(outcome) = run_or_refusal(
                runner,
                &crate::image_ops::CommandSpec::new(
                    vec![
                        ctx.tools.fusermount.clone(),
                        "-u".to_string(),
                        mount_point.to_string_lossy().into_owned(),
                    ],
                    std::time::Duration::from_secs(120),
                ),
            ) {
                return outcome;
            }
        }
    }
    if !residual.is_empty() {
        return refusal(
            &format!(
                "unmounted {slot_type}/{slot} on this host, but {} still holds its image (a mount in another mount namespace that is still in use). Do not copy or change the image file until that is released; the next wrkslots command remounts the slot through the same device",
                residual.join(", ")
            ),
            "image",
        );
    }
    HandlerOutcome {
        exit: 0,
        stdout: format!("unmounted {slot_type}/{slot}; the next wrkslots command remounts it\n"),
        stderr: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_ops::RunOutcome;
    use crate::image_record::{image_directory, write_record};
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct NoRunner;
    impl PrivilegedRunner for NoRunner {
        fn run(&self, _spec: &crate::image_ops::CommandSpec) -> Result<RunOutcome, String> {
            Err("status must not run commands".to_string())
        }
    }

    #[test]
    fn status_renders_python_bytes() {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("wrkslotsd-imagecmd-{}-{n}", std::process::id()));
        let control = root.join("control");
        let record = ImageRecord {
            directory: image_directory(&control, "agent", "slot-a"),
            slot: "slot-a".to_string(),
            slot_type: "agent".to_string(),
            location: root.join("slots").join("slot-a"),
            ceiling_bytes: 8 * 1_073_741_824,
            state_ceiling_bytes: 2 * 1_073_741_824,
            backend: "kernel".to_string(),
            created_at: "2026-11-07T00:00:00+00:00".to_string(),
            phase: "ready".to_string(),
        };
        std::fs::create_dir_all(&record.directory).expect("dir");
        write_record(&record).expect("record");
        std::fs::File::create(record.image_file())
            .expect("img")
            .set_len(8 * 1_073_741_824)
            .expect("size");
        std::fs::File::create(record.state_image())
            .expect("state")
            .set_len(2 * 1_073_741_824)
            .expect("size");

        let facts = |_: &Path, _: &Path| (None, None);
        let human = image_status(
            &control,
            &|_type, slot| root.join("slots").join(slot),
            "image",
            "human",
            &[],
            &facts,
            &NoRunner,
        );
        assert_eq!(human.exit, 0);
        assert_eq!(human.stderr, "");
        assert_eq!(
            human.stdout,
            format!(
                "new slots: image\nagent/slot-a at {} (ready)\n  slot  mounted=False backend=- host=0.00G used=- ceiling=8.00G inodes=-\n  state mounted=False backend=- host=0.00G used=- ceiling=2.00G inodes=-\n",
                root.join("slots").join("slot-a").display()
            )
        );

        let json = image_status(
            &control,
            &|_type, slot| root.join("slots").join(slot),
            "image",
            "json",
            &[],
            &facts,
            &NoRunner,
        );
        assert_eq!(json.exit, 0);
        let expected_json = format!(
            "{{\n  \"new_slot_representation\": \"image\",\n  \"images\": [\n    {{\n      \"name\": \"slot-a\",\n      \"slot_type\": \"agent\",\n      \"location\": \"{}\",\n      \"phase\": \"ready\",\n      \"slot\": {{\n        \"image\": \"{}\",\n        \"ceiling_bytes\": 8589934592,\n        \"host_allocated_bytes\": 0,\n        \"mounted\": false\n      }},\n      \"state\": {{\n        \"image\": \"{}\",\n        \"ceiling_bytes\": 2147483648,\n        \"host_allocated_bytes\": 0,\n        \"mounted\": false\n      }}\n    }}\n  ]\n}}\n",
            root.join("slots").join("slot-a").display(),
            record.image_file().display(),
            record.state_image().display(),
        );
        assert_eq!(json.stdout, expected_json);

        // Empty registry renders the Python empty forms.
        let empty_control = root.join("empty-control");
        std::fs::create_dir_all(&empty_control).expect("empty control");
        let empty = image_status(
            &empty_control,
            &|_type, slot| root.join("slots").join(slot),
            "worktree",
            "human",
            &[],
            &facts,
            &NoRunner,
        );
        assert_eq!(empty.stdout, "new slots: worktree\nno image-backed slots\n");
        let empty_json = image_status(
            &empty_control,
            &|_type, slot| root.join("slots").join(slot),
            "worktree",
            "json",
            &[],
            &facts,
            &NoRunner,
        );
        assert_eq!(
            empty_json.stdout,
            "{\n  \"new_slot_representation\": \"worktree\",\n  \"images\": []\n}\n"
        );

        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    const DRIVER: &str = r####""""Differential driver: run Python _cmd_image_status on a fixture.

Usage: imgstatus_driver.py ROOT CASE FORMAT
Cases: mounted | unmounted | missing | badrecord
Prints: <exit>\n<stdout bytes as-is> then a marker line + stderr.
The fixture layout mirrors the Rust test's: ROOT/project with
control at ROOT/project/worktrees and slot-images under it.
Mount activation on config load is stubbed out: the handler under
comparison takes post-load inputs, so both editions start there.
"""

import argparse
import io
import json
import os
import sys
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path

sys.path.insert(0, os.environ["WRKSLOTS_PY_ROOT"])

from wrkslots import cli, imagecmd, slotimage  # noqa: E402

root = Path(sys.argv[1])
case = sys.argv[2]
fmt = sys.argv[3]
proj = root / "project"
control = proj / "worktrees"
imgdir = control / "slot-images" / "agent" / "slot-a"
location = control / "slot-a"

# Post-load isolation: no mount activation, empty mount table.
slotimage.ensure_all_mounted = lambda *a, **k: ([], [])  # type: ignore[assignment]
cli._OWNED_REPRESENTATION_MOUNTS = frozenset()  # type: ignore[attr-defined]

if case == "mounted":
    fixed = os.statvfs_result(
        (4096, 1024, 1000003, 400009, 300007, 70001, 20011, 20003, 0, 255)
    )
    slotimage.mounted_backend = (  # type: ignore[assignment]
        lambda image_file, mount_point, table=None: "kernel"
        if str(image_file).endswith("slot.img")
        else None
    )
    os.statvfs = lambda path: fixed  # type: ignore[misc]
elif case in ("unmounted", "missing", "badrecord"):
    slotimage.mounted_backend = lambda *a, **k: None  # type: ignore[assignment]

args = argparse.Namespace(project_root=str(proj), machine="node-a", format=fmt)
out, err = io.StringIO(), io.StringIO()
try:
    with redirect_stdout(out), redirect_stderr(err):
        rc = imagecmd._cmd_image_status(args)
except cli.Refusal as exc:
    remedy = cli._refusal_remedy(
        argparse.Namespace(command="image"), exc
    )
    sys.stdout.write(f"3\n{out.getvalue()}---STDERR---\nREFUSED: {exc}\nREMEDY: {remedy}\n")
    sys.exit(0)
sys.stdout.write(f"{rc}\n{out.getvalue()}---STDERR---\n{err.getvalue()}")
"####;

    /// Differential against the Python oracle (#1095): drive
    /// `_cmd_image_status` as a subprocess on the same fixture with
    /// stubbed mount observations, and compare (exit, stdout,
    /// stderr) byte-for-byte with this handler on identical facts.
    /// Mount activation on Python config load is stubbed in the
    /// driver: both editions start from post-load inputs.
    #[test]
    fn status_matches_python_oracle() {
        use std::io::Write as _;
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "wrkslotsd-imagecmd-diff-{}-{n}",
            std::process::id()
        ));
        let proj = root.join("project");
        let control = proj.join("worktrees");
        let imgdir = control.join("slot-images").join("agent").join("slot-a");
        std::fs::create_dir_all(&imgdir).expect("imgdir");
        std::fs::create_dir_all(&proj).expect("proj");
        std::fs::write(proj.join("live.py"), "# probe\n").expect("probe");
        std::fs::write(
            proj.join(".wrkslots.yml"),
            "schema: 2\nmachine: node-a\nworktrees_dir: worktrees\ndefault_remote: origin\ndefault_landed_ref: refs/remotes/origin/main\nheartbeat_ttl_seconds: 60\nliveness_command: live.py\nslot_representation: image\n",
        )
        .expect("config");
        let location = control.join("slot-a");
        let record = ImageRecord {
            directory: imgdir.clone(),
            slot: "slot-a".to_string(),
            slot_type: "agent".to_string(),
            location: location.clone(),
            ceiling_bytes: 3 * 1_073_741_824,
            state_ceiling_bytes: 1_073_741_824,
            backend: "kernel".to_string(),
            created_at: "2026-11-07T00:00:00+00:00".to_string(),
            phase: "ready".to_string(),
        };
        write_record(&record).expect("record");
        for (file, size) in [
            (record.image_file(), 3 * 1_073_741_824_u64),
            (record.state_image(), 1_073_741_824_u64),
        ] {
            let handle = std::fs::File::create(&file).expect("image");
            handle.set_len(size).expect("size");
            // One written MiB in the slot image, as the Python fixture does.
            if file == record.image_file() {
                let mut handle = handle;
                handle.write_all(&vec![b'x'; 1024 * 1024]).expect("write");
            }
        }

        let driver = root.join("driver.py");
        std::fs::write(&driver, DRIVER).expect("driver");
        let repo = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("repo root")
            .to_path_buf();
        let slot_path = |_t: &str, slot: &str| control.join(slot);

        let run_python = |case: &str, fmt: &str| -> (i32, String, String) {
            let output = std::process::Command::new("python3")
                .arg(&driver)
                .arg(&root)
                .arg(case)
                .arg(fmt)
                .env("WRKSLOTS_PY_ROOT", repo.join("py"))
                .output()
                .expect("python oracle runs");
            let text = String::from_utf8(output.stdout).expect("utf8");
            let (head, rest) = text.split_once('\n').expect("exit line");
            let (stdout, stderr) = rest.split_once("---STDERR---\n").expect("marker");
            (
                head.trim().parse().expect("exit"),
                stdout.to_string(),
                stderr.to_string(),
            )
        };

        // The fixture each case needs on disk.
        let set_case = |case: &str| {
            // Restore both images, then apply the case's damage.
            for (file, size) in [
                (record.image_file(), 3 * 1_073_741_824_u64),
                (record.state_image(), 1_073_741_824_u64),
            ] {
                if !file.exists() {
                    let handle = std::fs::File::create(&file).expect("restore");
                    handle.set_len(size).expect("size");
                }
            }
            let record_path = imgdir.join("IMAGE.json");
            write_record(&record).expect("restore record");
            match case {
                "missing" => std::fs::remove_file(record.state_image()).expect("unlink state"),
                "badrecord" => {
                    std::fs::write(&record_path, "{\"schema\": \"other/v9\"}\n").expect("bad")
                }
                _ => {}
            }
        };

        for (case, fmt) in [
            ("mounted", "human"),
            ("mounted", "json"),
            ("unmounted", "human"),
            ("unmounted", "json"),
            ("missing", "json"),
            ("badrecord", "human"),
        ] {
            set_case(case);
            let (exit, stdout, stderr) = run_python(case, fmt);
            let facts = |image_file: &Path, _: &Path| -> (Option<String>, Option<VfsFacts>) {
                if case == "mounted" && image_file == record.image_file() {
                    (
                        Some("kernel".to_string()),
                        Some(VfsFacts {
                            used_bytes: (1_000_003 - 400_009) * 1024,
                            free_bytes: 300_007 * 1024,
                            used_inodes: 70_001 - 20_011,
                        }),
                    )
                } else {
                    (None, None)
                }
            };
            let outcome = image_status(&control, &slot_path, "image", fmt, &[], &facts, &NoRunner);
            assert_eq!(
                (
                    outcome.exit,
                    outcome.stdout.as_str(),
                    outcome.stderr.as_str()
                ),
                (exit, stdout.as_str(), stderr.as_str()),
                "case {case} format {fmt}"
            );
        }
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn status_with_worktrees_field() {
        let wt = serde_json::json!({
            "image_file": "/proj/wt.img",
            "mount_point": "/proj/.slots-image",
            "mount_at": "binds",
            "configured_size_bytes": 2147483648i64,
            "state": "missing",
            "image_exists": false,
            "quota_mode": "unknown",
            "configured_quota_mode": "simple",
            "binds": {}
        });
        let root = std::env::temp_dir().join(format!("wrkslotsd-wt-field-{}", std::process::id()));
        let control = root.join("control");
        std::fs::create_dir_all(&control).expect("control");
        let facts = |_: &Path, _: &Path| (None, None);
        let human = image_status_wt(
            &control,
            &|_t, slot| root.join(slot),
            "worktree",
            "human",
            &[],
            &facts,
            &NoRunner,
            Some(&wt),
        );
        assert!(
            human
                .stdout
                .contains("worktrees image: missing at /proj/.slots-image (mount_at=binds)\n"),
            "{}",
            human.stdout
        );
        let json = image_status_wt(
            &control,
            &|_t, slot| root.join(slot),
            "worktree",
            "json",
            &[],
            &facts,
            &NoRunner,
            Some(&wt),
        );
        assert!(
            json.stdout.contains("\"worktrees_image\": {"),
            "{}",
            json.stdout
        );
        assert!(
            json.stdout.contains("\"mount_at\": \"binds\""),
            "{}",
            json.stdout
        );
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn set_default_trim_grow_handlers() {
        use crate::image_ops::{CommandSpec, ToolPaths};
        use serde_json::json;
        use std::cell::RefCell;

        struct Fake {
            calls: RefCell<Vec<Vec<String>>>,
        }
        impl PrivilegedRunner for Fake {
            fn run(&self, spec: &CommandSpec) -> Result<RunOutcome, String> {
                self.calls.borrow_mut().push(spec.argv.clone());
                Ok(RunOutcome {
                    returncode: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                })
            }
        }
        let tools = ToolPaths {
            sudo: "sudo".to_string(),
            python: "/usr/bin/python3".to_string(),
            helper: PathBuf::from("/opt/imagehelper.py"),
            systemd_run: None,
            fusermount: "fusermount".to_string(),
        };

        // set-default: worktree removes the key, image sets it.
        let (outcome, updated) = image_set_default(
            "worktree",
            &json!({"slot_representation": "image", "machine": "n"}),
        );
        assert_eq!(outcome.exit, 0);
        assert!(outcome
            .stdout
            .starts_with("new slots will use the worktree representation;"));
        assert!(updated
            .expect("updated")
            .get("slot_representation")
            .is_none());
        let (_o, updated) = image_set_default("image", &json!({"machine": "n"}));
        assert_eq!(updated.expect("updated")["slot_representation"], "image");

        // Fixture image for trim/grow.
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("wrkslotsd-mount-family-{}-{n}", std::process::id()));
        let control = root.join("control");
        let record = ImageRecord {
            directory: image_directory(&control, "agent", "slot-a"),
            slot: "slot-a".to_string(),
            slot_type: "agent".to_string(),
            location: root.join("slots").join("slot-a"),
            ceiling_bytes: 2 * 1_073_741_824,
            state_ceiling_bytes: 1_073_741_824,
            backend: "kernel".to_string(),
            created_at: "2026-11-07T00:00:00+00:00".to_string(),
            phase: "ready".to_string(),
        };
        std::fs::create_dir_all(&record.directory).expect("dir");
        write_record(&record).expect("record");
        std::fs::File::create(record.image_file())
            .expect("img")
            .set_len(2 * 1_073_741_824)
            .expect("size");
        std::fs::File::create(record.state_image())
            .expect("state")
            .set_len(1_073_741_824)
            .expect("size");
        let slot_path = |_t: &str, slot: &str| root.join("slots").join(slot);
        let no_facts = |_: &Path, _: &Path| (None, None);

        // trim with nothing mounted prints nothing and runs nothing.
        let fake = Fake {
            calls: RefCell::new(Vec::new()),
        };
        let ctx = MountCtx {
            control: &control,
            slot_path: &slot_path,
            tools: &tools,
        };
        let outcome = image_trim(&ctx, "agent", "slot-a", &no_facts, &fake);
        assert_eq!(
            outcome,
            HandlerOutcome {
                exit: 0,
                stdout: String::new(),
                stderr: String::new()
            }
        );
        // not image-backed refuses.
        let outcome = image_trim(&ctx, "agent", "nope", &no_facts, &fake);
        assert_eq!(outcome.exit, 3);
        assert!(
            outcome.stderr.contains("is not image-backed"),
            "{}",
            outcome.stderr
        );

        // grow: shrink refuses; grow online runs helper and rewrites the record.
        let outcome = image_grow(&ctx, "agent", "slot-a", 1, &no_facts, &fake);
        assert_eq!(outcome.exit, 3);
        let fake = Fake {
            calls: RefCell::new(Vec::new()),
        };
        let outcome = image_grow(&ctx, "agent", "slot-a", 4, &no_facts, &fake);
        assert_eq!(outcome.exit, 0, "{}", outcome.stderr);
        assert_eq!(outcome.stdout, "agent/slot-a ceiling is now 4 GiB\n");
        assert_eq!(
            std::fs::metadata(record.image_file()).expect("meta").len(),
            4 * 1_073_741_824
        );
        let loaded =
            crate::image_record::load_slot_image(&control, "agent", "slot-a", &record.location)
                .expect("load")
                .expect("some");
        assert_eq!(loaded.ceiling_bytes, 4 * 1_073_741_824);
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn mount_unmount_handlers() {
        use crate::image_ops::{CommandSpec, ToolPaths};
        use std::cell::RefCell;

        struct Fake {
            calls: RefCell<Vec<Vec<String>>>,
        }
        impl PrivilegedRunner for Fake {
            fn run(&self, spec: &CommandSpec) -> Result<RunOutcome, String> {
                self.calls.borrow_mut().push(spec.argv.clone());
                Ok(RunOutcome {
                    returncode: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                })
            }
        }
        let tools = ToolPaths {
            sudo: "sudo".to_string(),
            python: "/usr/bin/python3".to_string(),
            helper: PathBuf::from("/opt/imagehelper.py"),
            systemd_run: Some("/usr/bin/systemd-run".to_string()),
            fusermount: "fusermount".to_string(),
        };
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "wrkslotsd-mount-unmount-{}-{n}",
            std::process::id()
        ));
        let control = root.join("control");
        let record = ImageRecord {
            directory: image_directory(&control, "agent", "slot-a"),
            slot: "slot-a".to_string(),
            slot_type: "agent".to_string(),
            location: root.join("slots").join("slot-a"),
            ceiling_bytes: 2 * 1_073_741_824,
            state_ceiling_bytes: 1_073_741_824,
            backend: "kernel".to_string(),
            created_at: "2026-11-07T00:00:00+00:00".to_string(),
            phase: "ready".to_string(),
        };
        std::fs::create_dir_all(&record.directory).expect("dir");
        std::fs::create_dir_all(&record.location).expect("mount point");
        std::fs::create_dir_all(record.state_mount()).expect("state point");
        write_record(&record).expect("record");
        std::fs::File::create(record.image_file())
            .expect("img")
            .set_len(2 * 1_073_741_824)
            .expect("size");
        std::fs::File::create(record.state_image())
            .expect("state")
            .set_len(1_073_741_824)
            .expect("size");
        let slot_path = |_t: &str, slot: &str| root.join("slots").join(slot);
        let ctx = MountCtx {
            control: &control,
            slot_path: &slot_path,
            tools: &tools,
        };
        let absent = |_: &Path, _: &Path| {
            (
                "absent".to_string(),
                "nothing is mounted there".to_string(),
                None,
            )
        };
        let no_held = |_: &Path| Vec::new();
        let kernel = |_: &ImageRecord| Ok("kernel".to_string());
        let fuse_unit = |_: &Path| {
            (
                "wrkslots-fuse-x-1-2".to_string(),
                "wrkslots-images.slice".to_string(),
            )
        };

        // mount: both images unmounted -> two helper mount actions.
        let fake = Fake {
            calls: RefCell::new(Vec::new()),
        };
        let outcome = image_mount(&ctx, false, &absent, &no_held, &kernel, &fuse_unit, &fake);
        assert_eq!(outcome.exit, 0, "{}", outcome.stderr);
        assert!(
            outcome.stdout.contains("mounted slot.img at"),
            "{}",
            outcome.stdout
        );
        assert_eq!(fake.calls.borrow().len(), 2);

        // unmount: kernel backend -> helper umount, then the success line.
        let mounted = |_: &Path, _: &Path| {
            (
                "kernel".to_string(),
                "ext4 /dev/loop3 (7:3, root /)".to_string(),
                Some("kernel".to_string()),
            )
        };
        let fake = Fake {
            calls: RefCell::new(Vec::new()),
        };
        let outcome = image_unmount(&ctx, "agent", "slot-a", false, &mounted, &no_held, &fake);
        assert_eq!(outcome.exit, 0, "{}", outcome.stderr);
        assert_eq!(
            outcome.stdout,
            "unmounted agent/slot-a; the next wrkslots command remounts it\n"
        );
        // residual holder refuses.
        let held = |_: &Path| vec!["loop3 (7:3)".to_string()];
        let outcome = image_unmount(&ctx, "agent", "slot-a", false, &mounted, &held, &fake);
        assert_eq!(outcome.exit, 3);
        assert!(
            outcome.stderr.contains("still holds its image"),
            "{}",
            outcome.stderr
        );
        std::fs::remove_dir_all(&root).expect("cleanup");
    }
}
