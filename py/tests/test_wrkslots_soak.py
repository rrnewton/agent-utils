"""WRKSLOTS_ENGINE=rust-soak dispatch for the wrkslots soak.

#562 rust-primary-wrkslots: soak mode accepts the exact Python CLI,
runs covered commands in Rust under their wrkslotsd names, runs the
rest in Python with a stderr note, and logs every invocation. The
sandbox uses stub launchers; the log goes to a temp file, never to
the operator's ~/.local/state.
"""

from __future__ import annotations

import os
import shutil
import stat
import subprocess
from pathlib import Path


AU_ROOT = Path(__file__).resolve().parents[2]
RESOLVER = AU_ROOT / "common" / "bin" / "engine-resolver"
SOAK = AU_ROOT / "common" / "bin" / "wrkslots-soak"

STUB_RUST = """#!/bin/sh
case "$1" in
  --help) echo "wrkslotsd stub" ;;
  parse-status)
    if [ "$2" = "--help" ]; then echo "Usage: --root --machine"; exit 0; fi
    echo "RS parse-status $@" ;;
  parse-doctor)
    if [ "$2" = "--help" ]; then echo "Usage: --root --machine --all-machines"; exit 0; fi
    echo "RS parse-doctor $@" ;;
  doctor-text)
    if [ "$2" = "--help" ]; then echo "Usage: --root --machine --all-machines --format"; exit 0; fi
    echo "RS doctor-text $@" ;;
  audit-text)
    if [ "$2" = "--help" ]; then echo "Usage: --root --machine --cache-census-state --cache-work-limit --cache-wall-seconds --gate --format"; exit 0; fi
    echo "RS audit-text $@" ;;
  status-text)
    if [ "$2" = "--help" ]; then echo "Usage: --root --machine --all-machines --slot --format"; exit 0; fi
    echo "RS status-text $@" ;;
  shell-command)
    if [ "$2" = "--help" ]; then echo "Usage: --root --machine --slot-type --box --name --writable --cwd --isolation --shell --format"; exit 0; fi
    echo "RS shell-command $@" ;;
  heartbeat)
    if [ "$2" = "--help" ]; then echo "Usage: --agent --owner-pid --expected-generation --root --machine"; exit 0; fi
    echo "RS heartbeat-run $@" ;;
  *) echo "RS $@" ;;
esac
"""


def _chmod_x(path: Path) -> None:
    path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def _sandbox(tmp_path: Path) -> tuple[Path, Path]:
    root = tmp_path / "au"
    for directory in (root / "common" / "bin", root / "py" / "bin", root / "rs" / "bin"):
        directory.mkdir(parents=True)
    shutil.copy2(RESOLVER, root / "common" / "bin" / "engine-resolver")
    shutil.copy2(SOAK, root / "common" / "bin" / "wrkslots-soak")
    _chmod_x(root / "common" / "bin" / "engine-resolver")
    _chmod_x(root / "common" / "bin" / "wrkslots-soak")
    (root / "common" / "bin" / "wrkslots").symlink_to("engine-resolver")
    python_stub = root / "py" / "bin" / "wrkslots"
    python_stub.write_text("import sys\nprint('PY wrkslots ' + ' '.join(sys.argv[1:]))\n")
    _chmod_x(python_stub)
    rust_stub = root / "rs" / "bin" / "wrkslotsd"
    rust_stub.write_text(STUB_RUST)
    _chmod_x(rust_stub)
    (root / "validation").mkdir()
    (root / "validation" / "editions.tsv").write_text("wrkslots\tpython\tabsent\t\n")
    return root, tmp_path / "soak.log"


def _run(root: Path, log: Path, *args: str) -> subprocess.CompletedProcess[str]:
    env = {
        key: value
        for key, value in os.environ.items()
        if key not in ("DAGRUN_ENGINE", "AGENT_UTILS_ENGINE", "WRKSLOTS_ENGINE", "CI_HUB_PYTHON")
    }
    env["WRKSLOTS_ENGINE"] = "rust-soak"
    env["WRKSLOTS_SOAK_LOG"] = str(log)
    return subprocess.run(
        [str(root / "common" / "bin" / "wrkslots"), *args],
        cwd=root, capture_output=True, text=True, env=env, check=False, timeout=30,
    )


