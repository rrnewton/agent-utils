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

use std::path::{Path, PathBuf};
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

/// Which `run` options were given alongside `--place-only`,
/// mirroring the flags Python `_cmd_run`/`_place_only` check.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PlaceOnlyGiven {
    pub(crate) command: bool,
    pub(crate) print_only: bool,
    pub(crate) cwd: bool,
    pub(crate) in_scope: bool,
    pub(crate) expect_slices: bool,
    pub(crate) expect_writable: bool,
    pub(crate) git_config_policy: bool,
    pub(crate) expect_masks: bool,
    pub(crate) closed_set: bool,
}

fn run_refusal(message: &str) -> (i32, String, String) {
    refusal_for(message, "run")
}

fn refusal_for(message: &str, command: &str) -> (i32, String, String) {
    (
        3,
        String::new(),
        format!(
            "REFUSED: {message}\nREMEDY: run 'wrkslots {command} --help', correct the named condition, then rerun 'wrkslots {command}' with the required arguments\n"
        ),
    )
}

/// Quote one argv word the way Python `shlex.quote` does.
pub(crate) fn shlex_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    let safe = value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'_' | b'@' | b'%' | b'+' | b'=' | b':' | b',' | b'.' | b'/' | b'-'
            )
    });
    if safe {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

/// Host facts for `run_print`, injected so tests never probe
/// the real systemd or cgroup state.
pub(crate) struct RunPrintFacts {
    pub(crate) home: PathBuf,
    pub(crate) pid: u32,
    pub(crate) cgroup_text: Option<String>,
    pub(crate) systemd_run_available: bool,
    pub(crate) spec: crate::sandbox_state::SpecFacts,
    /// Interpreter and sandbox module the helper argv names
    /// (the Rust binary's own helper at dispatch time).
    pub(crate) helper_interpreter: String,
    pub(crate) helper_module: String,
    pub(crate) dont_write_bytecode: bool,
}

/// `wrkslots run|box --print` (#1317): print the boxed command
/// without running it, mirroring Python `sandbox.run`'s
/// `print_only` branch for all three isolations. The glue
/// supplies the final view and a non-empty command (`$SHELL`
/// substituted for an empty one, as `_cmd_run`/`_cmd_box` do).
pub(crate) fn run_print(
    view: &crate::sandbox_state::SlotView,
    settings: &crate::sandbox_state::SandboxSettings,
    command: &[String],
    cwd: Option<&Path>,
    facts: &RunPrintFacts,
    command_name: &str,
) -> (i32, String, String) {
    use crate::sandbox_state::{build_spec, expand_home, slice_limit_properties, slice_name};
    if !crate::sandbox_state::ISOLATIONS.contains(&settings.isolation.as_str()) {
        return refusal_for(
            &format!(
                "isolation must be one of {}",
                crate::sandbox_state::ISOLATIONS.join(", ")
            ),
            command_name,
        );
    }
    if command.is_empty() {
        return refusal_for("no command to run", command_name);
    }
    if !facts.systemd_run_available {
        return refusal_for(
            "systemd-run is not available; the sandbox needs a systemd user manager",
            command_name,
        );
    }
    let kind = if view.coordinator { "box" } else { "run" };
    let unit = scope_unit_name(kind, &view.slot, facts.pid);
    let slot_slice = slice_name(
        &view.slot_type,
        &view.slot,
        None,
        facts.cgroup_text.as_deref(),
    );
    let workdir: &Path = cwd.unwrap_or(&view.slot_path);
    let spec = match build_spec(
        view,
        settings,
        &facts.home,
        workdir,
        true,
        "inherit",
        &facts.spec,
    ) {
        Ok(spec) => spec,
        Err(message) => return refusal_for(&message, command_name),
    };
    let mut out = format!(
        "# isolation {}; scope {unit} in {slot_slice}\n",
        settings.isolation
    );
    for line in slice_limit_properties(&settings.limits, None) {
        out.push_str(&format!("# slice {slot_slice}: {line}\n"));
    }
    let home_text = facts.home.to_string_lossy();
    for (key, value) in &settings.env {
        out.push_str(&format!("# env {key}={}\n", expand_home(value, &home_text)));
    }
    match settings.isolation.as_str() {
        "userns" => {
            let spec_json = crate::sandbox_state::py_json_compact_sorted(&spec);
            match userns_helper_argv(
                &facts.helper_interpreter,
                &facts.helper_module,
                &spec_json,
                command,
            ) {
                Ok(argv) => {
                    let quoted: Vec<String> = argv.iter().map(|part| shlex_quote(part)).collect();
                    out.push_str(&quoted.join(" \\\n  "));
                    out.push('\n');
                }
                Err(message) => return refusal_for(&message, command_name),
            }
        }
        "root" => {
            let argv = root_helper_argv(
                &facts.helper_interpreter,
                &facts.helper_module,
                "SPEC-FILE",
                facts.dont_write_bytecode,
            );
            let quoted: Vec<String> = argv.iter().map(|part| shlex_quote(part)).collect();
            out.push_str(&quoted.join(" "));
            out.push('\n');
            let payload = json!({"view": spec, "command": command});
            out.push_str(&crate::sandbox_state::py_json_indented_sorted(&payload));
            out.push('\n');
        }
        _ => {
            let quoted: Vec<String> = command.iter().map(|part| shlex_quote(part)).collect();
            out.push_str(&quoted.join(" "));
            out.push('\n');
        }
    }
    (0, out, String::new())
}

/// `wrkslots run --place-only` (#1304): apply the slot's slice
/// limits through the runner and print the placement, running
/// nothing. Mirrors Python `_cmd_run`'s place-only branch and
/// `_place_only`: placement is derived once from the injected
/// cgroup text and limits are applied to exactly those names.
pub(crate) fn run_place_only(
    runner: &impl PrivilegedRunner,
    slot_type: &str,
    slot: &str,
    limits: &SandboxLimits,
    cgroup_text: Option<&str>,
    format_json: bool,
    given: &PlaceOnlyGiven,
) -> (i32, String, String) {
    for (option, is_given) in [
        ("--in-scope", given.in_scope),
        ("--expect-slices", given.expect_slices),
        ("--expect-writable", given.expect_writable),
        ("--git-config-policy", given.git_config_policy),
        ("--expect-masks", given.expect_masks),
        ("--closed-set", given.closed_set),
    ] {
        if is_given {
            return run_refusal(&format!("--place-only runs nothing and takes no {option}"));
        }
    }
    for (option, is_given) in [
        ("COMMAND", given.command),
        ("--print", given.print_only),
        ("--cwd", given.cwd),
    ] {
        if is_given {
            return run_refusal(&format!("--place-only runs nothing and takes no {option}"));
        }
    }
    let place = placement(slot_type, slot, cgroup_text);
    let parent = place.enclosing.as_deref().unwrap_or("");
    let plan = slice_application_plan(limits, slot_type, slot, Some(parent), None);
    let applied = match run_slice_plan(runner, &plan) {
        Ok(applied) => applied,
        Err(message) => return run_refusal(&message),
    };
    let mut stderr = String::new();
    for line in &applied {
        stderr.push_str(&format!("wrkslots run: limits {line}\n"));
    }
    let stdout = if format_json {
        let enclosing = match &place.enclosing {
            Some(name) => serde_json::to_string(name).unwrap_or_default(),
            None => "null".to_string(),
        };
        format!(
            "{{\"all_slots\": {}, \"enclosing\": {}, \"slot\": {}}}\n",
            serde_json::to_string(&place.all_slots).unwrap_or_default(),
            enclosing,
            serde_json::to_string(&place.slot).unwrap_or_default(),
        )
    } else {
        format!(
            "enclosing: {}\nall_slots: {}\nslot: {}\n",
            place.enclosing.as_deref().unwrap_or("-"),
            place.all_slots,
            place.slot,
        )
    };
    (0, stdout, stderr)
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

/// Python `!r` for the short name strings the in-scope
/// refusals quote.
fn py_repr(value: &str) -> String {
    if value.contains('\'') && !value.contains('"') {
        return format!("\"{}\"", value.replace('\\', "\\\\"));
    }
    let mut out = String::from("\'");
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\\'"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch => out.push(ch),
        }
    }
    out.push('\'');
    out
}

