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

/// `shutil.which` semantics for the fixed tool names the image
/// plans name (sudo, systemd-run, fusermount), mirroring Python.
fn which(tool: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    use std::os::unix::fs::PermissionsExt as _;
    std::env::split_paths(&path).find_map(|dir| {
        let candidate = dir.join(tool);
        let executable = std::fs::metadata(&candidate)
            .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false);
        executable.then(|| candidate.to_string_lossy().into_owned())
    })
}

fn check_helper(helper: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt as _;
    let meta = std::fs::metadata(helper).map_err(|error| {
        format!(
            "image helper {} is not readable: {error}; it ships with the Python wrkslots package",
            helper.display()
        )
    })?;
    if !meta.is_file() {
        return Err(format!(
            "image helper {} is not a regular file; refusing to run it privileged",
            helper.display()
        ));
    }
    if meta.uid() != rustix::process::getuid().as_raw() {
        return Err(format!(
            "image helper {} is not owned by this user; refusing to run it privileged",
            helper.display()
        ));
    }
    if meta.mode() & 0o022 != 0 {
        return Err(format!(
            "image helper {} is group- or world-writable; refusing to run it privileged",
            helper.display()
        ));
    }
    Ok(())
}

/// The production tool paths for the privileged image plans, per
/// the resolution recorded in #1116: the interpreter is the one
/// the launcher exports (`WRKSLOTS_PYTHON_BIN`, absolute), and
/// `imagehelper.py` resolves through that interpreter's installed
/// wrkslots package after an ownership/writability check. The
/// `WRKSLOTS_IMAGEHELPER` override (absolute, tests/operators)
/// skips the package lookup but not the check.
pub(crate) fn tool_paths() -> Result<crate::image_ops::ToolPaths> {
    let python = match std::env::var_os("WRKSLOTS_PYTHON_BIN") {
        Some(value) if Path::new(&value).is_absolute() => {
            PathBuf::from(value).to_string_lossy().into_owned()
        }
        _ => {
            return Err(
                "WRKSLOTS_PYTHON_BIN is not set to an absolute interpreter path; the editions launcher exports it, and the privileged image helper runs through exactly that interpreter"
                    .to_string(),
            )
        }
    };
    let helper = match std::env::var_os("WRKSLOTS_IMAGEHELPER") {
        Some(value) if Path::new(&value).is_absolute() => PathBuf::from(value),
        Some(_) => return Err("WRKSLOTS_IMAGEHELPER must be an absolute path when set".to_string()),
        None => {
            let spec = CommandSpec::new(
                vec![
                    python.clone(),
                    "-I".to_string(),
                    "-c".to_string(),
                    "import importlib.util,sys; s=importlib.util.find_spec(\"wrkslots.imagehelper\"); sys.stdout.write(s.origin or \"\")".to_string(),
                ],
                Duration::from_secs(30),
            );
            let outcome = SubprocessRunner.run(&spec)?;
            let origin = outcome.stdout.trim();
            if outcome.returncode != 0 || origin.is_empty() {
                return Err(format!(
                    "cannot locate imagehelper.py through {python}'s wrkslots package: {}",
                    outcome.stderr.trim()
                ));
            }
            PathBuf::from(origin)
        }
    };
    check_helper(&helper)?;
    Ok(crate::image_ops::ToolPaths {
        sudo: which("sudo").unwrap_or_else(|| "sudo".to_string()),
        python,
        helper,
        systemd_run: which("systemd-run"),
        fusermount: which("fusermount").unwrap_or_else(|| "fusermount".to_string()),
    })
}

fn mount_entries() -> Vec<MountEntry> {
    std::fs::read_to_string("/proc/self/mountinfo")
        .map(|text| parse_mountinfo(&text))
        .unwrap_or_default()
}

