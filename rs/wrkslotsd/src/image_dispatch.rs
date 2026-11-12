//! Dispatch glue for the `image` family (#1095/#1116): config
//! loading, live mount-table facts, and the production privileged
//! runner, feeding the dispatch-shaped handlers in `imagecmd`.
//! The handlers stay pure; every host read lives here.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::image_mounts::{parse_mountinfo, MountEntry};
use crate::image_ops::{CommandSpec, PrivilegedRunner, RunOutcome};
use crate::image_status::VfsFacts;
use crate::imagecmd::HandlerOutcome;
use crate::worktrees_image::{
    classify_mount, settings_from_obj, status_report_with_quota, WorktreesImageSettings, WtCtx,
    MOUNTED,
};

type Result<T> = std::result::Result<T, String>;

/// Runs one `CommandSpec` as a subprocess, exactly its argv (no
/// shell), honouring its timeout; the production side of the
/// `PrivilegedRunner` seam.
pub(crate) struct SubprocessRunner;

impl PrivilegedRunner for SubprocessRunner {
    fn run(&self, spec: &CommandSpec) -> Result<RunOutcome> {
        let Some((program, args)) = spec.argv.split_first() else {
            return Err("cannot run an empty command".to_string());
        };
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("cannot run {program}: {error}"))?;
        let deadline = Instant::now() + spec.timeout;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => {
                    let output = child
                        .wait_with_output()
                        .map_err(|error| format!("cannot collect {program}: {error}"))?;
                    return Ok(RunOutcome {
                        returncode: output.status.code().unwrap_or(1),
                        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                    });
                }
                Ok(None) => {
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(format!(
                            "{} exceeded its {}-second bound",
                            spec.argv.join(" "),
                            spec.timeout.as_secs()
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => {
                    return Err(format!("cannot wait for {program}: {error}"));
                }
            }
        }
    }
}

fn mount_entries() -> Vec<MountEntry> {
    std::fs::read_to_string("/proc/self/mountinfo")
        .map(|text| parse_mountinfo(&text))
        .unwrap_or_default()
}

fn vfs_facts(mount_point: &Path) -> Option<VfsFacts> {
    let stat = rustix::fs::statvfs(mount_point).ok()?;
    let frsize = stat.f_frsize;
    Some(VfsFacts {
        used_bytes: (stat.f_blocks.saturating_sub(stat.f_bfree)).saturating_mul(frsize),
        free_bytes: stat.f_bavail.saturating_mul(frsize),
        used_inodes: stat.f_files.saturating_sub(stat.f_ffree),
    })
}

/// Where validation slots surface, mirroring Python
/// `validate_external_dir`: beside a slots directory named
/// `slots`, else inside it.
fn validate_external_dir(worktrees: &Path) -> PathBuf {
    let managed = if worktrees.file_name().is_some_and(|name| name == "slots") {
        worktrees.parent().unwrap_or(worktrees)
    } else {
        worktrees
    };
    managed.join("validate")
}

fn quotas_enabled(runner: &dyn PrivilegedRunner, mount_point: &Path) -> bool {
    let spec = CommandSpec::new(
        vec![
            "sudo".to_string(),
            "-n".to_string(),
            "btrfs".to_string(),
            "qgroup".to_string(),
            "show".to_string(),
            mount_point.to_string_lossy().into_owned(),
        ],
        Duration::from_secs(30),
    );
    matches!(runner.run(&spec), Ok(outcome) if outcome.returncode == 0)
}

fn simple_quota_flag(runner: &dyn PrivilegedRunner, image_file: &Path) -> bool {
    let spec = CommandSpec::new(
        vec![
            "btrfs".to_string(),
            "inspect-internal".to_string(),
            "dump-super".to_string(),
            image_file.to_string_lossy().into_owned(),
        ],
        Duration::from_secs(30),
    );
    matches!(runner.run(&spec), Ok(outcome) if outcome.returncode == 0 && outcome.stdout.contains("SIMPLE_QUOTA"))
}

/// Python `quota_mode_at`: `none`, `simple`, or `full`.
fn quota_mode_at(
    runner: &dyn PrivilegedRunner,
    mount_point: &Path,
    image_file: &Path,
) -> &'static str {
    if !quotas_enabled(runner, mount_point) {
        return "none";
    }
    if simple_quota_flag(runner, image_file) {
        "simple"
    } else {
        "full"
    }
}

fn dir_facts(path: &Path) -> (bool, bool) {
    let exists = path.is_dir();
    let empty = if exists {
        std::fs::read_dir(path)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(false)
    } else {
        true
    };
    (exists, empty)
}

fn config_path(config: &Value, key: &str) -> PathBuf {
    PathBuf::from(config.get(key).and_then(Value::as_str).unwrap_or_default())
}

