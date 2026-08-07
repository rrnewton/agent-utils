//! End-to-end proof that the native `box` surface both exists and enforces its limit.

use std::process::Command;

#[test]
fn box_help_succeeds_and_memory_breach_is_killed() {
    let bin = env!("CARGO_BIN_EXE_safe-ci-dag-runner");

    let help = Command::new(bin)
        .args(["box", "--help"])
        .output()
        .expect("failed to run box --help");
    assert!(help.status.success(), "box --help must exit zero");
    let help_text = String::from_utf8_lossy(&help.stdout);
    assert!(
        help_text.contains("fail-closed cgroup-v2 box")
            && help_text.contains("--mem SIZE")
            && help_text.contains("--timeout SECS"),
        "box --help omitted its enforcement contract:\n{help_text}"
    );

    // Grow memory in the step leader until the inner cgroup's 64-MiB memory.max kills it. This
    // distinguishes a working command surface from the historical inert-enforcement failure.
    let breach = Command::new(bin)
        .args([
            "box",
            "--mem",
            "64M",
            "--timeout",
            "20",
            "--label",
            "integration-breach",
            "--quiet",
            "--",
            "bash",
            "-c",
            "s=x; while true; do s=\"$s$s\"; done",
        ])
        .output()
        .expect("failed to run the box breach probe");
    let stdout = String::from_utf8_lossy(&breach.stdout);
    let stderr = String::from_utf8_lossy(&breach.stderr);
    let combined = format!("{stdout}{stderr}");

    if breach.status.code() == Some(3) {
        eprintln!(
            "SKIP box limit enforcement: cgroup boxing unavailable (need cgroup-v2 + a working \
             systemd --user scope). Details:\n{combined}"
        );
        return;
    }

    assert_eq!(
        breach.status.code(),
        Some(1),
        "an over-limit command must be killed and fail:\n{combined}"
    );
    assert!(
        combined.contains("VERDICT label=integration-breach class=OOM")
            && combined.contains("OOM-KILLED"),
        "expected a cgroup OOM verdict proving memory.max enforcement:\n{combined}"
    );
}