/// Backend and statvfs facts for one image file and mount point,
/// from the live mount table, as the handlers inject them.
fn mount_fact(image_file: &Path, mount_point: &Path) -> (Option<String>, Option<VfsFacts>) {
    let (state, _detail) = crate::storage_state::mount_state(image_file, mount_point);
    let backend = crate::image_mounts::mounted_backend(&state).map(str::to_string);
    let vfs = if backend.is_some() {
        vfs_facts(mount_point)
    } else {
        None
    };
    (backend, vfs)
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

/// `wrkslots limits show` (read-only; `apply`/`clear` are
/// writers and stay Python in this phase): the caller's uid, the
/// host's physical memory from /proc/meminfo, and its CPU count.
pub(crate) fn limits_show_cli(memory_fraction: f64, cpu_fraction: f64) -> (i32, String, String) {
    let physical_bytes = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|text| {
            text.lines().find_map(|line| {
                line.strip_prefix("MemTotal:")
                    .and_then(|rest| rest.trim().strip_suffix("kB"))
                    .and_then(|kib| kib.trim().parse::<u64>().ok())
            })
        })
        .map(|kib| kib.saturating_mul(1024))
        .unwrap_or(0);
    let cpu_count = std::thread::available_parallelism()
        .map(|count| count.get() as u64)
        .unwrap_or(1);
    crate::sandbox_exec::limits(
        &SubprocessRunner,
        rustix::process::getuid().as_raw(),
        "show",
        memory_fraction,
        cpu_fraction,
        physical_bytes,
        cpu_count,
    )
}

/// Shared mount-family inputs: validated paths plus tool paths.
fn mount_ctx(
    root: Option<&Path>,
    machine: Option<&str>,
) -> Result<(PathBuf, PathBuf, crate::image_ops::ToolPaths)> {
    let config = crate::config_validate::load_config(root, machine)?;
    Ok((
        config_path(&config, "control"),
        config_path(&config, "worktrees"),
        tool_paths()?,
    ))
}

/// `wrkslots image trim`: no registry mutation; the plan runs
/// through the production runner only for mounted images.
pub(crate) fn image_trim_cli(
    root: Option<&Path>,
    machine: Option<&str>,
    slot_type: &str,
    slot: &str,
) -> Result<HandlerOutcome> {
    let (control, worktrees, tools) = mount_ctx(root, machine)?;
    let slot_path = |_: &str, slot: &str| worktrees.join(slot);
    let ctx = crate::imagecmd::MountCtx {
        control: &control,
        slot_path: &slot_path,
        tools: &tools,
    };
    Ok(crate::imagecmd::image_trim(
        &ctx,
        slot_type,
        slot,
        &mount_fact,
        &SubprocessRunner,
    ))
}

/// Whether the root launcher's spec directory is a mount point
/// here (true only in a box), mirroring Python
/// `_spec_directory_is_mounted`.
fn spec_directory_is_mounted(entries: &[MountEntry]) -> bool {
    let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") else {
        return false;
    };
    let directory = PathBuf::from(runtime).join("wrkslots");
    if !directory.is_dir() {
        return false;
    }
    entries.iter().any(|entry| {
        crate::image_mounts::normalize_absolute(&entry.mount_point)
            == crate::image_mounts::normalize_absolute(&directory)
    })
}

/// Python `box_kind`: which wrkslots box this process runs in.
fn box_kind(entries: &[MountEntry]) -> crate::image_ops::BoxKind {
    use crate::image_ops::BoxKind;
    match std::env::var("WRKSLOTS_SANDBOX").ok().as_deref() {
        Some("cgroup") => BoxKind::None,
        Some("userns" | "root") => {
            if std::env::var_os("WRKSLOTS_BOX").is_some() {
                BoxKind::Coordinator
            } else {
                BoxKind::Slot
            }
        }
        _ if spec_directory_is_mounted(entries) => {
            if std::env::var_os("WRKSLOTS_BOX").is_some() {
                BoxKind::Coordinator
            } else {
                BoxKind::Slot
            }
        }
        _ => BoxKind::None,
    }
}

/// Whether `sudo -n true` works, mirroring Python `sudo_available`
/// (host-routed from a coordinator box).
fn sudo_available(tools: &crate::image_ops::ToolPaths, kind: crate::image_ops::BoxKind) -> bool {
    let argv = vec![tools.sudo.clone(), "-n".to_string(), "true".to_string()];
    let Ok(argv) = crate::image_ops::host_argv(tools, &argv, kind, true) else {
        return false;
    };
    matches!(
        SubprocessRunner.run(&CommandSpec::new(argv, Duration::from_secs(30))),
        Ok(outcome) if outcome.returncode == 0
    )
}

