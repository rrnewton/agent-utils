//! Privileged image operations behind a Runner seam (P8 S5, mu-rsimg,
//! unwired until dispatch).
//!
//! Mirrors the argv construction of Python `slotimage.py`:
//! `_privileged_argv`/`_privileged` (the `sudo -n` helper),
//! `_user_manager_argv`/`_host_argv` (host-namespace routing from a
//! coordinator box), and the linear plans for `trim` and `grow`.
//! Every effect goes through [`PrivilegedRunner`]; production will
//! supply a subprocess runner at dispatch wiring, tests supply a
//! recording fake. Nothing in this module executes a command, and
//! no test mounts, formats, or touches a live image.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::image_record::ImageRecord;

/// One external command to run, with its timeout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CommandSpec {
    pub(crate) argv: Vec<String>,
    pub(crate) timeout: Duration,
}

impl CommandSpec {
    pub(crate) fn new(argv: Vec<String>, timeout: Duration) -> Self {
        Self { argv, timeout }
    }
}

/// The observed result of one command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RunOutcome {
    pub(crate) returncode: i32,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

/// The seam every privileged effect goes through. Implementations
/// must run exactly `spec.argv` (no shell) and honour its timeout.
pub(crate) trait PrivilegedRunner {
    fn run(&self, spec: &CommandSpec) -> Result<RunOutcome, String>;
}

/// Paths the privileged argv is built from, injected so tests never
/// resolve a real sudo/python/helper on the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ToolPaths {
    pub(crate) sudo: String,
    pub(crate) python: String,
    pub(crate) helper: PathBuf,
    pub(crate) systemd_run: Option<String>,
    pub(crate) fusermount: String,
}

impl ToolPaths {
    /// Python `_privileged_argv`: `[sudo, -n, --, python, -I, helper]`.
    pub(crate) fn privileged_argv(&self) -> Vec<String> {
        vec![
            self.sudo.clone(),
            "-n".to_string(),
            "--".to_string(),
            self.python.clone(),
            "-I".to_string(),
            self.helper.to_string_lossy().into_owned(),
        ]
    }

    /// One helper operation: privileged argv plus `operation` and args.
    pub(crate) fn helper_argv(&self, operation: &str, arguments: &[String]) -> Vec<String> {
        let mut argv = self.privileged_argv();
        argv.push(operation.to_string());
        argv.extend(arguments.iter().cloned());
        argv
    }
}

/// Which wrkslots box the caller runs in, mirroring Python
/// `box_kind`'s result (its detection reads the environment and
/// mount table in the caller, never here).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BoxKind {
    None,
    Coordinator,
    Slot,
}

pub(crate) const SLOT_BOX_REFUSAL: &str = "slot images cannot be mounted, unmounted, converted, or removed from inside a slot's box (its mounts are private to it); run this outside the box, or from a coordinator box (`wrkslots box`)";

/// Route `argv` to the host namespace, mirroring Python
/// `_via_user_manager` + `_host_argv`: plain outside a box, through
/// `systemd-run --user` from a coordinator box whose anchor is
/// writable, refused from a slot's box.
pub(crate) fn host_argv(
    tools: &ToolPaths,
    argv: &[String],
    kind: BoxKind,
    anchor_writable: bool,
) -> Result<Vec<String>, String> {
    match kind {
        BoxKind::None => Ok(argv.to_vec()),
        BoxKind::Slot => Err(SLOT_BOX_REFUSAL.to_string()),
        BoxKind::Coordinator => {
            if !anchor_writable {
                return Err(SLOT_BOX_REFUSAL.to_string());
            }
            let Some(systemd_run) = &tools.systemd_run else {
                return Err(
                    "systemd-run is not installed; image mounts requested from inside a coordinator box run in the host's mount namespace through the user's systemd manager (systemd-run --user); if that is unavailable, create and remove image slots outside the box"
                        .to_string(),
                );
            };
            let mut routed = vec![
                systemd_run.clone(),
                "--user".to_string(),
                "--quiet".to_string(),
                "--collect".to_string(),
                "--wait".to_string(),
                "--pipe".to_string(),
                "--service-type=exec".to_string(),
                "--".to_string(),
            ];
            routed.extend(argv.iter().cloned());
            Ok(routed)
        }
    }
}

