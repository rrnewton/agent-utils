//! Boxed smoke coverage for the manual nested CPU-cgroup API.
//!
//! The outer test launches this same test binary as one dagrun step. That gives the child an
//! exclusively owned delegated cgroup, matching the API's production precondition. Hosts without
//! a usable cgroup-v2 systemd user scope report that limitation explicitly.

use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use dagrun::{ManualCpuCgroup, ManualCpuCgroupRoot, ManualCpuCgroupStatus};

const CHILD_ENV: &str = "DAGRUN_MANUAL_CPU_CGROUP_SMOKE_CHILD";
const TEST_NAME: &str = "manual_cpu_cgroup_contains_fast_exit_and_setsid_escape";

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn wait_for_status(child: &ManualCpuCgroup, wanted: ManualCpuCgroupStatus) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.status() {
            Ok(observed) if observed == wanted => return,
            Ok(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(observed) => panic!(
                "manual CPU cgroup {} remained {observed:?}; wanted {wanted:?}",
                child.path().display()
            ),
            Err(error) => panic!(
                "manual CPU cgroup {} status became unreadable: {error}",
                child.path().display()
            ),
        }
    }
}

fn run_inside_boxed_step() {
    let root = ManualCpuCgroupRoot::current()
        .expect("the boxed step must provide an exclusively delegated CPU cgroup");

    let fast = root.create_child("fast-exit").unwrap();
    let mut fast_command = Command::new("/bin/true");
    fast.attach_command(&mut fast_command).unwrap();
    assert!(fast_command.status().unwrap().success());
    drop(fast_command);
    wait_for_status(&fast, ManualCpuCgroupStatus::Empty);
    // A very fast child may legitimately consume zero rounded microseconds. The important
    // property is that the value came from a successful final cpu.stat read.
    let _fast_cpu_usec = fast.final_cpu_usage_usec().unwrap();
    fast.cleanup().unwrap();

    let setsid_available = Command::new("setsid")
        .arg("--help")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !setsid_available {
        eprintln!(
            "HOST LIMITATION: setsid is unavailable; fast-exit cgroup coverage passed, but the \
             process-group escape smoke cannot run"
        );
        root.cleanup().unwrap();
        return;
    }

    let escape = root.create_child("setsid-escape").unwrap();
    let scratch =
        std::env::temp_dir().join(format!("dagrun-manual-cpu-escape-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).unwrap();
    let pid_file = scratch.join("escape.pid");
    let script = format!(
        "setsid /bin/sh -c 'while :; do :; done' </dev/null >/dev/null 2>&1 & echo $! > {}",
        shell_quote(&pid_file.to_string_lossy())
    );
    let mut leader = Command::new("/bin/sh");
    leader.arg("-c").arg(script);
    escape.attach_command(&mut leader).unwrap();
    let mut leader_child = leader.spawn().unwrap();
    let leader_pid = leader_child.id();
    let status = leader_child.wait().unwrap();
    assert!(status.success());
    drop(leader_child);
    drop(leader);

    let deadline = Instant::now() + Duration::from_secs(5);
    while !pid_file.is_file() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    let escape_pid: u32 = std::fs::read_to_string(&pid_file)
        .expect("setsid child did not publish its pid")
        .trim()
        .parse()
        .unwrap();
    assert_ne!(escape_pid, leader_pid);
    wait_for_status(&escape, ManualCpuCgroupStatus::Populated);

    // The command leader is gone and the remaining process called setsid, but cgroup.kill still
    // reaches it because session and process-group changes do not change cgroup membership.
    escape.kill().unwrap();
    wait_for_status(&escape, ManualCpuCgroupStatus::Empty);
    let escape_cpu_usec = escape.final_cpu_usage_usec().unwrap();
    assert!(
        escape_cpu_usec > 0,
        "the spinning escapee must consume measured CPU"
    );
    escape.cleanup().unwrap();
    root.cleanup().unwrap();
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn manual_cpu_cgroup_contains_fast_exit_and_setsid_escape() {
    if std::env::var(CHILD_ENV).as_deref() == Ok("1") {
        run_inside_boxed_step();
        return;
    }

    let dagrun = env!("CARGO_BIN_EXE_dagrun");
    let this_test = std::env::current_exe().unwrap();
    let scratch = std::env::temp_dir().join(format!(
        "dagrun-manual-cpu-smoke-parent-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).unwrap();
    let dag = scratch.join("dag.json");
    let command = format!(
        "export {CHILD_ENV}=1; exec {} --exact {TEST_NAME} --nocapture",
        shell_quote(&this_test.to_string_lossy())
    );
    let document = serde_json::json!({
        "steps": [{
            "group": "manual-cpu",
            "job": "smoke",
            "desc": "exercise a shared nested CPU cgroup",
            "cmd": command
        }]
    });
    std::fs::write(&dag, serde_json::to_vec(&document).unwrap()).unwrap();

    let output = Command::new(dagrun)
        .args([
            "run",
            "--dag",
            dag.to_str().unwrap(),
            "--max-steps",
            "1",
            "--max-cpus",
            "2",
            "--no-profile",
        ])
        .env("DAGRUN_FORCE_SCOPE_ATTEMPT", "1")
        .output()
        .expect("failed to start dagrun for nested CPU-cgroup smoke");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");
    let _ = std::fs::remove_dir_all(&scratch);

    if output.status.code() == Some(3)
        && (combined.contains("systemd --user scope is unavailable")
            || combined.contains("scope setup SKIPPED BY POLICY"))
    {
        eprintln!(
            "HOST LIMITATION: nested CPU-cgroup smoke requires cgroup v2 and a delegated \
             systemd --user scope:\n{combined}"
        );
        return;
    }
    assert!(
        output.status.success(),
        "boxed nested CPU-cgroup smoke failed:\n{combined}"
    );
}