/// The conditional worktrees-image report for `image status`
/// (#1279), with the validate surface fields Python adds.
fn wt_report(
    config: &Value,
    entries: &[MountEntry],
    runner: &dyn PrivilegedRunner,
) -> Result<Option<Value>> {
    let Some(section) = config
        .get("worktrees_image")
        .filter(|value| !value.is_null())
    else {
        return Ok(None);
    };
    let settings: WorktreesImageSettings = settings_from_obj(section)?;
    let root = config_path(config, "root");
    let control = config_path(config, "control");
    let worktrees = config_path(config, "worktrees");
    let skipped = config
        .get("worktrees_image_skipped")
        .and_then(Value::as_str);
    let ctx = WtCtx {
        root: &root,
        control: &control,
        worktrees: &worktrees,
        entries,
        skipped,
    };
    // The same mount directory `status_report_with_quota` reports.
    let mount_dir = if settings.direct() {
        worktrees.clone()
    } else {
        worktrees
            .parent()
            .map(|parent| parent.join(crate::worktrees_image::IMAGE_MOUNT_NAME))
            .unwrap_or_default()
    };
    let (exists, empty) = dir_facts(&mount_dir);
    let quota = if classify_mount(&mount_dir, entries, exists, empty) == MOUNTED {
        Some(quota_mode_at(
            runner,
            &mount_dir,
            &settings.image_file(&root),
        ))
    } else {
        None
    };
    let mut report = status_report_with_quota(&ctx, &settings, quota);
    if let Some(object) = report.as_object_mut() {
        let external = validate_external_dir(&worktrees);
        let mounted = entries.iter().any(|entry| {
            crate::image_mounts::normalize_absolute(&entry.mount_point)
                == crate::image_mounts::normalize_absolute(&external)
        });
        object.insert(
            "validate_dir".to_string(),
            json!(external.to_string_lossy()),
        );
        object.insert(
            "validate_bind".to_string(),
            json!(if settings.direct() && mounted {
                "mounted"
            } else if settings.direct() {
                "not-mounted"
            } else {
                "n/a"
            }),
        );
    }
    Ok(Some(report))
}

/// The `env` key order inside the top-level `sandbox` section of
/// a configuration document, for documents in block style. The
/// strict YAML subset nests by indentation; only the first mapping
/// level under `env:` is collected. Flow style yields no order and
/// the handler's sorted order stands.
fn sandbox_env_order(text: &str) -> Vec<String> {
    let mut order: Vec<String> = Vec::new();
    let mut sandbox_indent: Option<usize> = None;
    let mut env_indent: Option<usize> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let key = trimmed
            .split_once(':')
            .map(|(name, _)| name.trim().trim_matches(['"', '\'']))
            .unwrap_or_default();
        match sandbox_indent {
            None => {
                if indent == 0 && key == "sandbox" {
                    sandbox_indent = Some(indent);
                }
            }
            Some(base) => {
                if indent <= base && key != "sandbox" {
                    break;
                }
                match env_indent {
                    None => {
                        if key == "env" && indent > base {
                            if let Some(flow) = trimmed
                                .split_once(':')
                                .map(|(_, rest)| rest.trim())
                                .filter(|rest| rest.starts_with('{'))
                            {
                                for part in flow.trim_matches(['{', '}']).split(',') {
                                    if let Some((name, _)) = part.split_once(':') {
                                        let name =
                                            name.trim().trim_matches(['"', '\'']).to_string();
                                        if !name.is_empty() {
                                            order.push(name);
                                        }
                                    }
                                }
                                return order;
                            }
                            env_indent = Some(indent);
                        }
                    }
                    Some(env_base) => {
                        if indent <= env_base {
                            break;
                        }
                        if !key.is_empty() {
                            order.push(key.to_string());
                        }
                    }
                }
            }
        }
    }
    order
}

/// `wrkslots sandbox show-config`: validate through the config
/// loader, then hand the raw `sandbox` section to the handler
/// with the document's own env order.
pub(crate) fn sandbox_show_config_cli(
    root: Option<&Path>,
    machine: Option<&str>,
) -> Result<(i32, String, String)> {
    let config = crate::config_validate::load_config(root, machine)?;
    let raw = crate::config_validate::raw_section(root, "sandbox")?;
    let text = std::fs::read_to_string(PathBuf::from(
        config
            .get("config_path")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    ))
    .unwrap_or_default();
    Ok(crate::sandbox_state::sandbox_show_config_ordered(
        raw.as_ref(),
        &sandbox_env_order(&text),
    ))
}

/// `wrkslots image status`: load the config, derive live mount
/// facts, and run the pure handler. Config errors use the native
/// error form, like the other reader commands.
pub(crate) fn image_status_cli(
    root: Option<&Path>,
    machine: Option<&str>,
    format: &str,
) -> Result<HandlerOutcome> {
    let config = crate::config_validate::load_config(root, machine)?;
    let control = config_path(&config, "control");
    let worktrees = config_path(&config, "worktrees");
    let representation = config
        .get("slot_representation")
        .and_then(Value::as_str)
        .unwrap_or("worktree")
        .to_string();
    let entries = mount_entries();
    let runner = SubprocessRunner;
    let report = wt_report(&config, &entries, &runner)?;
    let facts = |image_file: &Path, mount_point: &Path| {
        let (state, _detail) = crate::storage_state::mount_state(image_file, mount_point);
        let backend = crate::image_mounts::mounted_backend(&state).map(str::to_string);
        let vfs = if backend.is_some() {
            vfs_facts(mount_point)
        } else {
            None
        };
        (backend, vfs)
    };
    Ok(crate::imagecmd::image_status_wt(
        &control,
        &|_slot_type: &str, slot: &str| worktrees.join(slot),
        &representation,
        format,
        &entries,
        &facts,
        &runner,
        report.as_ref(),
    ))
}
