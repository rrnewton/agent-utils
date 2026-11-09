//! Sandbox exec plans behind the Runner seam (P9 S9, mu-rsimg,
//! unwired until dispatch).
//!
//! Mirrors the external-command half of Python `sandbox.py`:
//! `apply_slice_limits` (systemctl start/set-property pairs),
//! `place_only` placement, the `busctl` scope-in-place call, the
//! `systemd-run --scope` fallback, and the userns/root helper argv.
//! Namespace entry itself is a later dispatch-time step. Effects go
//! through `image_ops::PrivilegedRunner`; tests use a recording
//! fake, and nothing here starts a scope, enters a namespace, or
//! touches a cgroup.

use std::time::Duration;

use serde_json::{json, Value};

use crate::image_ops::{CommandSpec, PrivilegedRunner};
use crate::sandbox_state::{
    all_slots_properties, root_slice, slice_limit_properties, slice_name, SandboxLimits,
};

fn spec(argv: Vec<String>, timeout_s: u64) -> CommandSpec {
    CommandSpec::new(argv, Duration::from_secs(timeout_s))
}

/// One slice's limit application: start (idempotent load) then
/// set-property --runtime, exactly as Python `apply_slice_limits`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SliceApplication {
    pub(crate) unit: String,
    pub(crate) properties: Vec<String>,
}

impl SliceApplication {
    fn specs(&self) -> Vec<CommandSpec> {
        let mut argv = vec![
            "systemctl".to_string(),
            "--user".to_string(),
            "set-property".to_string(),
            "--runtime".to_string(),
            self.unit.clone(),
        ];
        argv.extend(self.properties.iter().cloned());
        vec![
            spec(
                vec![
                    "systemctl".to_string(),
                    "--user".to_string(),
                    "start".to_string(),
                    self.unit.clone(),
                ],
                120,
            ),
            spec(argv, 120),
        ]
    }

    /// The applied line Python prints: `"<unit>: <props joined>"`.
    pub(crate) fn applied_line(&self) -> String {
        format!("{}: {}", self.unit, self.properties.join(" "))
    }
}

/// The ordered slice applications for one run (all-slots first,
/// then the slot's), skipping slices with no properties, mirroring
/// `apply_slice_limits`' loop.
pub(crate) fn slice_application_plan(
    limits: &SandboxLimits,
    slot_type: &str,
    slot: &str,
    parent: Option<&str>,
    cgroup_text: Option<&str>,
) -> Vec<SliceApplication> {
    let root = root_slice(parent, cgroup_text);
    let slot_unit = slice_name(slot_type, slot, parent, cgroup_text);
    [
        (root, all_slots_properties(limits)),
        (slot_unit, slice_limit_properties(limits, Some(slot))),
    ]
    .into_iter()
    .filter(|(_, properties)| !properties.is_empty())
    .map(|(unit, properties)| SliceApplication { unit, properties })
    .collect()
}

/// Execute a slice plan through the runner; a failing set-property
/// refuses with Python's `"cannot set limits on <unit>: <stderr>"`.
pub(crate) fn run_slice_plan(
    runner: &impl PrivilegedRunner,
    plan: &[SliceApplication],
) -> Result<Vec<String>, String> {
    let mut applied = Vec::new();
    for application in plan {
        for item in application.specs() {
            let is_set_property = item.argv.contains(&"set-property".to_string());
            let outcome = runner.run(&item)?;
            if is_set_property && outcome.returncode != 0 {
                return Err(format!(
                    "cannot set limits on {}: {}",
                    application.unit,
                    outcome.stderr.trim()
                ));
            }
        }
        applied.push(application.applied_line());
    }
    Ok(applied)
}

/// The three slice names a caller places its own scope under,
/// mirroring Python `SlicePlacement` and its JSON form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SlicePlacement {
    pub(crate) enclosing: Option<String>,
    pub(crate) all_slots: String,
    pub(crate) slot: String,
}

impl SlicePlacement {
    pub(crate) fn to_obj(&self) -> Value {
        json!({
            "enclosing": self.enclosing,
            "all_slots": self.all_slots,
            "slot": self.slot,
        })
    }
}

/// Compute placement from an injected cgroup text without applying
/// anything (the pure half of Python `place_only`).
pub(crate) fn placement(slot_type: &str, slot: &str, cgroup_text: Option<&str>) -> SlicePlacement {
    let enclosing = crate::sandbox_state::parent_slice_from_text(cgroup_text);
    let parent = enclosing.as_deref().unwrap_or("");
    SlicePlacement {
        enclosing: enclosing.clone(),
        all_slots: root_slice(Some(parent), None),
        slot: slice_name(slot_type, slot, Some(parent), None),
    }
}

