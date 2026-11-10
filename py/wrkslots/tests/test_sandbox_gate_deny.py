"""Gate-admission writable path and log-only denied-write tracing (#1205)."""

from pathlib import Path

from wrkslots.sandbox import deny_log_command, gate_admission_dir


def test_gate_admission_default_is_unbound() -> None:
    # No host /tmp path leaks into a box unless the launcher exports it.
    assert gate_admission_dir({}) is None
    assert gate_admission_dir({"GATE_ADMISSION_DIR": ""}) is None


def test_gate_admission_env_override() -> None:
    assert gate_admission_dir({"GATE_ADMISSION_DIR": "/tmp/custom-admission"}) == Path(
        "/tmp/custom-admission"
    )


def test_deny_log_wraps_failed_file_syscalls() -> None:
    wrapped = deny_log_command(
        ["/bin/echo", "hi"], Path("/state/wrkslots-deny.log"), enabled=True, strace="/usr/bin/strace"
    )
    assert wrapped == [
        "/usr/bin/strace",
        "-f",
        "-qq",
        "-e",
        "trace=%file",
        "-e",
        "status=failed",
        "-o",
        "/state/wrkslots-deny.log",
        "--",
        "/bin/echo",
        "hi",
    ]


def test_deny_log_is_off_by_default_and_without_strace() -> None:
    command = ["/bin/echo", "hi"]
    assert deny_log_command(command, Path("/x.log"), enabled=False, strace="/usr/bin/strace") == command
    assert deny_log_command(command, Path("/x.log"), enabled=True, strace=None) == command
    assert deny_log_command([], Path("/x.log"), enabled=True, strace="/usr/bin/strace") == []
