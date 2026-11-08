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

#[cfg(test)]
mod tests {
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
}