def test_covered_reader_runs_in_rust_with_translation(tmp_path: Path) -> None:
    root, log = _sandbox(tmp_path)
    result = _run(root, log, "status", "--format", "json")
    assert result.returncode == 0, result.stderr
    assert result.stdout.startswith("RS status-text status-text --format json")
    assert "cmd=status edition=rust rc=0" in log.read_text()


def test_human_status_runs_in_rust(tmp_path: Path) -> None:
    root, log = _sandbox(tmp_path)
    result = _run(root, log, "status")
    assert result.returncode == 0, result.stderr
    assert result.stdout.startswith("RS status-text status-text")
    assert "cmd=status edition=rust rc=0" in log.read_text()


def test_human_doctor_runs_in_rust(tmp_path: Path) -> None:
    root, log = _sandbox(tmp_path)
    result = _run(root, log, "doctor")
    assert result.returncode == 0, result.stderr
    assert result.stdout.startswith("RS doctor-text doctor-text")
    assert "cmd=doctor edition=rust rc=0" in log.read_text()


def test_human_audit_runs_in_rust(tmp_path: Path) -> None:
    root, log = _sandbox(tmp_path)
    result = _run(root, log, "audit", "--gate")
    assert result.returncode == 0, result.stderr
    assert result.stdout.startswith("RS audit-text audit-text --gate")
    assert "cmd=audit edition=rust rc=0" in log.read_text()


def test_shell_command_runs_in_rust(tmp_path: Path) -> None:
    root, log = _sandbox(tmp_path)
    result = _run(root, log, "shell-command", "slot01", "--format", "json")
    assert result.returncode == 0, result.stderr
    assert result.stdout.startswith("RS shell-command shell-command slot01 --format json")
    assert "cmd=shell-command edition=rust rc=0" in log.read_text()


def test_uncovered_command_stays_python_with_note(tmp_path: Path) -> None:
    root, log = _sandbox(tmp_path)
    result = _run(root, log, "init", "--help")
    assert result.stdout.startswith("PY wrkslots init")
    assert "cmd=init edition=python" in log.read_text()


def test_covered_writer_runs_in_rust(tmp_path: Path) -> None:
    root, log = _sandbox(tmp_path)
    result = _run(
        root, log, "heartbeat", "slot1",
        "--agent", "a", "--owner-pid", "1", "--expected-generation", "2",
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout.startswith("RS heartbeat-run heartbeat slot1")
    assert "cmd=heartbeat edition=rust" in log.read_text()


def test_unknown_flag_falls_back_to_python(tmp_path: Path) -> None:
    root, log = _sandbox(tmp_path)
    result = _run(
        root, log, "heartbeat", "slot1", "--agent", "a", "--bogus-flag", "1",
    )
    assert result.stdout.startswith("PY wrkslots heartbeat")
    assert "python (not yet in Rust)" in result.stderr


def test_format_value_is_checked_not_just_the_flag(tmp_path: Path) -> None:
    root, log = _sandbox(tmp_path)
    human = _run(root, log, "create", "--format", "human")
    assert human.stdout.startswith("PY wrkslots create")
    bogus = _run(root, log, "create", "--format", "bogus")
    assert bogus.stdout.startswith("PY wrkslots create")


def test_format_default_commands_stay_python_without_json(tmp_path: Path) -> None:
    root, log = _sandbox(tmp_path)
    result = _run(root, log, "unpushed")
    assert result.stdout.startswith("PY wrkslots unpushed")
    assert "python (not yet in Rust)" in result.stderr


def test_abbreviated_flag_falls_back_to_python(tmp_path: Path) -> None:
    root, log = _sandbox(tmp_path)
    result = _run(root, log, "heartbeat", "slot1", "--agen", "a")
    assert result.stdout.startswith("PY wrkslots heartbeat")


def test_wait_lock_reaches_the_rust_path(tmp_path: Path) -> None:
    root, log = _sandbox(tmp_path)
    result = _run(
        root, log, "--wait-lock", "5", "heartbeat", "slot1",
        "--agent", "a", "--owner-pid", "1", "--expected-generation", "2",
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout.startswith("RS heartbeat-run")
    assert "cmd=heartbeat edition=rust" in log.read_text()
