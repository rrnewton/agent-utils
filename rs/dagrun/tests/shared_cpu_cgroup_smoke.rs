//! Boxed smoke coverage for shared-parent per-command CPU cgroups.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use dagrun::{ManualCpuCgroup, ManualCpuCgroupStatus, SharedCpuCgroupParent};

const CHILD_ENV: &str = "DAGRUN_SHARED_CPU_CGROUP_SMOKE_CHILD";
const CREATOR_ENV: &str = "DAGRUN_SHARED_CPU_CGROUP_SMOKE_CREATOR";
const TEST_NAME: &str = "shared_cpu_parent_preserves_siblings_and_fast_exit_accounting";

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn cgroup_path_for(pid: u32) -> PathBuf {
    let text = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).unwrap();
    let relative = text
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .expect("the smoke requires the cgroup-v2 unified hierarchy")
        .trim_start_matches('/');
    Path::new("/sys/fs/cgroup").join(relative)
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

fn wait_for_cpu(child: &ManualCpuCgroup) -> u64 {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let usage = child.cpu_usage_usec().unwrap();
        if usage >= 20_000 {
            return usage;
        }
        assert!(
            Instant::now() < deadline,
            "manual CPU cgroup {} did not accumulate CPU use",
            child.path().display()
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn cpu_burner() -> Command {
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", "while :; do :; done"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

fn kill_and_reap(cgroup: &ManualCpuCgroup, child: &mut Child) -> u64 {
    cgroup.kill().unwrap();
    let status = child.wait().unwrap();
    assert!(!status.success(), "the CPU burner survived its cgroup kill");
    wait_for_status(cgroup, ManualCpuCgroupStatus::Empty);
    let usage = cgroup.final_cpu_usage_usec().unwrap();
    cgroup.cleanup().unwrap();
    usage
}

fn create_and_clean_one_child() {
    let parent = SharedCpuCgroupParent::current().unwrap();
    let child = parent.create_child("concurrent-creator").unwrap();
    let mut command = Command::new("/bin/true");
    child.attach_command(&mut command).unwrap();
    assert!(command.status().unwrap().success());
    wait_for_status(&child, ManualCpuCgroupStatus::Empty);
    child.final_cpu_usage_usec().unwrap();
    child.cleanup().unwrap();
}

fn finite_cpu_burner() -> Command {
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", "i=0; while [ $i -lt 200000 ]; do i=$((i + 1)); done"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

fn run_inside_boxed_step() {
    let parent_path = cgroup_path_for(std::process::id());
    let subtree_control_path = parent_path.join("cgroup.subtree_control");
    let subtree_control_before = std::fs::read_to_string(&subtree_control_path).unwrap();
    let mut unrelated = Command::new("/bin/sleep")
        .arg("30")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    assert_eq!(cgroup_path_for(unrelated.id()), parent_path);

    // The handle is deliberately created while another live process shares the parent.
    let parent = SharedCpuCgroupParent::current().unwrap();
    assert_eq!(parent.path(), parent_path);
    assert_eq!(cgroup_path_for(std::process::id()), parent_path);

    let this_test = std::env::current_exe().unwrap();
    let mut creators = Vec::new();
    for _ in 0..2 {
        creators.push(
            Command::new(&this_test)
                .args(["--exact", TEST_NAME, "--nocapture"])
                .env(CREATOR_ENV, "1")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
    }
    for mut creator in creators {
        assert!(creator.wait().unwrap().success());
    }

    let fast = parent.create_child("shared-fast-exit").unwrap();
    let fast_path = fast.path().to_path_buf();
    let mut fast_command = finite_cpu_burner();
    fast.attach_command(&mut fast_command).unwrap();
    assert!(fast_command.status().unwrap().success());
    drop(fast_command);
    wait_for_status(&fast, ManualCpuCgroupStatus::Empty);
    let fast_usage = fast.final_cpu_usage_usec().unwrap();
    assert!(
        fast_usage >= 10_000,
        "finite fast-exit burner used only {fast_usage}us"
    );
    fast.cleanup().unwrap();
    assert!(!fast_path.exists());

    let left = parent.create_child("shared-left").unwrap();
    let right = parent.create_child("shared-right").unwrap();
    assert_ne!(left.path(), right.path());
    let mut left_command = cpu_burner();
    let mut right_command = Command::new("/bin/sleep");
    right_command.arg("30");
    left.attach_command(&mut left_command).unwrap();
    right.attach_command(&mut right_command).unwrap();
    let mut left_process = left_command.spawn().unwrap();
    let mut right_process = right_command.spawn().unwrap();
    drop(left_command);
    drop(right_command);
    wait_for_cpu(&left);
    let idle_usage = right.cpu_usage_usec().unwrap();
    assert!(
        idle_usage < 10_000,
        "idle sibling accumulated {idle_usage}us while its peer burned CPU"
    );

    let left_usage = kill_and_reap(&left, &mut left_process);
    assert!(left_usage > 0);
    assert_eq!(right.status().unwrap(), ManualCpuCgroupStatus::Populated);
    assert!(
        unrelated.try_wait().unwrap().is_none(),
        "killing one owned child affected an unrelated parent process"
    );

    let right_usage = kill_and_reap(&right, &mut right_process);
    assert!(right_usage > 0);
    assert!(
        unrelated.try_wait().unwrap().is_none(),
        "killing the second owned child affected an unrelated parent process"
    );
    unrelated.kill().unwrap();
    let _ = unrelated.wait();
    assert!(parent.path().exists(), "the borrowed parent was removed");
    assert_eq!(cgroup_path_for(std::process::id()), parent_path);
    assert_eq!(
        std::fs::read_to_string(&subtree_control_path).unwrap(),
        subtree_control_before,
        "shared-parent API changed cgroup.subtree_control"
    );
}

#[test]
fn shared_cpu_parent_preserves_siblings_and_fast_exit_accounting() {
    if std::env::var(CREATOR_ENV).as_deref() == Ok("1") {
        create_and_clean_one_child();
        return;
    }
    if std::env::var(CHILD_ENV).as_deref() == Ok("1") {
        run_inside_boxed_step();
        return;
    }

    let dagrun = env!("CARGO_BIN_EXE_dagrun");
    let this_test = std::env::current_exe().unwrap();
    let scratch = std::env::temp_dir().join(format!(
        "dagrun-shared-cpu-smoke-parent-{}",
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
            "job": "shared-parent-smoke",
            "desc": "exercise independent CPU cgroups beneath a shared step parent",
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
        .expect("failed to start dagrun for shared-parent CPU-cgroup smoke");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");
    let _ = std::fs::remove_dir_all(&scratch);

    if output.status.code() == Some(3)
        && (combined.contains("systemd --user scope is unavailable")
            || combined.contains("scope setup SKIPPED BY POLICY"))
    {
        eprintln!(
            "HOST LIMITATION: shared-parent CPU-cgroup smoke requires cgroup v2 and a delegated \
             systemd --user scope:\n{combined}"
        );
        return;
    }
    assert!(
        output.status.success(),
        "boxed shared-parent CPU-cgroup smoke failed:\n{combined}"
    );
}