fn slice_chain(slice_unit: &str) -> Vec<String> {
    let base = slice_unit.strip_suffix(".slice").unwrap_or(slice_unit);
    let parts: Vec<&str> = base.split('-').collect();
    (0..parts.len())
        .map(|index| format!("{}.slice", parts[..=index].join("-")))
        .collect()
}

/// Refuse unless this process is in `unit` under exactly the
/// expected slices, mirroring Python `verify_in_scope`. The
/// cgroup file is read at the given path (fixture in tests).
pub(crate) fn verify_in_scope(
    view: &crate::sandbox_state::SlotView,
    unit: &str,
    expected: &SlicePlacement,
    cgroup_path: &Path,
) -> Result<(), String> {
    if !unit.ends_with(".scope") || unit.contains('/') || unit == ".scope" {
        return Err(format!(
            "--in-scope needs a scope unit name such as run-1.scope, not {}",
            py_repr(unit)
        ));
    }
    if let Some(enclosing) = &expected.enclosing {
        if !enclosing.ends_with(".slice") || enclosing.contains('/') {
            return Err(format!(
                "the expected enclosing slice {} is not a slice name",
                py_repr(enclosing)
            ));
        }
    }
    let parent = expected.enclosing.as_deref().unwrap_or("");
    let derived = (
        root_slice(Some(parent), None),
        slice_name(&view.slot_type, &view.slot, Some(parent), None),
    );
    if (expected.all_slots.as_str(), expected.slot.as_str())
        != (derived.0.as_str(), derived.1.as_str())
    {
        return Err(format!(
            "the expected slices {}, {} are not slot {}'s under {} ({}, {})",
            expected.all_slots,
            expected.slot,
            view.slot,
            expected
                .enclosing
                .as_deref()
                .unwrap_or("no enclosing slice"),
            derived.0,
            derived.1,
        ));
    }
    let text = std::fs::read_to_string(cgroup_path)
        .map_err(|error| format!("cannot read {}: {error}", cgroup_path.display()))?;
    let path = text
        .lines()
        .find(|line| line.starts_with("0::"))
        .map(|line| &line[3..]);
    let Some(path) = path else {
        return Err(format!(
            "{} has no unified (0::) cgroup line",
            cgroup_path.display()
        ));
    };
    let parts: Vec<&str> = path.split('/').collect();
    let manager = parts.iter().position(|part| part.starts_with("user@"));
    let below: Vec<&str> = match manager {
        Some(index) => parts[index + 1..].to_vec(),
        None => Vec::new(),
    };
    let mut wanted = slice_chain(&expected.slot);
    wanted.push(unit.to_string());
    let below: Vec<String> = below.iter().map(|part| part.to_string()).collect();
    if manager.is_none() || below != wanted {
        return Err(format!(
            "this process is in {path}, not in {unit} directly under {} ({} below the user manager); refusing to enter the box",
            expected.slot,
            wanted.join("/"),
        ));
    }
    Ok(())
}