fn fuse_available() -> bool {
    if which("fuse2fs").is_none() {
        return false;
    }
    use std::os::unix::fs::PermissionsExt as _;
    let Ok(meta) = std::fs::metadata("/dev/fuse") else {
        return false;
    };
    let mode = meta.permissions().mode();
    let uid = rustix::process::getuid().as_raw();
    use std::os::unix::fs::MetadataExt as _;
    let (read, write) = if meta.uid() == uid {
        (mode & 0o400 != 0, mode & 0o200 != 0)
    } else if meta.gid() == rustix::process::getgid().as_raw() {
        (mode & 0o040 != 0, mode & 0o020 != 0)
    } else {
        (mode & 0o004 != 0, mode & 0o002 != 0)
    };
    read && write
}

/// Python `resolve_backend` for one record's preference.
fn resolve_backend(
    preference: &str,
    tools: &crate::image_ops::ToolPaths,
    kind: crate::image_ops::BoxKind,
) -> Result<String> {
    use crate::image_ops::BoxKind;
    if kind == BoxKind::Slot {
        return Err(crate::image_ops::SLOT_BOX_REFUSAL.to_string());
    }
    let preference = std::env::var("WRKSLOTS_IMAGE_BACKEND")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| preference.to_string());
    if !matches!(preference.as_str(), "auto" | "kernel" | "fuse") {
        return Err(format!(
            "image backend must be one of auto, kernel, fuse, not '{preference}'"
        ));
    }
    if matches!(preference.as_str(), "auto" | "kernel") && sudo_available(tools, kind) {
        return Ok("kernel".to_string());
    }
    if preference == "kernel" {
        return Err(
            "kernel image backend needs passwordless sudo (sudo -n true failed)".to_string(),
        );
    }
    if fuse_available() {
        return Ok("fuse".to_string());
    }
    Err(
        "no image backend is usable: sudo -n is unavailable and fuse2fs or /dev/fuse is missing"
            .to_string(),
    )
}

/// The FUSE unit and slice for one image file, mirroring Python
/// `_fuse_mount`'s naming.
fn fuse_unit(image_file: &Path) -> (String, String) {
    use sha2::Digest as _;
    let absolute = if image_file.is_absolute() {
        image_file.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(image_file))
            .unwrap_or_else(|_| image_file.to_path_buf())
    };
    let digest = sha2::Sha256::digest(absolute.to_string_lossy().as_bytes());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    let cgroup = std::fs::read_to_string("/proc/self/cgroup").ok();
    let root = crate::sandbox_state::root_slice(None, cgroup.as_deref());
    (
        format!("wrkslots-fuse-{}", &hex[..12]),
        format!("{}-images.slice", root.trim_end_matches(".slice")),
    )
}

/// (state, detail, backend) for the mount handlers.
fn mount_state3(image_file: &Path, mount_point: &Path) -> (String, String, Option<String>) {
    let (state, detail) = crate::storage_state::mount_state(image_file, mount_point);
    let backend = crate::image_mounts::mounted_backend(&state).map(str::to_string);
    (state, detail, backend)
}

/// `wrkslots image mount`: fixture-gated writer holding the
/// mutation locks. (Stray Git-pointer clearing stays Python for
/// now; see #1116.)
pub(crate) fn image_mount_cli(
    root: Option<&Path>,
    machine: Option<&str>,
    coordinator_authorized: bool,
) -> Result<HandlerOutcome> {
    crate::ensure_states::require_fixture()?;
    let (control, worktrees, tools) = mount_ctx(root, machine)?;
    let (cfg, selected) = crate::active_state::load_cfg(root, machine)?;
    let _locks = crate::hold_write::acquire_mutation_locks(root, machine, &cfg, &selected)?;
    let entries = mount_entries();
    let kind = box_kind(&entries);
    let slot_path = |_: &str, slot: &str| worktrees.join(slot);
    let ctx = crate::imagecmd::MountCtx {
        control: &control,
        slot_path: &slot_path,
        tools: &tools,
    };
    let backend_for =
        |record: &crate::image_record::ImageRecord| resolve_backend(&record.backend, &tools, kind);
    Ok(crate::imagecmd::image_mount(
        &ctx,
        coordinator_authorized,
        &mount_state3,
        &crate::storage_state::still_attached_labels,
        &backend_for,
        &fuse_unit,
        &SubprocessRunner,
    ))
}

