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
  unpushed-text)
    if [ "$2" = "--help" ]; then echo "Usage: --root --machine --slot --format"; exit 0; fi
    echo "RS unpushed-text $@" ;;
  create-text)
    if [ "$2" = "--help" ]; then echo "Usage: --root --machine --slot-type --coordinator-authorized --agent --task --purpose --owner-pid --coordinator-pid --repo --remote --remote-url --branch --start --override-disk-floor --task-scope-unit --task-scope-invocation-id --representation --format"; exit 0; fi
    echo "RS create-text $@" ;;
  recover-absent-agent-rows-text)
    if [ "$2" = "--help" ]; then echo "Usage: --root --machine --row --input --apply --coordinator-authorized --coordinator-pid --format"; exit 0; fi
    echo "RS recover-absent-agent-rows-text $@" ;;
  recover-absent-validate-rows-text)
    if [ "$2" = "--help" ]; then echo "Usage: --root --machine --input --apply --coordinator-authorized --coordinator-pid --format"; exit 0; fi
    echo "RS recover-absent-validate-rows-text $@" ;;
  recover-ownerless-validate-batch-text)
    if [ "$2" = "--help" ]; then echo "Usage: --root --machine --checkout --frozen-validate-checkout --completed-record --repository --validation-proof-manifest --coordinator-authorized --coordinator-pid --format"; exit 0; fi
    echo "RS recover-ownerless-validate-batch-text $@" ;;
  freeze)
    if [ "$2" = "--help" ]; then echo "Usage: --root --machine set clear status"; exit 0; fi
    echo "RS freeze $@" ;;
  reap-stray-worktrees)
    if [ "$2" = "--help" ]; then echo "Usage: --root --machine --repository --apply --format"; exit 0; fi
    echo "RS reap-stray-worktrees $@" ;;
  reclaim-orphan-state)
    if [ "$2" = "--help" ]; then echo "Usage: --root --machine --apply --format"; exit 0; fi
    echo "RS reclaim-orphan-state $@" ;;
  classify-create-journals)
    if [ "$2" = "--help" ]; then echo "Usage: --root --machine --agent --slot-type --format"; exit 0; fi
    echo "RS classify-create-journals $@" ;;
  classify-ownerless-validate-batch-text)
    if [ "$2" = "--help" ]; then echo "Usage: --root --machine --checkout --frozen-validate-checkout --completed-record --repository --validation-proof-manifest --format"; exit 0; fi
    echo "RS classify-ownerless-validate-batch-text $@" ;;
  quickstart)
    if [ "$2" = "--help" ]; then echo "Usage: quickstart"; exit 0; fi
    echo "RS quickstart $@" ;;
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


def _run(
    root: Path, log: Path, *args: str, readers_only: bool = False
) -> subprocess.CompletedProcess[str]:
    env = {
        key: value
        for key, value in os.environ.items()
        if key
        not in (
            "DAGRUN_ENGINE",
            "AGENT_UTILS_ENGINE",
            "WRKSLOTS_ENGINE",
            "WRKSLOTS_SOAK_READERS_ONLY",
            "CI_HUB_PYTHON",
        )
    }
    env["WRKSLOTS_ENGINE"] = "rust-soak"
    if readers_only:
        env["WRKSLOTS_SOAK_READERS_ONLY"] = "1"
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


def test_unpushed_both_formats_run_in_rust(tmp_path: Path) -> None:
    root, log = _sandbox(tmp_path)
    human = _run(root, log, "unpushed")
    assert human.returncode == 0, human.stderr
    assert human.stdout.startswith("RS unpushed-text unpushed-text")
    payload = _run(root, log, "unpushed", "--format", "json")
    assert payload.returncode == 0, payload.stderr
    assert payload.stdout.startswith("RS unpushed-text unpushed-text --format json")
    assert "cmd=unpushed edition=rust rc=0" in log.read_text()


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


def test_create_both_formats_run_in_rust(tmp_path: Path) -> None:
    root, log = _sandbox(tmp_path)
    human = _run(root, log, "create", "slot01")
    assert human.returncode == 0, human.stderr
    assert human.stdout.startswith("RS create-text create-text slot01")
    payload = _run(root, log, "create", "slot01", "--format", "json")
    assert payload.returncode == 0, payload.stderr
    assert payload.stdout.startswith("RS create-text create-text slot01 --format json")
    assert "cmd=create edition=rust rc=0" in log.read_text()