/// The default transient scope unit for one launch, mirroring
/// Python `run`'s `wrkslots-{kind}-{slot}-{pid}.scope`.
pub(crate) fn scope_unit_name(kind: &str, slot: &str, pid: u32) -> String {
    format!(
        "wrkslots-{kind}-{}-{pid}.scope",
        crate::sandbox_state::systemd_escape(slot)
    )
}

/// The `busctl` call that moves this process into a transient scope
/// without forking, mirroring Python `place_in_scope`.
pub(crate) fn place_in_scope_argv(unit: &str, slice_unit: &str, pid: u32) -> Vec<String> {
    vec![
        "busctl".to_string(),
        "--user".to_string(),
        "call".to_string(),
        "org.freedesktop.systemd1".to_string(),
        "/org/freedesktop/systemd1".to_string(),
        "org.freedesktop.systemd1.Manager".to_string(),
        "StartTransientUnit".to_string(),
        "ssa(sv)a(sa(sv))".to_string(),
        unit.to_string(),
        "fail".to_string(),
        "2".to_string(),
        "PIDs".to_string(),
        "au".to_string(),
        "1".to_string(),
        pid.to_string(),
        "Slice".to_string(),
        "s".to_string(),
        slice_unit.to_string(),
        "0".to_string(),
    ]
}

/// The fallback when the D-Bus path is unavailable, mirroring
/// Python `run`'s `systemd-run --scope` argv (the PID differs).
pub(crate) fn scope_fallback_argv(unit: &str, slice_unit: &str, argv: &[String]) -> Vec<String> {
    let mut fallback = vec![
        "systemd-run".to_string(),
        "--user".to_string(),
        "--scope".to_string(),
        "--quiet".to_string(),
        "--collect".to_string(),
        format!("--unit={unit}"),
        format!("--slice={slice_unit}"),
        "--".to_string(),
    ];
    fallback.extend(argv.iter().cloned());
    fallback
}

/// Argv that builds the view in a user namespace, mirroring Python
/// `userns_helper`: `[python, -I, sandbox.py, enter, SPEC-JSON, --, CMD...]`.
/// The interpreter and module paths are injected for the Rust
/// binary's own helper subcommand at dispatch time.
pub(crate) fn userns_helper_argv(
    interpreter: &str,
    module: &str,
    spec_json: &str,
    command: &[String],
) -> Result<Vec<String>, String> {
    if command.is_empty() {
        return Err("no command to run".to_string());
    }
    let mut argv = vec![
        interpreter.to_string(),
        "-I".to_string(),
        module.to_string(),
        "enter".to_string(),
        spec_json.to_string(),
        "--".to_string(),
    ];
    argv.extend(command.iter().cloned());
    Ok(argv)
}

/// Argv that builds the view as root through `sudo -n`, mirroring
/// Python `root_helper` (`-B` only when bytecode writing is off).
pub(crate) fn root_helper_argv(
    interpreter: &str,
    module: &str,
    spec_file: &str,
    dont_write_bytecode: bool,
) -> Vec<String> {
    let mut argv = vec![
        "sudo".to_string(),
        "-n".to_string(),
        "--".to_string(),
        interpreter.to_string(),
        "-I".to_string(),
    ];
    if dont_write_bytecode {
        argv.push("-B".to_string());
    }
    argv.push(module.to_string());
    argv.push("enter-root".to_string());
    argv.push(spec_file.to_string());
    argv
}