/// `wrkslots image unmount`: fixture-gated writer; prove the slot
/// unused under the mutation locks before unmounting.
pub(crate) fn image_unmount_cli(
    root: Option<&Path>,
    machine: Option<&str>,
    slot_type: &str,
    slot: &str,
) -> Result<HandlerOutcome> {
    crate::ensure_states::require_fixture()?;
    let (control, worktrees, tools) = mount_ctx(root, machine)?;
    let (cfg, selected) = crate::active_state::load_cfg(root, machine)?;
    let _locks = crate::hold_write::acquire_mutation_locks(root, machine, &cfg, &selected)?;
    // Python loads the image first (a non-image slot refuses in
    // the handler) and asserts use of its location, which may not
    // exist as a directory for a slot that was never mounted.
    if worktrees.join(slot).is_dir() {
        crate::remove_write::assert_slot_unused(
            &worktrees.join(slot),
            i64::from(std::process::id()),
        )?;
    }
    let slot_path = |_: &str, slot: &str| worktrees.join(slot);
    let ctx = crate::imagecmd::MountCtx {
        control: &control,
        slot_path: &slot_path,
        tools: &tools,
    };
    Ok(crate::imagecmd::image_unmount(
        &ctx,
        slot_type,
        slot,
        false,
        &mount_state3,
        &crate::storage_state::still_attached_labels,
        &SubprocessRunner,
    ))
}

/// `wrkslots image grow`: fixture-gated writer holding the
/// mutation locks. (Python mounts an unmounted kernel image
/// before resizing; that ensure step is not wired yet, see #1116.)
pub(crate) fn image_grow_cli(
    root: Option<&Path>,
    machine: Option<&str>,
    slot_type: &str,
    slot: &str,
    ceiling_gib: i64,
) -> Result<HandlerOutcome> {
    crate::ensure_states::require_fixture()?;
    let (control, worktrees, tools) = mount_ctx(root, machine)?;
    let (cfg, selected) = crate::active_state::load_cfg(root, machine)?;
    let _locks = crate::hold_write::acquire_mutation_locks(root, machine, &cfg, &selected)?;
    let slot_path = |_: &str, slot: &str| worktrees.join(slot);
    let ctx = crate::imagecmd::MountCtx {
        control: &control,
        slot_path: &slot_path,
        tools: &tools,
    };
    Ok(crate::imagecmd::image_grow(
        &ctx,
        slot_type,
        slot,
        ceiling_gib,
        &mount_fact,
        &SubprocessRunner,
    ))
}

/// `wrkslots image set-default`: fixture-gated writer. Validate,
/// then under the config lock hand the raw document to the pure
/// handler and write its update in the file's own format.
pub(crate) fn image_set_default_cli(
    root: Option<&Path>,
    machine: Option<&str>,
    representation: &str,
) -> Result<HandlerOutcome> {
    crate::ensure_states::require_fixture()?;
    crate::config_validate::load_config(root, machine)?;
    let (cfg, _selected) = crate::active_state::load_cfg(root, machine)?;
    let config_path = cfg.root.join(".wrkslots.yml");
    let _lock = crate::hold_write::acquire_config_lock(&cfg)?;
    let text = std::fs::read_to_string(&config_path).map_err(|error| {
        format!(
            "cannot read configuration {}: {error}",
            config_path.display()
        )
    })?;
    let (raw, format) = crate::yamlconfig::load_document(&text)
        .map_err(|error| format!("configuration is malformed: {error}"))?;
    let (outcome, updated) = crate::imagecmd::image_set_default(representation, &raw);
    if let Some(updated) = updated {
        crate::config_convert::write_config(&config_path, &updated, format)?;
    }
    Ok(outcome)
}

