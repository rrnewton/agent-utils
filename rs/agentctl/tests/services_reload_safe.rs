//! `agentctl services refresh --no-reload` and `agentctl services transients` end to end through
//! the real CLI (#2441), with a fake `systemctl` (`--systemctl PATH`) that answers list-units,
//! list-unit-files, show and cat from fixture files and logs every call. Nothing here reaches a
//! user manager. Linux only (the fake is a POSIX shell script).

#![cfg(target_os = "linux")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A fake systemctl: `show` and `cat` print fixture files named after the unit, `list-units`
/// prints the fixture for its `--type`, and every call is appended to `calls.log`.
const FAKE_SYSTEMCTL: &str = r#"#!/bin/sh
dir="$FAKE_SYSTEMCTL_DIR"
echo "$*" >> "$dir/calls.log"
[ "$1" = "--user" ] && shift
verb="$1"
shift
case "$verb" in
  list-units)
    for arg in "$@"; do
      case "$arg" in
        --type=*) cat "$dir/list-units-${arg#--type=}" 2>/dev/null ;;
      esac
    done
    ;;
  list-unit-files) ;;
  show)
    first=1
    for arg in "$@"; do
      case "$arg" in
        *.service|*.scope)
          [ "$first" = 1 ] || echo
          first=0
          cat "$dir/show-$arg"
          ;;
      esac
    done
    ;;
  cat)
    for arg in "$@"; do
      case "$arg" in
        *.service|*.scope) cat "$dir/cat-$arg" ;;
      esac
    done
    ;;
  *) ;;
esac
exit 0
"#;

const NEW: &str = "2222222222222222222222222222222222222222";

struct Scene {
    base: PathBuf,
    fixtures: PathBuf,
    systemctl: PathBuf,
    registry: PathBuf,
    root: PathBuf,
    drop_ins: PathBuf,
}