/// Inputs for `run --in-scope ... --print`, as the glue parsed
/// them (texts unparsed: their refusals are the handler's).
pub(crate) struct InScopeGiven {
    pub(crate) unit: Option<String>,
    pub(crate) expect_slices: Option<String>,
    pub(crate) expect_writable: Option<String>,
    pub(crate) expect_masks: Option<String>,
    pub(crate) git_config_policy: String,
    pub(crate) closed_set: bool,
    pub(crate) no_protect_system: bool,
}

/// Host facts for the in-scope plan.
pub(crate) struct InScopeFacts {
    pub(crate) home: PathBuf,
    pub(crate) cgroup_path: PathBuf,
    pub(crate) spec: crate::sandbox_state::SpecFacts,
    pub(crate) helper_interpreter: String,
    pub(crate) helper_module: String,
    pub(crate) dont_write_bytecode: bool,
}

fn expected_slices(text: &str) -> Result<SlicePlacement, String> {
    let loaded: Value = serde_json::from_str(text).map_err(|error| {
        format!("--expect-slices must be the JSON object --place-only printed: {error}")
    })?;
    let Some(map) = loaded.as_object() else {
        return Err(
            "--expect-slices must be an object with exactly enclosing, all_slots and slot"
                .to_string(),
        );
    };
    if map.len() != 3
        || !map.contains_key("enclosing")
        || !map.contains_key("all_slots")
        || !map.contains_key("slot")
    {
        return Err(
            "--expect-slices must be an object with exactly enclosing, all_slots and slot"
                .to_string(),
        );
    }
    let enclosing = match &map["enclosing"] {
        Value::Null => None,
        Value::String(name) if !name.is_empty() => Some(name.clone()),
        _ => return Err("--expect-slices enclosing must be a slice name or null".to_string()),
    };
    let (Some(all_slots), Some(slot)) = (
        map["all_slots"].as_str().filter(|name| !name.is_empty()),
        map["slot"].as_str().filter(|name| !name.is_empty()),
    ) else {
        return Err("--expect-slices all_slots and slot must be slice names".to_string());
    };
    Ok(SlicePlacement {
        enclosing,
        all_slots: all_slots.to_string(),
        slot: slot.to_string(),
    })
}

fn expected_object(text: &str, what: &str) -> Result<Value, String> {
    let loaded: Value = serde_json::from_str(text).map_err(|error| {
        format!("--expect-{what} must be the JSON object --print reported as {what}: {error}")
    })?;
    if !loaded.is_object() {
        return Err(format!(
            "--expect-{what} must be the JSON object --print reported as {what}"
        ));
    }
    Ok(loaded)
}