/// One checkout's Git worktree lock state, mirroring Python
/// `worktree_lock_state`: `(locked, reason)`, or None when the
/// checkout is not a registered worktree of the repository.
fn worktree_lock_state(
    repository: &Path,
    checkout: &Path,
) -> Result<Option<(bool, Option<String>)>> {
    let spec = CommandSpec::new(
        vec![
            "git".to_string(),
            "-C".to_string(),
            repository.to_string_lossy().into_owned(),
            "worktree".to_string(),
            "list".to_string(),
            "--porcelain".to_string(),
            "-z".to_string(),
        ],
        Duration::from_secs(30),
    );
    let outcome = SubprocessRunner.run(&spec)?;
    if outcome.returncode != 0 {
        return Err(format!(
            "git worktree list failed in {}: {}",
            repository.display(),
            outcome.stderr.trim()
        ));
    }
    let target = crate::image_mounts::normalize_absolute(checkout);
    let mut found: Option<Vec<(String, Option<String>)>> = None;
    let mut current: Vec<(String, Option<String>)> = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    for field in outcome.stdout.split('\0') {
        if field.is_empty() {
            continue;
        }
        let (key, value) = match field.split_once(' ') {
            Some((key, value)) => (key, Some(value.to_string())),
            None => (field, None),
        };
        if key == "worktree" {
            if current_path.as_deref() == Some(target.as_path()) {
                found = Some(std::mem::take(&mut current));
            } else {
                current.clear();
            }
            current_path = Some(crate::image_mounts::normalize_absolute(Path::new(
                value.as_deref().unwrap_or_default(),
            )));
            continue;
        }
        current.push((key.to_string(), value));
    }
    if current_path.as_deref() == Some(target.as_path()) {
        found = Some(current);
    }
    let Some(fields) = found else {
        return Ok(None);
    };
    let locked = fields
        .iter()
        .find(|(key, _)| key == "locked")
        .map(|(_, reason)| (true, reason.clone()))
        .unwrap_or((false, None));
    Ok(Some(locked))
}

/// Python `_image_slots_missing_lock` for the selected machine:
/// `(missing, lock_check_error)`.
fn wt_lock_missing(
    config: &Value,
    root: Option<&Path>,
    machine: Option<&str>,
    settings: &WorktreesImageSettings,
    entries: &[MountEntry],
) -> (Vec<String>, Option<String>) {
    let run = || -> Result<Vec<String>> {
        let state = crate::active_state::parse_state(root, machine)?;
        let project_root = config_path(config, "root");
        let control = config_path(config, "control");
        let worktrees = config_path(config, "worktrees");
        let mount_dir = if settings.direct() {
            worktrees.clone()
        } else {
            worktrees
                .parent()
                .map(|parent| parent.join(crate::worktrees_image::IMAGE_MOUNT_NAME))
                .unwrap_or_default()
        };
        let (exists, empty) = dir_facts(&mount_dir);
        if classify_mount(&mount_dir, entries, exists, empty) != MOUNTED {
            return Ok(Vec::new());
        }
        let bind_slots: std::collections::BTreeMap<String, crate::worktrees_image::BindRecord> =
            if settings.direct() {
                Default::default()
            } else {
                crate::worktrees_image::load_bind_records(&control).unwrap_or_default()
            };
        let mut missing = Vec::new();
        let Some(records) = state.get("slots").and_then(Value::as_array) else {
            return Ok(missing);
        };
        for record in records {
            let slot = record
                .get("slot")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let slot_type = record
                .get("slot_type")
                .and_then(Value::as_str)
                .unwrap_or("agent");
            let slot_path = worktrees.join(slot);
            let in_image = if settings.direct() {
                use std::os::unix::fs::MetadataExt as _;
                std::fs::metadata(&slot_path)
                    .map(|meta| meta.ino() == 256)
                    .unwrap_or(false)
            } else {
                bind_slots.contains_key(slot)
            };
            if !in_image {
                continue;
            }
            let Some(checkouts) = record.get("checkouts").and_then(Value::as_array) else {
                continue;
            };
            for checkout in checkouts {
                let name = checkout
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let path = crate::image_mounts::normalize_absolute(
                    &project_root.join(
                        checkout
                            .get("path")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    ),
                );
                let repository = crate::image_mounts::normalize_absolute(
                    &project_root.join(
                        checkout
                            .get("repository")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    ),
                );
                if let Some(lock) = worktree_lock_state(&repository, &path)? {
                    if lock != (true, Some("in worktrees image".to_string())) {
                        missing.push(format!("{slot_type}/{slot}/{name}"));
                    }
                }
            }
        }
        Ok(missing)
    };
    match run() {
        Ok(missing) => (missing, None),
        Err(error) => (Vec::new(), Some(error)),
    }
}

