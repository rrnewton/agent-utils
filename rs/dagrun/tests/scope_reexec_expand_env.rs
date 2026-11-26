//! The outer-scope re-exec hands its own argv to `systemd-run` unexpanded (#2316).
//!
//! `systemd-run --scope` in newer systemd expands `${NAME}`, a whole-word `$NAME` and `$$` in the
//! command line it starts, from the caller's environment. The re-exec carries this process's argv
//! (for example an `--args` value, which is shell text for the steps), so it must pass
//! `--expand-environment=no` where systemd-run accepts it, and keep the old argv, with one note,
//! where it does not (systemd before 254, whose scopes never expanded).

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// One `--args` value with every form systemd-run rewrites.
const PAYLOAD: &str = "A:${PROBE} B:$$ C:${x%% *} D:$PROBE";

struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("dagrun_expand_env_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Fixture { dir }
    }

    /// A one-step DAG whose step writes its `{args}` text, single-quoted, to `out`.
    fn dag(&self, out: &Path) -> PathBuf {
        let dag = self.dir.join("args.json");
        let step = serde_json::json!({
            "group": "g",
            "job": "j",
            "cmd": format!("printf '%s' '{{args}}' > '{}'", out.display()),
            "timeout": 60,
            "cpu_timeout": 600,
        });
        std::fs::write(&dag, serde_json::json!({ "steps": [step] }).to_string()).unwrap();
        dag
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The built runner with the scope-policy and nesting knobs cleared, as a fresh top-level run.
fn runner() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_dagrun"));
    for key in [
        "CI",
        "GITHUB_ACTIONS",
        "DAGRUN_IN_SCOPE",
        "DAGRUN_SCOPE_UNIT",
        "DAGRUN_DIRECT_CGROUP",
        "DAGRUN_OUTER_RUN",
        "DAGRUN_DELEGATED_CGROUP",
        "DAGRUN_DELEGATED_UNBOXED",
        "DAGRUN_EXPECTED_OUTER_MEMORY_MAX_BYTES",
        "DAGRUN_EXPECTED_OUTER_CPU_COUNT",
        "DAGRUN_OUTER_MEMORY_MAX_BYTES",
    ] {
        cmd.env_remove(key);
    }
    cmd.env("DAGRUN_FORCE_SCOPE_ATTEMPT", "1")
        .env("DAGRUN_NO_STEP_LOGS", "1");
    cmd
}