impl Drop for Scene {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn scene(label: &str) -> Scene {
    let base = std::env::temp_dir().join(format!(
        "agentctl-reload-safe-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let fixtures = base.join("fixtures");
    fs::create_dir_all(&fixtures).unwrap();
    let systemctl = base.join("systemctl");
    fs::write(&systemctl, FAKE_SYSTEMCTL).unwrap();
    fs::set_permissions(&systemctl, fs::Permissions::from_mode(0o755)).unwrap();
    let registry = base.join("team/.agentctl");
    fs::create_dir_all(&registry).unwrap();
    let root = base.join("services");
    // A snapshot to move onto.
    let snapshot = root.join("snapshots").join(&NEW[..12]);
    fs::create_dir_all(snapshot.join("bin")).unwrap();
    fs::write(snapshot.join("bin/agentctl"), "#!/bin/sh\n").unwrap();
    fs::write(
        snapshot.join("snapshot.json"),
        format!(
            "{{\"schema\":\"agentctl-services-snapshot/v1\",\"commit\":\"{NEW}\",\"built_at\":1,\"source\":{{\"committed_at\":1}},\"files\":{{}}}}"
        ),
    )
    .unwrap();
    Scene {
        drop_ins: base.join("drop-ins"),
        base,
        fixtures,
        systemctl,
        registry,
        root,
    }
}

fn agentctl(scene: &Scene, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agentctl"))
        .arg("--registry")
        .arg(&scene.registry)
        .args(args)
        .env("FAKE_SYSTEMCTL_DIR", &scene.fixtures)
        .output()
        .unwrap()
}

fn calls(scene: &Scene) -> String {
    fs::read_to_string(scene.fixtures.join("calls.log")).unwrap_or_default()
}

fn write_headless_unit(scene: &Scene) {
    let registry = scene.registry.display();
    let exec = format!(
        "/nonexistent/bin/agentctl-old --registry {registry} run-headless --approval-timeout 600 lead"
    );
    fs::write(
        scene.fixtures.join("list-units-service"),
        "headless.service loaded active running headless wrapper\n",
    )
    .unwrap();
    fs::write(
        scene.fixtures.join("show-headless.service"),
        format!(
            "Id=headless.service\nLoadState=loaded\nActiveState=active\nMainPID=4242\nFragmentPath=/units/headless.service\nDropInPaths=\nTransient=no\nExecStart={{ path=/nonexistent/bin/agentctl-old ; argv[]={exec} ; }}\nNeedDaemonReload=no\n"
        ),
    )
    .unwrap();
    fs::write(
        scene.fixtures.join("cat-headless.service"),
        format!(
            "# /units/headless.service\n[Unit]\nDescription=headless wrapper\n\n[Service]\nExecStart={exec}\n"
        ),
    )
    .unwrap();
}

#[test]
fn no_reload_refuses_a_unit_file_move_without_touching_systemd() {
    let scene = scene("refuse");
    write_headless_unit(&scene);
    let output = agentctl(
        &scene,
        &[
            "services",
            "--systemctl",
            &scene.systemctl.to_string_lossy(),
            "--root",
            &scene.root.to_string_lossy(),
            "refresh",
            "--no-reload",
            "--drop-in-dir",
            &scene.drop_ins.to_string_lossy(),
            "--json",
        ],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stdout}\n{stderr}");
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let row = &report["services"][0];
    assert_eq!(row["unit"], "headless.service", "{report}");
    assert_eq!(row["action"], "refused", "{report}");
    assert_eq!(row["form"], "drop-in", "{report}");
    assert!(
        row["reason"]
            .as_str()
            .unwrap()
            .starts_with("needs-daemon-reload: "),
        "{report}"
    );
    // Only reads reached systemctl: no reload, restart, stop, start or systemd-run.
    let log = calls(&scene);
    for line in log.lines() {
        let verb = line.split_whitespace().nth(1).unwrap_or("");
        assert!(
            matches!(verb, "list-units" | "list-unit-files" | "show" | "cat"),
            "unexpected systemctl call {line:?} in\n{log}"
        );
    }
    assert!(log.contains("NeedDaemonReload"), "{log}");
    assert!(!scene.drop_ins.exists());
    assert!(!scene.root.join("links").exists());
    assert!(!scene.root.join("refreshes").exists());
}

fn write_scope(scene: &Scene, cgroups: &Path, unit: &str, active: &str, populated: bool) {
    let cgroup = format!("/user.slice/app.slice/{unit}");
    fs::write(
        scene.fixtures.join(format!("show-{unit}")),
        format!(
            "Id={unit}\nTransient=yes\nLoadState=loaded\nActiveState={active}\nSubState={}\nControlGroup={cgroup}\nMainPID=0\n",
            if active == "active" { "running" } else { "dead" }
        ),
    )
    .unwrap();
    let directory = cgroups.join(cgroup.trim_start_matches('/'));
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("cgroup.events"),
        format!("populated {}\n", u8::from(populated)),
    )
    .unwrap();
    fs::write(directory.join("cpu.stat"), "usage_usec 1000\n").unwrap();
}

#[test]
fn transients_save_then_check_names_a_scope_systemd_lost() {
    let scene = scene("transients");
    let cgroups = scene.base.join("cgroup");
    fs::write(
        scene.fixtures.join("list-units-service,scope"),
        "run-a.scope loaded active running a\nrun-b.scope loaded active running b\n",
    )
    .unwrap();
    write_scope(&scene, &cgroups, "run-a.scope", "active", true);
    write_scope(&scene, &cgroups, "run-b.scope", "active", true);
    let record = scene.base.join("before.json");
    let common = [
        "services".to_owned(),
        "--systemctl".to_owned(),
        scene.systemctl.to_string_lossy().into_owned(),
        "transients".to_owned(),
        "--cgroup-root".to_owned(),
        cgroups.to_string_lossy().into_owned(),
    ];
    let with = |extra: &[&str]| {
        let mut args: Vec<&str> = common.iter().map(String::as_str).collect();
        args.extend_from_slice(extra);
        agentctl(&scene, &args)
    };
    let saved = with(&["--save", &record.to_string_lossy()]);
    assert_eq!(
        saved.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&saved.stderr)
    );
    assert!(record.is_file());
    // The same record twice is refused.
    assert_ne!(
        with(&["--save", &record.to_string_lossy()]).status.code(),
        Some(0)
    );
    // A reload later: systemd calls run-b dead while its cgroup still runs.
    write_scope(&scene, &cgroups, "run-b.scope", "inactive", true);
    let checked = with(&["--check", &record.to_string_lossy()]);
    let text = String::from_utf8_lossy(&checked.stdout);
    assert_eq!(checked.status.code(), Some(1), "{text}");
    assert!(text.contains("run-a.scope: KEPT"), "{text}");
    assert!(text.contains("run-b.scope: LOST"), "{text}");
    // Read-only throughout.
    let log = calls(&scene);
    for line in log.lines() {
        let verb = line.split_whitespace().nth(1).unwrap_or("");
        assert!(
            matches!(verb, "list-units" | "show"),
            "unexpected systemctl call {line:?} in\n{log}"
        );
    }
}