/// Shared worktrees-image inputs: validated config, parsed
/// settings (None when unconfigured), and the live mount table.
struct WtInputs {
    config: Value,
    settings: Option<WorktreesImageSettings>,
    project_root: PathBuf,
    control: PathBuf,
    worktrees: PathBuf,
    entries: Vec<MountEntry>,
}

impl WtInputs {
    fn load(root: Option<&Path>, machine: Option<&str>) -> Result<Self> {
        let config = crate::config_validate::load_config(root, machine)?;
        let settings = match config.get("worktrees_image") {
            Some(section) if !section.is_null() => Some(settings_from_obj(section)?),
            _ => None,
        };
        Ok(Self {
            project_root: config_path(&config, "root"),
            control: config_path(&config, "control"),
            worktrees: config_path(&config, "worktrees"),
            entries: mount_entries(),
            config,
            settings,
        })
    }

    fn ctx(&self) -> WtCtx<'_> {
        WtCtx {
            root: &self.project_root,
            control: &self.control,
            worktrees: &self.worktrees,
            entries: &self.entries,
            skipped: self
                .config
                .get("worktrees_image_skipped")
                .and_then(Value::as_str),
        }
    }
}

/// `wrkslots worktrees-image create` (writer): with `--path` and
/// `--size-bytes` together the pair is validated as a section and
/// no configured section is read, as Python does before enabling.
pub(crate) fn wt_create_cli(
    root: Option<&Path>,
    machine: Option<&str>,
    path: Option<&str>,
    size_bytes: Option<i64>,
) -> Result<HandlerOutcome> {
    crate::ensure_states::require_fixture()?;
    let inputs = WtInputs::load(root, machine)?;
    let ctx = inputs.ctx();
    if let (Some(path), Some(size)) = (path, size_bytes) {
        let settings = settings_from_obj(&json!({"path": path, "size_bytes": size}))?;
        return Ok(crate::worktrees_image::wt_create(
            &ctx,
            Some(settings),
            &SubprocessRunner,
        ));
    }
    if path.is_some() != size_bytes.is_some() {
        return Ok(HandlerOutcome {
            exit: 3,
            stdout: String::new(),
            stderr: "REFUSED: --path and --size-bytes must be given together\nREMEDY: run 'wrkslots worktrees-image --help', correct the named condition, then rerun 'wrkslots worktrees-image' with the required arguments\n".to_string(),
        });
    }
    Ok(crate::worktrees_image::wt_create(
        &ctx,
        inputs.settings.clone(),
        &SubprocessRunner,
    ))
}

/// `wrkslots worktrees-image mount` (writer).
pub(crate) fn wt_mount_cli(root: Option<&Path>, machine: Option<&str>) -> Result<HandlerOutcome> {
    crate::ensure_states::require_fixture()?;
    let inputs = WtInputs::load(root, machine)?;
    let ctx = inputs.ctx();
    let mount_id = |target: &Path| -> Option<i64> {
        let text = std::fs::read_to_string("/proc/self/mountinfo").ok()?;
        let target = target.to_string_lossy();
        let mut found = None;
        for line in text.lines() {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() >= 5 && fields[4].replace("\\040", " ") == target {
                found = fields[0].parse::<i64>().ok();
            }
        }
        found
    };
    Ok(crate::worktrees_image::wt_mount(
        &ctx,
        inputs.settings.clone(),
        &mount_id,
        &SubprocessRunner,
    ))
}

/// `wrkslots worktrees-image unmount` (writer): the direct layout
/// refuses before anything runs, as Python does.
pub(crate) fn wt_unmount_cli(root: Option<&Path>, machine: Option<&str>) -> Result<HandlerOutcome> {
    crate::ensure_states::require_fixture()?;
    let inputs = WtInputs::load(root, machine)?;
    if let Some(settings) = &inputs.settings {
        if settings.direct() {
            return Ok(HandlerOutcome {
                exit: 3,
                stdout: String::new(),
                stderr: "REFUSED: in mount_at: worktrees_dir layout the image mounts at the slots directory and every wrkslots command remounts it; unmount it outside wrkslots only when no slot is in use, or switch layout with 'wrkslots worktrees-image migrate-layout --to binds' first\nREMEDY: run 'wrkslots worktrees-image --help', correct the named condition, then rerun 'wrkslots worktrees-image' with the required arguments\n".to_string(),
            });
        }
    }
    let ctx = inputs.ctx();
    Ok(crate::worktrees_image::wt_unmount(
        &ctx,
        inputs.settings.clone(),
        &SubprocessRunner,
    ))
}