/// `wrkslots limits {show,apply,clear}`: the dispatch-shaped
/// handler. Host facts (uid, physical bytes, cpu count) and the
/// runner are injected; the properties and outputs mirror Python
/// `_cmd_limits` (`imagecmd.py:1727`) byte for byte.
pub(crate) fn limits(
    runner: &impl PrivilegedRunner,
    uid: u32,
    action: &str,
    memory_fraction: f64,
    cpu_fraction: f64,
    physical_bytes: u64,
    cpu_count: u64,
) -> (i32, String, String) {
    let unit = format!("user-{uid}.slice");
    let refusal = |message: String| -> (i32, String, String) {
        (
            3,
            String::new(),
            format!(
                "REFUSED: {message}\nREMEDY: run 'wrkslots limits --help', correct the named condition, then rerun 'wrkslots limits' with the required arguments\n"
            ),
        )
    };
    if action == "apply" || action == "clear" {
        let properties: Vec<String> = if action == "apply" {
            for (label, value) in [
                ("--memory-fraction", memory_fraction),
                ("--cpu-fraction", cpu_fraction),
            ] {
                if !(value > 0.0 && value <= 1.0) {
                    return refusal(format!("{label} must be greater than 0 and at most 1"));
                }
            }
            let memory = (physical_bytes as f64 * memory_fraction) as u64;
            let cpu = ((cpu_count.max(1) * 100) as f64 * cpu_fraction) as u64;
            vec![
                format!("MemoryMax={memory}"),
                "MemorySwapMax=0".to_string(),
                format!("CPUQuota={}%", cpu.max(1)),
            ]
        } else {
            vec![
                "MemoryMax=infinity".to_string(),
                "MemorySwapMax=infinity".to_string(),
                "CPUQuota=".to_string(),
            ]
        };
        let mut argv = vec![
            "sudo".to_string(),
            "-n".to_string(),
            "systemctl".to_string(),
            "set-property".to_string(),
            "--runtime".to_string(),
            unit.clone(),
        ];
        argv.extend(properties);
        match runner.run(&CommandSpec::new(argv, Duration::from_secs(120))) {
            Ok(outcome) if outcome.returncode == 0 => {}
            Ok(outcome) => {
                return refusal(format!(
                    "cannot set {unit} limits (needs sudo -n): {}",
                    outcome.stderr.trim()
                ));
            }
            Err(message) => return refusal(message),
        }
    }
    let show_argv = vec![
        "systemctl".to_string(),
        "show".to_string(),
        unit.clone(),
        "--property=MemoryMax".to_string(),
        "--property=MemorySwapMax".to_string(),
        "--property=CPUQuotaPerSecUSec".to_string(),
    ];
    match runner.run(&CommandSpec::new(show_argv, Duration::from_secs(120))) {
        Ok(outcome) => {
            let mut stdout = format!("{unit}:\n");
            for line in outcome.stdout.lines() {
                stdout.push_str(&format!("  {line}\n"));
            }
            (
                if outcome.returncode == 0 { 0 } else { 1 },
                stdout,
                String::new(),
            )
        }
        Err(message) => (1, format!("{unit}:\n"), message),
    }
}