/// Write executable `name` in `dir` running `body` under /bin/sh.
fn write_shim(dir: &Path, name: &str, body: &str) {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A recording `systemd-run`: each call appends its arguments, one per line, after a `CALL`
/// line. The option probe (`--expand-environment=no --version`) exits `probe_exit`; every other
/// call succeeds, so the scope probe passes and the re-exec ends the run right here.
fn recorder(fx: &Fixture, probe_exit: i32) -> (String, PathBuf) {
    let shim = fx.dir.join("bin");
    std::fs::create_dir_all(&shim).unwrap();
    let log = fx.dir.join("calls.txt");
    write_shim(
        &shim,
        "systemd-run",
        &format!(
            "echo CALL >> '{log}'\nfor a in \"$@\"; do printf '%s\\n' \"$a\" >> '{log}'; done\n\
             if [ \"$1\" = --expand-environment=no ] && [ \"$2\" = --version ]; then exit {probe_exit}; fi\n\
             exit 0",
            log = log.display()
        ),
    );
    // A refusing systemctl keeps the aggregate-slice setup off the real user session.
    write_shim(&shim, "systemctl", "exit 1");
    let path = format!(
        "{}:{}",
        shim.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    (path, log)
}

/// The recorded calls, each as its argument list.
fn calls(log: &Path) -> Vec<Vec<String>> {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let mut out: Vec<Vec<String>> = Vec::new();
    for line in text.lines() {
        if line == "CALL" {
            out.push(Vec::new());
        } else if let Some(last) = out.last_mut() {
            last.push(line.to_string());
        }
    }
    out
}

/// The re-exec call: the one that carries `run` after the separator.
fn reexec_call(calls: &[Vec<String>]) -> Vec<String> {
    calls
        .iter()
        .find(|call| {
            call.iter()
                .position(|arg| arg == "--")
                .is_some_and(|i| call[i + 1..].iter().any(|arg| arg == "run"))
        })
        .cloned()
        .unwrap_or_default()
}

#[test]
fn reexec_turns_expansion_off_when_systemd_run_accepts_it() {
    let fx = Fixture::new("supported");
    let (path, log) = recorder(&fx, 0);
    let dag = fx.dag(&fx.dir.join("out.txt"));
    let out = runner()
        .args(["run", "--dag", dag.to_str().unwrap(), "--no-profile", "-q"])
        .args(["--args", PAYLOAD])
        .env("PATH", &path)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    let calls = calls(&log);
    let reexec = reexec_call(&calls);
    let separator = reexec.iter().position(|arg| arg == "--");
    assert!(
        separator.is_some(),
        "no re-exec recorded:\n{calls:?}\n{stderr}"
    );
    let separator = separator.unwrap();
    assert_eq!(
        reexec[separator - 1],
        "--expand-environment=no",
        "the flag must sit right before the separator:\n{reexec:?}"
    );
    let value = reexec
        .iter()
        .position(|arg| arg == "--args")
        .map(|i| reexec[i + 1].as_str());
    assert_eq!(
        value,
        Some(PAYLOAD),
        "the --args text must reach systemd-run unchanged:\n{reexec:?}"
    );
    // Asked once: exactly one option probe.
    let probes = calls
        .iter()
        .filter(|call| call.first().map(String::as_str) == Some("--expand-environment=no"))
        .count();
    assert_eq!(probes, 1, "{calls:?}");
    assert!(
        !stderr.contains("#2316"),
        "no note when supported:\n{stderr}"
    );
}

#[test]
fn reexec_keeps_the_old_argv_with_a_note_when_systemd_run_refuses_the_option() {
    let fx = Fixture::new("unsupported");
    let (path, log) = recorder(&fx, 1);
    let dag = fx.dag(&fx.dir.join("out.txt"));
    let out = runner()
        .args(["run", "--dag", dag.to_str().unwrap(), "--no-profile", "-q"])
        .args(["--args", PAYLOAD])
        .env("PATH", &path)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    let reexec = reexec_call(&calls(&log));
    assert!(
        reexec.iter().any(|arg| arg == "--"),
        "no re-exec recorded:\n{stderr}"
    );
    assert!(
        !reexec
            .iter()
            .any(|arg| arg.starts_with("--expand-environment")),
        "an old systemd-run must get the old argv:\n{reexec:?}"
    );
    assert!(
        stderr.contains("refuses --expand-environment=no") && stderr.contains("#2316"),
        "the fallback must be said once:\n{stderr}"
    );
}

/// Live: through a real `systemd-run --user --scope`, the step sees the `--args` text byte for
/// byte although the caller has `PROBE` set. Skipped without a usable user scope, and below an
/// outer dagrun run, where a fresh top-level scope would escape the validation's cgroup.
#[test]
fn live_reexec_delivers_dollar_text_byte_for_byte() {
    let outer = [
        "DAGRUN_DELEGATED_CGROUP",
        "DAGRUN_DELEGATED_UNBOXED",
        "DAGRUN_IN_SCOPE",
        "DAGRUN_OUTER_RUN",
    ]
    .iter()
    .any(|key| std::env::var_os(key).is_some());
    if outer {
        eprintln!("skipping live re-exec: below an outer dagrun run (#2316)");
        return;
    }
    let probe = Command::new("systemd-run")
        .args([
            "--user",
            "--scope",
            "--quiet",
            "--collect",
            &format!("--unit=dagrun-expand-probe-{}", std::process::id()),
            "true",
        ])
        .output();
    if !probe.map(|o| o.status.success()).unwrap_or(false) {
        eprintln!("skipping live re-exec: no usable systemd --user scope here");
        return;
    }
    let fx = Fixture::new("live");
    let written = fx.dir.join("out.txt");
    let dag = fx.dag(&written);
    for payload in [PAYLOAD, "$PROBE"] {
        let _ = std::fs::remove_file(&written);
        let out = runner()
            .args(["run", "--dag", dag.to_str().unwrap(), "--no-profile"])
            .args(["--args", payload])
            .env("PROBE", "expanded")
            .env_remove("x")
            .output()
            .unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(out.status.code(), Some(0), "{text}");
        assert!(text.contains("cgroup boxing ACTIVE"), "{text}");
        assert_eq!(
            std::fs::read_to_string(&written).unwrap_or_default(),
            payload,
            "the step got a rewritten --args value:\n{text}"
        );
    }
}