/// `wrkslots worktrees-image grow` (writer).
pub(crate) fn wt_grow_cli(
    root: Option<&Path>,
    machine: Option<&str>,
    to_size_bytes: i64,
) -> Result<HandlerOutcome> {
    crate::ensure_states::require_fixture()?;
    let inputs = WtInputs::load(root, machine)?;
    let ctx = inputs.ctx();
    Ok(crate::worktrees_image::wt_grow(
        &ctx,
        inputs.settings.clone(),
        to_size_bytes,
        &SubprocessRunner,
    ))
}

/// `wrkslots worktrees-image trim` (writer).
pub(crate) fn wt_trim_cli(root: Option<&Path>, machine: Option<&str>) -> Result<HandlerOutcome> {
    crate::ensure_states::require_fixture()?;
    let inputs = WtInputs::load(root, machine)?;
    let ctx = inputs.ctx();
    Ok(crate::worktrees_image::wt_trim(
        &ctx,
        inputs.settings.clone(),
        &SubprocessRunner,
    ))
}

/// `wrkslots worktrees-image status`: build the full report
/// (quota when mounted, lock check) and render it.
pub(crate) fn wt_status_cli(
    root: Option<&Path>,
    machine: Option<&str>,
    format: &str,
) -> Result<HandlerOutcome> {
    let config = crate::config_validate::load_config(root, machine)?;
    let Some(section) = config
        .get("worktrees_image")
        .filter(|value| !value.is_null())
    else {
        let reason = config
            .get("worktrees_image_skipped")
            .and_then(Value::as_str)
            .map(|reason| {
                format!("worktrees_image does not apply on this host: {reason}")
            })
            .unwrap_or_else(|| {
                "no worktrees_image configured; add a worktrees_image section (path, size_bytes, fs: btrfs, mount: kernel) to enable it".to_string()
            });
        return Ok(HandlerOutcome {
            exit: 3,
            stdout: String::new(),
            stderr: format!(
                "REFUSED: {reason}\nREMEDY: run 'wrkslots worktrees-image --help', correct the named condition, then rerun 'wrkslots worktrees-image' with the required arguments\n"
            ),
        });
    };
    let settings = settings_from_obj(section)?;
    let project_root = config_path(&config, "root");
    let control = config_path(&config, "control");
    let worktrees = config_path(&config, "worktrees");
    let skipped = config
        .get("worktrees_image_skipped")
        .and_then(Value::as_str);
    let entries = mount_entries();
    let ctx = WtCtx {
        root: &project_root,
        control: &control,
        worktrees: &worktrees,
        entries: &entries,
        skipped,
    };
    let mount_dir = if settings.direct() {
        worktrees.clone()
    } else {
        worktrees
            .parent()
            .map(|parent| parent.join(crate::worktrees_image::IMAGE_MOUNT_NAME))
            .unwrap_or_default()
    };
    let (exists, empty) = dir_facts(&mount_dir);
    let quota = if classify_mount(&mount_dir, &entries, exists, empty) == MOUNTED {
        Some(quota_mode_at(
            &SubprocessRunner,
            &mount_dir,
            &settings.image_file(&project_root),
        ))
    } else {
        None
    };
    let mut report = status_report_with_quota(&ctx, &settings, quota);
    let (missing, lock_error) = wt_lock_missing(&config, root, machine, &settings, &entries);
    if let Some(object) = report.as_object_mut() {
        object.insert("lock_missing".to_string(), json!(missing));
        if let Some(error) = lock_error {
            object.insert("lock_check_error".to_string(), json!(error));
        }
    }
    Ok(HandlerOutcome {
        exit: 0,
        stdout: crate::worktrees_image::render_wt_status(&report, format),
        stderr: String::new(),
    })
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
    let facts = mount_fact;
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