#[cfg(test)]
mod tests {
    const LIMITS_DRIVER: &str = r####"import argparse, os, subprocess, sys
sys.path.insert(0, os.environ["WRKSLOTS_PY_ROOT"])
from wrkslots import cli, imagecmd

case, action = sys.argv[1], sys.argv[2]
os.getuid = lambda: 1000
os.sysconf = lambda name: {"SC_PHYS_PAGES": 1000000, "SC_PAGE_SIZE": 4096}[name]
os.cpu_count = lambda: 8
calls = []
def fake_run(argv, **kwargs):
    calls.append(argv)
    if argv[0] == "sudo":
        if case == "failing":
            return subprocess.CompletedProcess(argv, 1, "", "denied\n")
        return subprocess.CompletedProcess(argv, 0, "", "")
    return subprocess.CompletedProcess(argv, 0, "MemoryMax=858993459\nMemorySwapMax=0\nCPUQuotaPerSecUSec=720000\n", "")
subprocess.run = fake_run
args = argparse.Namespace(action=action, memory_fraction=0.2, cpu_fraction=0.9)
import io
from contextlib import redirect_stdout, redirect_stderr
out, err = io.StringIO(), io.StringIO()
try:
    with redirect_stdout(out), redirect_stderr(err):
        rc = imagecmd._cmd_limits(args)
except cli.Refusal as exc:
    remedy = cli._refusal_remedy(argparse.Namespace(command="limits"), exc)
    sys.stdout.write(f"3\n{out.getvalue()}---STDERR---\nREFUSED: {exc}\nREMEDY: {remedy}\n")
    sys.exit(0)
sys.stdout.write(f"{rc}\n{out.getvalue()}---STDERR---\n{err.getvalue()}")
"####;
    use super::*;
    use crate::image_ops::RunOutcome;
    use crate::sandbox_state::limits_from_obj;
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
                stderr: "denied".to_string(),
            })
        }
    }

    #[test]
    fn slice_plan_and_scope_argv_match_python() {
        let limits = limits_from_obj(&json!({
            "memory_max": "32G",
            "all_slots_cpu_quota": "1600%"
        }))
        .expect("limits");
        let plan = slice_application_plan(&limits, "agent", "slot-a", Some(""), None);
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[0].unit, "wrkslots.slice");
        assert_eq!(plan[1].unit, "wrkslots-slot\\x2da.slice");
        assert_eq!(
            plan[1].applied_line(),
            "wrkslots-slot\\x2da.slice: MemoryMax=32G MemorySwapMax=0 TasksMax=8192"
        );

        let fake = Fake {
            calls: RefCell::new(Vec::new()),
            returncode: 0,
        };
        let applied = run_slice_plan(&fake, &plan).expect("applied");
        assert_eq!(applied.len(), 2);
        // start + set-property per slice, in plan order.
        assert_eq!(fake.calls.borrow().len(), 4);
        assert_eq!(fake.calls.borrow()[0][3], "wrkslots.slice");

        let failing = Fake {
            calls: RefCell::new(Vec::new()),
            returncode: 1,
        };
        // `start` failing is tolerated by the oracle; set-property is not.
        let plan = &plan[1..];
        let error = run_slice_plan(&failing, plan).expect_err("set-property refusal");
        assert!(error.contains("cannot set limits on"), "{error}");

        let place = placement(
            "agent",
            "slot-a",
            Some("0::/user.slice/user@1000.service/s.slice\n"),
        );
        assert_eq!(place.enclosing.as_deref(), Some("s.slice"));
        assert_eq!(place.to_obj()["all_slots"], "s-wrkslots.slice");

        assert_eq!(
            scope_unit_name("run", "slot-a", 42),
            "wrkslots-run-slot\\x2da-42.scope"
        );
        let bus = place_in_scope_argv("u.scope", "wrkslots.slice", 42);
        assert_eq!(bus[0], "busctl");
        assert!(bus.contains(&"StartTransientUnit".to_string()));
        let fallback = scope_fallback_argv("u.scope", "wrkslots.slice", &["/bin/true".to_string()]);
        assert_eq!(fallback[6], "--slice=wrkslots.slice");
        let root = root_helper_argv("/usr/bin/python3", "sandbox.py", "/tmp/spec.json", true);
        assert_eq!(root[5], "-B");
    }

    /// Oracle differential (#1112): Python `_cmd_limits` with
    /// stubbed host facts and subprocess vs this handler through a
    /// recording fake, byte-for-byte including refusals.
    #[test]
    fn limits_matches_python_oracle() {
        use std::cell::RefCell;
        struct Fake {
            calls: RefCell<Vec<Vec<String>>>,
            fail_set: bool,
        }
        impl PrivilegedRunner for Fake {
            fn run(&self, spec: &CommandSpec) -> Result<RunOutcome, String> {
                self.calls.borrow_mut().push(spec.argv.clone());
                if spec.argv[0] == "sudo" && self.fail_set {
                    return Ok(RunOutcome {
                        returncode: 1,
                        stdout: String::new(),
                        stderr: "denied\n".to_string(),
                    });
                }
                if spec.argv[0] == "sudo" {
                    return Ok(RunOutcome {
                        returncode: 0,
                        stdout: String::new(),
                        stderr: String::new(),
                    });
                }
                Ok(RunOutcome {
                    returncode: 0,
                    stdout: "MemoryMax=858993459\nMemorySwapMax=0\nCPUQuotaPerSecUSec=720000\n"
                        .to_string(),
                    stderr: String::new(),
                })
            }
        }
        let repo = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("repo root")
            .to_path_buf();
        let dir = std::env::temp_dir().join(format!("wrkslotsd-limits-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let driver_path = dir.join("limits_driver.py");
        std::fs::write(&driver_path, LIMITS_DRIVER).expect("driver");
        // physical bytes = 1_000_000 pages * 4096; cpu_count = 8.
        for (case, action, fail_set) in [
            ("ok", "show", false),
            ("ok", "apply", false),
            ("ok", "clear", false),
            ("failing", "apply", true),
        ] {
            let output = std::process::Command::new("python3")
                .arg(&driver_path)
                .arg(case)
                .arg(action)
                .env("WRKSLOTS_PY_ROOT", repo.join("py"))
                .output()
                .expect("python oracle runs");
            let text = String::from_utf8(output.stdout).expect("utf8");
            let (head, rest) = text.split_once('\n').expect("exit line");
            let (stdout, stderr) = rest.split_once("---STDERR---\n").expect("marker");
            let fake = Fake {
                calls: RefCell::new(Vec::new()),
                fail_set,
            };
            let outcome = limits(&fake, 1000, action, 0.2, 0.9, 1_000_000 * 4096, 8);
            assert_eq!(
                outcome,
                (
                    head.trim().parse::<i32>().expect("exit"),
                    stdout.to_string(),
                    stderr.to_string()
                ),
                "case {case} action {action}"
            );
            if action == "apply" && !fail_set {
                assert!(fake.calls.borrow()[0].contains(&"CPUQuota=720%".to_string()));
            }
        }
        // Fraction refusal is computed identically on both sides.
        let fake = Fake {
            calls: RefCell::new(Vec::new()),
            fail_set: false,
        };
        let (exit, _out, stderr) = limits(&fake, 1000, "apply", 0.0, 0.9, 4096, 8);
        assert_eq!(exit, 3);
        assert!(
            stderr.contains("--memory-fraction must be greater than 0 and at most 1"),
            "{stderr}"
        );
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}