/// `wrkslots run --in-scope UNIT --expect-slices ... --print`:
/// verify the caller's placement and print what entering the
/// box would do, mirroring Python `_in_scope`'s print branch
/// and `sandbox.in_scope_plan`. Nothing is entered or written.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_in_scope_print(
    view: &crate::sandbox_state::SlotView,
    settings: &crate::sandbox_state::SandboxSettings,
    command: &[String],
    cwd: Option<&Path>,
    given: &InScopeGiven,
    facts: &InScopeFacts,
) -> (i32, String, String) {
    use crate::sandbox_state::{build_spec, mask_report, planned_private_sources, writable_set};
    if given.unit.is_none() || given.expect_slices.is_none() {
        return refusal_for("--in-scope and --expect-slices are given together", "run");
    }
    if given.closed_set && given.no_protect_system {
        return refusal_for(
            "--closed-set refuses --no-protect-system: every other mount must stay read-only",
            "run",
        );
    }
    let unit = given.unit.as_deref().unwrap_or("");
    let expected = match expected_slices(given.expect_slices.as_deref().unwrap_or("")) {
        Ok(expected) => expected,
        Err(message) => return refusal_for(&message, "run"),
    };
    let expected_writable = match &given.expect_writable {
        Some(text) => match expected_object(text, "writable") {
            Ok(value) => Some(value),
            Err(message) => return refusal_for(&message, "run"),
        },
        None => None,
    };
    let expected_masks = match &given.expect_masks {
        Some(text) => match expected_object(text, "masks") {
            Ok(value) => Some(value),
            Err(message) => return refusal_for(&message, "run"),
        },
        None => None,
    };
    if settings.isolation != "root" {
        return refusal_for(
            &format!(
                "--in-scope builds the view through the root helper and needs isolation root, not {}",
                settings.isolation
            ),
            "run",
        );
    }
    if view.coordinator {
        return refusal_for(
            "--in-scope enters one slot's box, not a coordinator box",
            "run",
        );
    }
    if command.is_empty() {
        return refusal_for("no command to run", "run");
    }
    if let Err(message) = verify_in_scope(view, unit, &expected, &facts.cgroup_path) {
        return refusal_for(&message, "run");
    }
    let workdir: &Path = cwd.unwrap_or(&view.slot_path);
    let spec = match build_spec(
        view,
        settings,
        &facts.home,
        workdir,
        !given.closed_set,
        if given.git_config_policy.is_empty() {
            "inherit"
        } else {
            &given.git_config_policy
        },
        &facts.spec,
    ) {
        Ok(spec) => spec,
        Err(message) => return refusal_for(&message, "run"),
    };
    let writable = writable_set(&spec, &planned_private_sources(view, settings));
    if let Some(want) = &expected_writable {
        if writable != *want {
            return refusal_for(
                &format!(
                    "the writable set this entry would have differs from the expected one: {} is not {}; refusing to enter the box",
                    crate::sandbox_state::py_json_compact_sorted(&writable),
                    crate::sandbox_state::py_json_compact_sorted(want),
                ),
                "run",
            );
        }
    }
    let masks = mask_report(&spec);
    if let Some(want) = &expected_masks {
        if masks != *want {
            return refusal_for(
                &format!(
                    "the masks this entry would apply differ from the expected ones: {} is not {}; refusing to enter the box",
                    crate::sandbox_state::py_json_compact_sorted(&masks),
                    crate::sandbox_state::py_json_compact_sorted(want),
                ),
                "run",
            );
        }
    }
    let plan = json!({
        "unit": unit,
        "slices": expected.to_obj(),
        "writable": writable,
        "masks": masks,
        "helper": root_helper_argv(
            &facts.helper_interpreter,
            &facts.helper_module,
            "SPEC-FILE",
            facts.dont_write_bytecode,
        ),
        "command": command,
        "view": spec,
    });
    (
        0,
        format!("{}\n", crate::sandbox_state::py_json_indented_sorted(&plan)),
        String::new(),
    )
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

    const PLACE_ONLY_DRIVER: &str = r####"import argparse, io, os, subprocess, sys
from contextlib import redirect_stdout, redirect_stderr
from pathlib import Path
sys.path.insert(0, os.environ["WRKSLOTS_PY_ROOT"])
from wrkslots import cli, imagecmd, sandbox

case = sys.argv[1]
real_run = subprocess.run
def fake_run(argv, **kwargs):
    if argv[0] == "systemctl":
        if case == "failing" and "set-property" in argv:
            return subprocess.CompletedProcess(argv, 1, "", "denied\n")
        return subprocess.CompletedProcess(argv, 0, "", "")
    return real_run(argv, **kwargs)
subprocess.run = fake_run
sandbox.parent_slice = lambda cgroup_file=None: ("s.slice" if case.startswith("enclosing") else None)
view = sandbox.SlotView(slot="slot-a", slot_type="agent", slot_path=Path("/slots/slot-a"), state_directory=Path("/state/slot-a"), git_directories=(), control_directory=Path("/control"), representation="worktree")
limits_obj = {} if case == "default-limits" else {"memory_max": "32G", "all_slots_cpu_quota": "1600%"}
settings = sandbox.SandboxSettings(limits=sandbox.limits_from_obj(limits_obj))
args = argparse.Namespace(command=["/bin/true"] if case == "with-command" else [], print_only=(case == "with-print"), cwd=("/tmp" if case == "with-cwd" else None))
fmt = "json" if case.endswith("json") else "human"
out, err = io.StringIO(), io.StringIO()
try:
    with redirect_stdout(out), redirect_stderr(err):
        rc = imagecmd._place_only(args, view, settings, None, fmt)
except sandbox.SandboxError as exc:
    exc = cli.Refusal(str(exc))
    remedy = cli._refusal_remedy(argparse.Namespace(command="run"), exc)
    sys.stdout.write(f"3\n{out.getvalue()}---STDERR---\nREFUSED: {exc}\nREMEDY: {remedy}\n")
    sys.exit(0)
except cli.Refusal as exc:
    remedy = cli._refusal_remedy(argparse.Namespace(command="run"), exc)
    sys.stdout.write(f"3\n{out.getvalue()}---STDERR---\nREFUSED: {exc}\nREMEDY: {remedy}\n")
    sys.exit(0)
sys.stdout.write(f"{rc}\n{out.getvalue()}---STDERR---\n{err.getvalue()}")
"####;

    /// Oracle differential (#1304): Python `_place_only` with a
    /// stubbed parent slice and systemctl vs `run_place_only`
    /// through a recording fake, byte for byte.
    #[test]
    fn place_only_matches_python_oracle() {
        struct Fake {
            fail_set: bool,
        }
        impl PrivilegedRunner for Fake {
            fn run(&self, spec: &CommandSpec) -> Result<RunOutcome, String> {
                let failing = self.fail_set && spec.argv.iter().any(|a| a == "set-property");
                Ok(RunOutcome {
                    returncode: if failing { 1 } else { 0 },
                    stdout: String::new(),
                    stderr: if failing {
                        "denied\n".to_string()
                    } else {
                        String::new()
                    },
                })
            }
        }
        let repo = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("repo root")
            .to_path_buf();
        let dir = std::env::temp_dir().join(format!("wrkslotsd-place-only-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let driver_path = dir.join("place_only_driver.py");
        std::fs::write(&driver_path, PLACE_ONLY_DRIVER).expect("driver");
        let no_enclosing = "0::/user.slice/user@1000.service/app.scope\n";
        let enclosing = "0::/user.slice/user@1000.service/s.slice/app.scope\n";
        let explicit = limits_from_obj(&json!({
            "memory_max": "32G",
            "all_slots_cpu_quota": "1600%"
        }))
        .expect("limits");
        let defaults = limits_from_obj(&json!({})).expect("default limits");
        let clean = PlaceOnlyGiven::default();
        for (case, cgroup, format_json, given, use_defaults, fail_set) in [
            ("plain", no_enclosing, false, clean, false, false),
            ("plain-json", no_enclosing, true, clean, false, false),
            ("enclosing", enclosing, false, clean, false, false),
            ("enclosing-json", enclosing, true, clean, false, false),
            ("default-limits", no_enclosing, false, clean, true, false),
            (
                "with-command",
                no_enclosing,
                false,
                PlaceOnlyGiven {
                    command: true,
                    ..clean
                },
                false,
                false,
            ),
            (
                "with-print",
                no_enclosing,
                false,
                PlaceOnlyGiven {
                    print_only: true,
                    ..clean
                },
                false,
                false,
            ),
            (
                "with-cwd",
                no_enclosing,
                false,
                PlaceOnlyGiven { cwd: true, ..clean },
                false,
                false,
            ),
            ("failing", no_enclosing, false, clean, false, true),
        ] {
            let output = std::process::Command::new("python3")
                .arg(&driver_path)
                .arg(case)
                .env("WRKSLOTS_PY_ROOT", repo.join("py"))
                .output()
                .expect("python oracle runs");
            assert!(output.status.success(), "case {case}: {:?}", output.stderr);
            let text = String::from_utf8(output.stdout).expect("utf8");
            let (head, rest) = text.split_once('\n').expect("exit line");
            let (stdout, stderr) = rest.split_once("---STDERR---\n").expect("marker");
            let outcome = run_place_only(
                &Fake { fail_set },
                "agent",
                "slot-a",
                if use_defaults { &defaults } else { &explicit },
                Some(cgroup),
                format_json,
                &given,
            );
            assert_eq!(
                outcome,
                (
                    head.trim().parse::<i32>().expect("exit"),
                    stdout.to_string(),
                    stderr.to_string()
                ),
                "case {case}"
            );
        }
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    const RUN_PRINT_DRIVER: &str = r####"import argparse, io, os, shutil, sys
from contextlib import redirect_stdout, redirect_stderr
from pathlib import Path
sys.path.insert(0, os.environ["WRKSLOTS_PY_ROOT"])
from wrkslots import cli, sandbox

case, base = sys.argv[1], Path(sys.argv[2])
os.environ.pop("GATE_ADMISSION_DIR", None)
os.environ.pop("XDG_RUNTIME_DIR", None)
os.environ["USER"] = "fixture-user"
home = base / "home"
os.environ["HOME"] = str(home)
sandbox.parent_slice = lambda cgroup_file=None: None
sandbox._interpreter = lambda: "/usr/bin/python3"
os.getpid = lambda: 4242
shutil.which = lambda name: (None if case == "no-systemd" else "/usr/bin/systemd-run")
isolation = {"cgroup": "cgroup", "userns": "userns", "root": "root"}.get(case, "cgroup")
coordinator = case == "box"
settings = sandbox.settings_from_obj({"isolation": isolation, "env": {"TASK": "~/work"}, "limits": {"memory_max": "32G"}})
view = sandbox.SlotView(
    slot="box-a" if coordinator else "slot-a",
    slot_type="box" if coordinator else "agent",
    slot_path=base / "project" if coordinator else base / "slot-a",
    state_directory=base / "state" / "slot-a",
    git_directories=() if coordinator else (base / "gitcommon",),
    control_directory=base / "control",
    representation="worktrees",
    project_root=base / "project",
    coordinator=coordinator,
    writable=(base / "project",) if coordinator else (),
    propagate=(),
)
command = [] if case == "no-command" else ["/bin/echo", "hello world", "it's"]
out, err = io.StringIO(), io.StringIO()
try:
    with redirect_stdout(out), redirect_stderr(err):
        rc = sandbox.run(view, settings, command, print_only=True)
except sandbox.SandboxError as exc:
    exc = cli.Refusal(str(exc))
    remedy = cli._refusal_remedy(argparse.Namespace(command="box" if coordinator else "run"), exc)
    sys.stdout.write(f"3\n{out.getvalue()}---STDERR---\nREFUSED: {exc}\nREMEDY: {remedy}\n")
    sys.exit(0)
sys.stdout.write(f"{rc}\n{out.getvalue()}---STDERR---\n{err.getvalue()}")
"####;

    /// Oracle differential (#1317): Python `sandbox.run`
    /// `print_only` vs `run_print` on the same fixture, byte for
    /// byte for every isolation.
    #[test]
    fn run_print_matches_python_oracle() {
        use crate::sandbox_state::{settings_from_obj, SlotView, SpecFacts};
        let repo = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("repo root")
            .to_path_buf();
        let root = std::env::temp_dir().join(format!("wrkslotsd-run-print-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        for dir in [
            home.join(".claude"),
            home.join(".cache"),
            root.join("slot-a"),
            root.join("state/slot-a"),
            root.join("gitcommon"),
            root.join("control"),
            root.join("project/ai_docs"),
        ] {
            std::fs::create_dir_all(&dir).expect("fixture dir");
        }
        std::fs::write(home.join("plain.txt"), "x").expect("file");
        let driver_path = root.join("run_print_driver.py");
        std::fs::write(&driver_path, RUN_PRINT_DRIVER).expect("driver");
        let uid = {
            use std::os::unix::fs::MetadataExt;
            std::fs::metadata(&root).expect("meta").uid()
        };
        let module = repo
            .join("py/wrkslots/sandbox.py")
            .canonicalize()
            .expect("module path");
        for (case, isolation, coordinator) in [
            ("cgroup", "cgroup", false),
            ("userns", "userns", false),
            ("root", "root", false),
            ("box", "cgroup", true),
            ("no-command", "cgroup", false),
            ("no-systemd", "cgroup", false),
        ] {
            let output = std::process::Command::new("python3")
                .arg(&driver_path)
                .arg(case)
                .arg(&root)
                .env("WRKSLOTS_PY_ROOT", repo.join("py"))
                .output()
                .expect("python oracle runs");
            assert!(output.status.success(), "case {case}: {:?}", output.stderr);
            let text = String::from_utf8(output.stdout).expect("utf8");
            let (head, rest) = text.split_once('\n').expect("exit line");
            let (stdout, stderr) = rest.split_once("---STDERR---\n").expect("marker");
            let view = SlotView {
                slot: if coordinator { "box-a" } else { "slot-a" }.to_string(),
                slot_type: if coordinator { "box" } else { "agent" }.to_string(),
                slot_path: if coordinator {
                    root.join("project")
                } else {
                    root.join("slot-a")
                },
                state_directory: root.join("state/slot-a"),
                git_directories: if coordinator {
                    Vec::new()
                } else {
                    vec![root.join("gitcommon")]
                },
                control_directory: root.join("control"),
                representation: "worktrees".to_string(),
                project_root: Some(root.join("project")),
                coordinator,
                writable: if coordinator {
                    vec![root.join("project")]
                } else {
                    Vec::new()
                },
                propagate: Vec::new(),
            };
            let settings = settings_from_obj(&json!({
                "isolation": isolation,
                "env": {"TASK": "~/work"},
                "limits": {"memory_max": "32G"}
            }))
            .expect("settings");
            let command: Vec<String> = if case == "no-command" {
                Vec::new()
            } else {
                vec![
                    "/bin/echo".to_string(),
                    "hello world".to_string(),
                    "it's".to_string(),
                ]
            };
            let facts = RunPrintFacts {
                home: home.clone(),
                pid: 4242,
                cgroup_text: Some("0::/user.slice/user@1000.service/app.scope\n".to_string()),
                systemd_run_available: case != "no-systemd",
                spec: SpecFacts {
                    user: "fixture-user".to_string(),
                    uid,
                    mountinfo: None,
                    gate_dir: None,
                    xdg_runtime_dir: None,
                },
                helper_interpreter: "/usr/bin/python3".to_string(),
                helper_module: module.to_string_lossy().into_owned(),
                dont_write_bytecode: false,
            };
            let outcome = run_print(
                &view,
                &settings,
                &command,
                None,
                &facts,
                if coordinator { "box" } else { "run" },
            );
            assert_eq!(
                outcome,
                (
                    head.trim().parse::<i32>().expect("exit"),
                    stdout.to_string(),
                    stderr.to_string()
                ),
                "case {case}"
            );
        }
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    const IN_SCOPE_DRIVER: &str = r####"import argparse, io, json, os, sys
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
sys.path.insert(0, os.environ["WRKSLOTS_PY_ROOT"])
from wrkslots import cli, imagecmd, sandbox

case, base = sys.argv[1], Path(sys.argv[2])
os.environ.pop("GATE_ADMISSION_DIR", None)
os.environ.pop("XDG_RUNTIME_DIR", None)
os.environ["USER"] = "fixture-user"
home = base / "home"
os.environ["HOME"] = str(home)
sandbox._interpreter = lambda: "/usr/bin/python3"
SLOT = "wrkslots-slot\\x2da.slice"
SLICES = json.dumps({"enclosing": None, "all_slots": "wrkslots.slice", "slot": SLOT})
cgroup = base / "cgroup"
cgroup.write_text("0::/user.slice/user@1000.service/wrkslots.slice/" + SLOT + "/run-1.scope\n" if case != "wrong-process" else "0::/user.slice/user@1000.service/app.slice/run-9.scope\n")
view = sandbox.SlotView(slot="slot-a", slot_type="agent", slot_path=base / "slot-a", state_directory=base / "state" / "slot-a", git_directories=(base / "gitcommon",), control_directory=base / "control", representation="worktrees", project_root=base / "project", coordinator=False, writable=(), propagate=())
settings = sandbox.settings_from_obj({"isolation": "cgroup" if case == "wrong-isolation" else "root", "limits": {"memory_max": "32G"}})
expected = sandbox.SlicePlacement(enclosing=None, all_slots="wrkslots.slice", slot=SLOT)
out, err = io.StringIO(), io.StringIO()
def render(rc):
    return f"{rc}\n{out.getvalue()}---STDERR---\n{err.getvalue()}"
try:
    with redirect_stdout(out), redirect_stderr(err):
        if case in ("missing-expect", "closed-noprotect"):
            args = argparse.Namespace(no_protect_system=(case == "closed-noprotect"), expect_writable=None, expect_masks=None, git_config_policy=None, command=["/bin/true"], cwd=None, print_only=True)
            rc = imagecmd._in_scope(args, view, settings, "run-1.scope", None if case == "missing-expect" else SLICES, case == "closed-noprotect", None)
        else:
            plan = sandbox.in_scope_plan(view, settings, ["/bin/true"], unit="run-1.scope", expected=expected, cgroup_file=cgroup)
            if case == "expect-ok":
                plan = sandbox.in_scope_plan(view, settings, ["/bin/true"], unit="run-1.scope", expected=expected, cgroup_file=cgroup, expected_writable=plan["writable"], expected_masks=plan["masks"])
            if case == "expect-wrong":
                bad = dict(plan["writable"]); bad["tmp"] = {"target": "/tmp", "size": "1G"}
                plan = sandbox.in_scope_plan(view, settings, ["/bin/true"], unit="run-1.scope", expected=expected, cgroup_file=cgroup, expected_writable=bad)
            print(json.dumps(plan, indent=2, sort_keys=True))
            rc = 0
except (cli.Refusal, sandbox.SandboxError) as exc:
    if not isinstance(exc, cli.Refusal):
        exc = cli.Refusal(str(exc))
    remedy = cli._refusal_remedy(argparse.Namespace(command="run"), exc)
    sys.stdout.write(f"3\n{out.getvalue()}---STDERR---\nREFUSED: {exc}\nREMEDY: {remedy}\n")
    sys.exit(0)
sys.stdout.write(render(rc))
"####;

    /// Oracle differential: Python `in_scope_plan` (and
    /// `_in_scope` for its early refusals) vs
    /// `run_in_scope_print` on the same fixture.
    #[test]
    fn in_scope_print_matches_python_oracle() {
        use crate::sandbox_state::{settings_from_obj, SlotView, SpecFacts};
        let repo = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("repo root")
            .to_path_buf();
        let root = std::env::temp_dir().join(format!("wrkslotsd-in-scope-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        for dir in [
            home.join(".claude"),
            home.join(".cache"),
            root.join("slot-a"),
            root.join("state/slot-a"),
            root.join("gitcommon"),
            root.join("control"),
            root.join("project/ai_docs"),
        ] {
            std::fs::create_dir_all(&dir).expect("fixture dir");
        }
        std::fs::write(home.join("plain.txt"), "x").expect("file");
        let driver_path = root.join("in_scope_driver.py");
        std::fs::write(&driver_path, IN_SCOPE_DRIVER).expect("driver");
        let cgroup_path = root.join("cgroup");
        let slot_slice = "wrkslots-slot\\x2da.slice";
        let slices_text = json!({
            "enclosing": Value::Null,
            "all_slots": "wrkslots.slice",
            "slot": slot_slice,
        })
        .to_string();
        let uid = {
            use std::os::unix::fs::MetadataExt;
            std::fs::metadata(&root).expect("meta").uid()
        };
        let module = repo
            .join("py/wrkslots/sandbox.py")
            .canonicalize()
            .expect("module path");
        for case in [
            "plan",
            "expect-ok",
            "expect-wrong",
            "wrong-process",
            "wrong-isolation",
            "missing-expect",
            "closed-noprotect",
        ] {
            let cgroup_text = if case == "wrong-process" {
                "0::/user.slice/user@1000.service/app.slice/run-9.scope\n"
            } else {
                "0::/user.slice/user@1000.service/wrkslots.slice/wrkslots-slot\\x2da.slice/run-1.scope\n"
            };
            // The Python driver writes the same text itself.
            std::fs::write(&cgroup_path, cgroup_text).expect("cgroup");
            let output = std::process::Command::new("python3")
                .arg(&driver_path)
                .arg(case)
                .arg(&root)
                .env("WRKSLOTS_PY_ROOT", repo.join("py"))
                .output()
                .expect("python oracle runs");
            assert!(output.status.success(), "case {case}: {:?}", output.stderr);
            let text = String::from_utf8(output.stdout).expect("utf8");
            let (head, rest) = text.split_once('\n').expect("exit line");
            let (stdout, stderr) = rest.split_once("---STDERR---\n").expect("marker");
            let view = SlotView {
                slot: "slot-a".to_string(),
                slot_type: "agent".to_string(),
                slot_path: root.join("slot-a"),
                state_directory: root.join("state/slot-a"),
                git_directories: vec![root.join("gitcommon")],
                control_directory: root.join("control"),
                representation: "worktrees".to_string(),
                project_root: Some(root.join("project")),
                coordinator: false,
                writable: Vec::new(),
                propagate: Vec::new(),
            };
            let settings = settings_from_obj(&json!({
                "isolation": if case == "wrong-isolation" { "cgroup" } else { "root" },
                "limits": {"memory_max": "32G"}
            }))
            .expect("settings");
            let facts = InScopeFacts {
                home: home.clone(),
                cgroup_path: cgroup_path.clone(),
                spec: SpecFacts {
                    user: "fixture-user".to_string(),
                    uid,
                    mountinfo: None,
                    gate_dir: None,
                    xdg_runtime_dir: None,
                },
                helper_interpreter: "/usr/bin/python3".to_string(),
                helper_module: module.to_string_lossy().into_owned(),
                dont_write_bytecode: false,
            };
            let clean = InScopeGiven {
                unit: Some("run-1.scope".to_string()),
                expect_slices: Some(slices_text.clone()),
                expect_writable: None,
                expect_masks: None,
                git_config_policy: "inherit".to_string(),
                closed_set: false,
                no_protect_system: false,
            };
            let outcome = match case {
                "missing-expect" => run_in_scope_print(
                    &view,
                    &settings,
                    &["/bin/true".to_string()],
                    None,
                    &InScopeGiven {
                        expect_slices: None,
                        ..clean
                    },
                    &facts,
                ),
                "closed-noprotect" => run_in_scope_print(
                    &view,
                    &settings,
                    &["/bin/true".to_string()],
                    None,
                    &InScopeGiven {
                        closed_set: true,
                        no_protect_system: true,
                        ..clean
                    },
                    &facts,
                ),
                "expect-ok" | "expect-wrong" => {
                    let first = run_in_scope_print(
                        &view,
                        &settings,
                        &["/bin/true".to_string()],
                        None,
                        &clean,
                        &facts,
                    );
                    assert_eq!(first.0, 0, "first pass {case}: {}", first.2);
                    let plan: Value = serde_json::from_str(first.1.trim_end()).expect("plan json");
                    let (writable, masks) = if case == "expect-ok" {
                        (plan["writable"].to_string(), plan["masks"].to_string())
                    } else {
                        let mut bad = plan["writable"].clone();
                        bad["tmp"] = json!({"target": "/tmp", "size": "1G"});
                        (bad.to_string(), plan["masks"].to_string())
                    };
                    run_in_scope_print(
                        &view,
                        &settings,
                        &["/bin/true".to_string()],
                        None,
                        &InScopeGiven {
                            expect_writable: Some(writable),
                            expect_masks: Some(masks),
                            ..clean
                        },
                        &facts,
                    )
                }
                _ => run_in_scope_print(
                    &view,
                    &settings,
                    &["/bin/true".to_string()],
                    None,
                    &clean,
                    &facts,
                ),
            };
            assert_eq!(
                outcome,
                (
                    head.trim().parse::<i32>().expect("exit"),
                    stdout.to_string(),
                    stderr.to_string()
                ),
                "case {case}"
            );
        }
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn place_only_refuses_cmd_run_option_combinations() {
        struct Fake;
        impl PrivilegedRunner for Fake {
            fn run(&self, _spec: &CommandSpec) -> Result<RunOutcome, String> {
                Ok(RunOutcome {
                    returncode: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                })
            }
        }
        let limits = limits_from_obj(&json!({})).expect("limits");
        for (given, option) in [
            (
                PlaceOnlyGiven {
                    in_scope: true,
                    ..Default::default()
                },
                "--in-scope",
            ),
            (
                PlaceOnlyGiven {
                    expect_slices: true,
                    ..Default::default()
                },
                "--expect-slices",
            ),
            (
                PlaceOnlyGiven {
                    closed_set: true,
                    ..Default::default()
                },
                "--closed-set",
            ),
        ] {
            let (exit, stdout, stderr) =
                run_place_only(&Fake, "agent", "slot-a", &limits, None, false, &given);
            assert_eq!(exit, 3);
            assert!(stdout.is_empty());
            assert!(
                stderr.contains(&format!("--place-only runs nothing and takes no {option}")),
                "{stderr}"
            );
        }
    }
}