/// The `trim` plan for one image, mirroring Python `slotimage.trim`:
/// one step per mounted image (slot then state), kernel through the
/// helper, FUSE through `fstrim`. Unmounted images produce no step.
pub(crate) fn trim_plan(
    tools: &ToolPaths,
    record: &ImageRecord,
    slot_backend: Option<&str>,
    state_backend: Option<&str>,
    kind: BoxKind,
) -> Result<Vec<(String, CommandSpec)>, String> {
    let mut plan = Vec::new();
    for (image_file, mount_point, backend) in [
        (record.image_file(), record.location.clone(), slot_backend),
        (record.state_image(), record.state_mount(), state_backend),
    ] {
        let Some(backend) = backend else { continue };
        let argv = if backend == "kernel" {
            tools.helper_argv(
                "trim",
                &[
                    image_file.to_string_lossy().into_owned(),
                    mount_point.to_string_lossy().into_owned(),
                ],
            )
        } else {
            vec![
                "fstrim".to_string(),
                mount_point.to_string_lossy().into_owned(),
            ]
        };
        let routed = host_argv(tools, &argv, kind, true)?;
        plan.push((
            mount_point.to_string_lossy().into_owned(),
            CommandSpec::new(routed, Duration::from_secs(120)),
        ));
    }
    Ok(plan)
}

/// Execute a trim plan through the runner, mirroring Python's
/// result strings: `"<mount>: trimmed"` on success, the command's
/// stderr text otherwise. A nonzero helper exit is reported, not
/// raised, exactly as the oracle does.
pub(crate) fn run_trim(
    runner: &dyn PrivilegedRunner,
    plan: &[(String, CommandSpec)],
) -> Result<Vec<String>, String> {
    let mut done = Vec::new();
    for (mount_point, spec) in plan {
        let outcome = runner.run(spec)?;
        if outcome.returncode == 0 {
            done.push(format!("{mount_point}: trimmed"));
        } else {
            done.push(format!("{mount_point}: {}", outcome.stderr.trim()));
        }
    }
    Ok(done)
}

/// The grow ceiling decision, mirroring Python `slotimage.grow`'s
/// refusal: the new ceiling must exceed the current file size.
pub(crate) fn grow_refusal(current_bytes: u64, new_ceiling_bytes: u64) -> Option<String> {
    if new_ceiling_bytes <= current_bytes {
        Some(format!(
            "new ceiling {new_ceiling_bytes} must exceed the current {current_bytes} bytes; shrinking is not supported"
        ))
    } else {
        None
    }
}

/// The offline/online grow steps after the ceiling decision, in
/// Python's order. Online (kernel mounted): truncate then helper
/// `grow`. Offline: `e2fsck -p -f` then `resize2fs` (the caller
/// truncates between unmount and these steps, as the oracle does).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GrowMode {
    OnlineKernel,
    Offline,
}

pub(crate) fn grow_plan(
    tools: &ToolPaths,
    record: &ImageRecord,
    mode: GrowMode,
    kind: BoxKind,
) -> Result<Vec<CommandSpec>, String> {
    let image_file = record.image_file().to_string_lossy().into_owned();
    let specs = match mode {
        GrowMode::OnlineKernel => vec![CommandSpec::new(
            tools.helper_argv(
                "grow",
                &[image_file, record.location.to_string_lossy().into_owned()],
            ),
            Duration::from_secs(600),
        )],
        GrowMode::Offline => vec![
            CommandSpec::new(
                vec![
                    "e2fsck".to_string(),
                    "-p".to_string(),
                    "-f".to_string(),
                    image_file.clone(),
                ],
                Duration::from_secs(600),
            ),
            CommandSpec::new(
                vec!["resize2fs".to_string(), image_file],
                Duration::from_secs(600),
            ),
        ],
    };
    specs
        .into_iter()
        .map(|spec| {
            Ok(CommandSpec::new(
                host_argv(tools, &spec.argv, kind, true)?,
                spec.timeout,
            ))
        })
        .collect()
}