def test_freeze_runs_in_rust(tmp_path: Path) -> None:
    root, log = _sandbox(tmp_path)
    result = _run(root, log, "freeze", "status")
    assert result.returncode == 0, result.stderr
    assert result.stdout.startswith("RS freeze freeze status")
    assert "cmd=freeze edition=rust rc=0" in log.read_text()
def test_reap_stray_worktrees_runs_in_rust(tmp_path: Path) -> None:
    root, log = _sandbox(tmp_path)
    result = _run(root, log, "reap-stray-worktrees", "--repository", "repo1")
    assert result.returncode == 0, result.stderr
    assert result.stdout.startswith("RS reap-stray-worktrees reap-stray-worktrees --repository repo1")
    assert "cmd=reap-stray-worktrees edition=rust rc=0" in log.read_text()


def test_reclaim_orphan_state_runs_in_rust(tmp_path: Path) -> None:
    root, log = _sandbox(tmp_path)
    result = _run(root, log, "reclaim-orphan-state")
    assert result.returncode == 0, result.stderr
    assert result.stdout.startswith("RS reclaim-orphan-state reclaim-orphan-state")
    assert "cmd=reclaim-orphan-state edition=rust rc=0" in log.read_text()


def test_classify_create_journals_runs_in_rust(tmp_path: Path) -> None:
    root, log = _sandbox(tmp_path)
    result = _run(root, log, "classify-create-journals", "slot01", "--agent", "a1")
    assert result.returncode == 0, result.stderr
    assert result.stdout.startswith("RS classify-create-journals classify-create-journals slot01")
    assert "cmd=classify-create-journals edition=rust rc=0" in log.read_text()


def test_recover_batch_both_formats_run_in_rust(tmp_path: Path) -> None:
    root, log = _sandbox(tmp_path)
    human = _run(root, log, "recover-absent-validate-rows", "--input", "rows.json")
    assert human.returncode == 0, human.stderr
    assert human.stdout.startswith(
        "RS recover-absent-validate-rows-text recover-absent-validate-rows-text --input rows.json"
    )
    payload = _run(
        root, log, "classify-ownerless-validate-batch",
        "--checkout", "c1", "--format", "json",
    )
    assert payload.returncode == 0, payload.stderr
    assert payload.stdout.startswith(
        "RS classify-ownerless-validate-batch-text classify-ownerless-validate-batch-text"
    )
    assert "cmd=classify-ownerless-validate-batch edition=rust rc=0" in log.read_text()


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


def test_quickstart_runs_in_rust(tmp_path: Path) -> None:
    root, log = _sandbox(tmp_path)
    result = _run(root, log, "--project-root", "/p", "quickstart")
    assert result.returncode == 0, result.stderr
    assert result.stdout.startswith("RS quickstart quickstart")
    assert "--root" not in result.stdout
    assert "cmd=quickstart edition=rust rc=0" in log.read_text()


def test_readers_only_approved_readers_still_run_in_rust(tmp_path: Path) -> None:
    root, log = _sandbox(tmp_path)
    result = _run(root, log, "status", readers_only=True)
    assert result.returncode == 0, result.stderr
    assert result.stdout.startswith("RS status-text status-text")
    assert "cmd=status edition=rust rc=0" in log.read_text()


def test_readers_only_audit_stays_python(tmp_path: Path) -> None:
    root, log = _sandbox(tmp_path)
    result = _run(root, log, "audit", "--gate", readers_only=True)
    assert result.stdout.startswith("PY wrkslots audit")
    assert "cmd=audit edition=python" in log.read_text()


def test_readers_only_writer_stays_python(tmp_path: Path) -> None:
    root, log = _sandbox(tmp_path)
    result = _run(
        root, log, "heartbeat", "slot1",
        "--agent", "a", "--owner-pid", "1", "--expected-generation", "2",
        readers_only=True,
    )
    assert result.stdout.startswith("PY wrkslots heartbeat")
    assert "cmd=heartbeat edition=python" in log.read_text()