/// The conversion staging directory name, mirroring Python's
/// `.{slot}.convert-<hex8>` (lowercase hex, exactly 8 chars).
pub(crate) fn convert_staging_name(slot: &str, hex8: &str) -> Option<String> {
    if hex8.len() == 8
        && hex8
            .chars()
            .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase())
    {
        Some(format!(".{slot}.convert-{hex8}"))
    } else {
        None
    }
}

/// Whether a convert is a no-op, mirroring `_cmd_image_convert`'s
/// already-target outcome (`"<type>/<slot> is already <target>-backed"`).
pub(crate) fn convert_already(current: &str, target: &str) -> bool {
    current == target
}

/// The copy step of `convert`, mirroring `slotimage.copy_tree`'s
/// command (contents of source into the existing destination).
pub(crate) fn copy_tree_spec(source: &Path, destination: &Path) -> CommandSpec {
    CommandSpec::new(
        vec![
            "cp".to_string(),
            "-a".to_string(),
            "--reflink=auto".to_string(),
            format!("{}/.", source.display()),
            format!("{}/", destination.display()),
        ],
        Duration::from_secs(24 * 3600),
    )
}

/// Canonicalize one `tree_listing` row set the way Python
/// `slotimage.tree_listing` does: directory sizes become `-`,
/// mtimes truncate to whole seconds, rows sorted. Input rows are
/// the already-collected `find -printf '%P\t%y\t%s\t%m\t%T@\t%l'`
/// lines (collection is the caller's `find` step, never run here).
pub(crate) fn canonical_listing(rows: &[String]) -> Vec<String> {
    let mut out: Vec<String> = rows
        .iter()
        .map(|row| {
            let fields: Vec<&str> = row.split('\t').collect();
            if fields.len() == 6 {
                let size = if fields[1] == "d" { "-" } else { fields[2] };
                let mtime = fields[4].split('.').next().unwrap_or(fields[4]);
                format!(
                    "{}\t{}\t{}\t{}\t{}\t{}",
                    fields[0], fields[1], size, fields[3], mtime, fields[5]
                )
            } else {
                row.clone()
            }
        })
        .collect();
    out.sort();
    out
}

/// The kept-original name for `convert --keep-original`, mirroring
/// `_convert_to_image`'s `pre-image-original` sibling.
pub(crate) fn pre_image_original_name() -> &'static str {
    "pre-image-original"
}

/// The FUSE mount probe, mirroring Python `_fuse_mount`'s
/// systemd-run form: a transient user service running
/// `fuse2fs -f`, nested in the caller's images slice. `unit` is
/// the full transient unit name the caller composed
/// (`wrkslots-fuse-<digest>-<pid>-<epoch>`).
pub(crate) fn fuse_mount_spec(
    systemd_run: &str,
    image_file: &Path,
    mount_point: &Path,
    unit: &str,
    images_slice: &str,
) -> CommandSpec {
    CommandSpec::new(
        vec![
            systemd_run.to_string(),
            "--user".to_string(),
            "--quiet".to_string(),
            "--collect".to_string(),
            format!("--unit={unit}"),
            format!("--slice={images_slice}"),
            format!(
                "--working-directory={}",
                image_file
                    .parent()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default()
            ),
            "--".to_string(),
            "fuse2fs".to_string(),
            "-f".to_string(),
            image_file.to_string_lossy().into_owned(),
            mount_point.to_string_lossy().into_owned(),
        ],
        Duration::from_secs(30),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct RecordingRunner {
        calls: RefCell<Vec<Vec<String>>>,
        returncode: i32,
    }

    impl PrivilegedRunner for RecordingRunner {
        fn run(&self, spec: &CommandSpec) -> Result<RunOutcome, String> {
            self.calls.borrow_mut().push(spec.argv.clone());
            Ok(RunOutcome {
                returncode: self.returncode,
                stdout: String::new(),
                stderr: "nope".to_string(),
            })
        }
    }

    fn tools() -> ToolPaths {
        ToolPaths {
            sudo: "sudo".to_string(),
            python: "/usr/bin/python3".to_string(),
            helper: PathBuf::from("/opt/wrkslots/imagehelper.py"),
            systemd_run: Some("/usr/bin/systemd-run".to_string()),
            fusermount: "fusermount".to_string(),
        }
    }

    fn record() -> ImageRecord {
        ImageRecord {
            directory: PathBuf::from("/ctl/slot-images/agent/slot-a"),
            slot: "slot-a".to_string(),
            slot_type: "agent".to_string(),
            location: PathBuf::from("/slots/slot-a"),
            ceiling_bytes: 8,
            state_ceiling_bytes: 2,
            backend: "kernel".to_string(),
            created_at: "2026-11-07T00:00:00+00:00".to_string(),
            phase: "ready".to_string(),
        }
    }

    #[test]
    fn argv_builders_match_python() {
        let tools = tools();
        assert_eq!(
            tools.helper_argv("mount", &["a".to_string()]),
            [
                "sudo",
                "-n",
                "--",
                "/usr/bin/python3",
                "-I",
                "/opt/wrkslots/imagehelper.py",
                "mount",
                "a"
            ]
        );
        let routed =
            host_argv(&tools, &["x".to_string()], BoxKind::Coordinator, true).expect("routed");
        assert!(routed.contains(&"--service-type=exec".to_string()));
        assert_eq!(routed.last().map(String::as_str), Some("x"));
        assert!(host_argv(&tools, &["x".to_string()], BoxKind::Slot, true).is_err());
    }

    #[test]
    fn convert_helpers_match_python() {
        assert_eq!(
            convert_staging_name("slot-a", "0123abcd").as_deref(),
            Some(".slot-a.convert-0123abcd")
        );
        assert_eq!(convert_staging_name("slot-a", "0123ABCD"), None);
        assert!(convert_already("image", "image"));
        let rows = vec![
            "sub\tf\t3\t644\t1700000000.5\t".to_string(),
            "sub\td\t4096\t755\t1700000001.9\t".to_string(),
        ];
        let canonical = canonical_listing(&rows);
        assert_eq!(canonical[0], "sub\td\t-\t755\t1700000001\t");
        assert_eq!(canonical[1], "sub\tf\t3\t644\t1700000000\t");
        let copy = copy_tree_spec(Path::new("/a"), Path::new("/b"));
        assert_eq!(copy.argv, ["cp", "-a", "--reflink=auto", "/a/.", "/b/"]);
    }

    #[test]
    fn trim_and_grow_plans_use_the_runner_seam() {
        let tools = tools();
        let record = record();
        // Unmounted images produce no trim steps, as in the oracle.
        assert!(trim_plan(&tools, &record, None, None, BoxKind::None)
            .expect("plan")
            .is_empty());
        let plan =
            trim_plan(&tools, &record, Some("kernel"), Some("fuse"), BoxKind::None).expect("plan");
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[0].1.argv[6], "trim");
        assert_eq!(plan[1].1.argv[0], "fstrim");

        let runner = RecordingRunner {
            calls: RefCell::new(Vec::new()),
            returncode: 0,
        };
        let done = run_trim(&runner, &plan).expect("trim");
        assert_eq!(done.len(), 2);
        assert!(done[0].ends_with(": trimmed"), "{done:?}");
        assert_eq!(runner.calls.borrow().len(), 2);

        assert!(grow_refusal(100, 100).is_some());
        assert!(grow_refusal(100, 101).is_none());
        let online =
            grow_plan(&tools, &record, GrowMode::OnlineKernel, BoxKind::None).expect("grow");
        assert_eq!(online[0].argv[6], "grow");
        let offline = grow_plan(&tools, &record, GrowMode::Offline, BoxKind::None).expect("grow");
        assert_eq!(offline[0].argv[0], "e2fsck");
        assert_eq!(offline[1].argv[0], "resize2fs");
    }
}
