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
SHADOW = AU_ROOT / "common" / "bin" / "wrkslots-shadow"

STUB_RUST = """#!/bin/sh
if [ "$AGENT_UTILS_RS_PROBE_ONLY" = "1" ]; then
  if [ -f ./rust-cold ]; then echo "stub cache=cold" >&2; exit 75; fi
  echo "stub-snapshot-path"; exit 0;
fi
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
  validate-config)
    if [ -f ./validate-image ]; then echo '{"slot_representation": "image"}'; else echo '{"slot_representation": "worktree"}'; fi ;;
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
  image-status)
    if [ "$2" = "--help" ]; then echo "Usage: --root --machine --format"; exit 0; fi
    echo "RS image-status $@" ;;
  sandbox-show-config)
    if [ "$2" = "--help" ]; then echo "Usage: --root --machine"; exit 0; fi
    echo "RS sandbox-show-config $@" ;;
  limits-show)
    if [ "$2" = "--help" ]; then echo "Usage: --root --machine --memory-fraction --cpu-fraction"; exit 0; fi
    echo "RS limits-show $@" ;;
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
    shutil.copy2(SHADOW, root / "common" / "bin" / "wrkslots-shadow")
    _chmod_x(root / "common" / "bin" / "engine-resolver")
    _chmod_x(root / "common" / "bin" / "wrkslots-soak")
    _chmod_x(root / "common" / "bin" / "wrkslots-shadow")
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
    root: Path,
    log: Path,
    *args: str,
    readers_only: bool = False,
    extra_env: dict[str, str] | None = None,
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
            "AGENT_UTILS_RS_NO_BUILD",
            "AGENT_UTILS_RS_PROBE_ONLY",
            "AGENT_UTILS_RS_ENSURE_ONLY",
            "WRKSLOTS_RUST_DISABLE",
            "WRKSLOTS_SHADOW",
            "WRKSLOTS_SHADOW_N",
            "WRKSLOTS_SHADOW_TIMEOUT_S",
            "WRKSLOTS_DIVERGENCE_LOG",
        )
    }
    env["WRKSLOTS_ENGINE"] = "rust-soak"
    if readers_only:
        env["WRKSLOTS_SOAK_READERS_ONLY"] = "1"
    if extra_env:
        env.update(extra_env)
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


def test_create_image_mode_hands_off_to_python(tmp_path: Path) -> None:
    root, log = _sandbox(tmp_path)
    explicit = _run(root, log, "create", "slot01", "--representation", "image")
    assert explicit.returncode == 0, explicit.stderr
    assert explicit.stdout.startswith("PY wrkslots create slot01 --representation image")
    assert "image provisioning in Python" in explicit.stderr

    (root / "validate-image").touch()
    configured = _run(root, log, "create", "slot01")
    assert configured.returncode == 0, configured.stderr
    assert configured.stdout.startswith("PY wrkslots create slot01")
    assert "image provisioning in Python" in configured.stderr
    assert log.read_text().count("cmd=create edition=python rc=0") == 2
    assert "cmd=create edition=rust" not in log.read_text()


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


def test_readers_only_image_status_runs_in_rust(tmp_path: Path) -> None:
    root, log = _sandbox(tmp_path)
    result = _run(root, log, "image", "status", "--format", "json", readers_only=True)
    assert result.returncode == 0, result.stderr
    assert result.stdout.startswith("RS image-status image-status --format json")
    assert "cmd=image edition=rust rc=0" in log.read_text()


def test_readers_only_sandbox_show_config_runs_in_rust(tmp_path: Path) -> None:
    root, log = _sandbox(tmp_path)
    result = _run(root, log, "sandbox", "show-config", readers_only=True)
    assert result.returncode == 0, result.stderr
    assert result.stdout.startswith("RS sandbox-show-config sandbox-show-config")
    assert "cmd=sandbox edition=rust rc=0" in log.read_text()


def test_readers_only_limits_show_runs_in_rust(tmp_path: Path) -> None:
    root, log = _sandbox(tmp_path)
    result = _run(root, log, "limits", "show", readers_only=True)
    assert result.returncode == 0, result.stderr
    assert result.stdout.startswith("RS limits-show limits-show")
    assert "cmd=limits edition=rust rc=0" in log.read_text()


def test_readers_only_image_set_default_writer_stays_python(tmp_path: Path) -> None:
    root, log = _sandbox(tmp_path)
    result = _run(root, log, "image", "set-default", "image", readers_only=True)
    assert result.stdout.startswith("PY wrkslots image")
    assert "cmd=image edition=python" in log.read_text()


def test_readers_only_limits_apply_writer_stays_python(tmp_path: Path) -> None:
    root, log = _sandbox(tmp_path)
    result = _run(root, log, "limits", "apply", readers_only=True)
    assert result.stdout.startswith("PY wrkslots limits")
    assert "cmd=limits edition=python" in log.read_text()


def test_readers_only_writer_stays_python(tmp_path: Path) -> None:
    root, log = _sandbox(tmp_path)
    result = _run(
        root, log, "heartbeat", "slot1",
        "--agent", "a", "--owner-pid", "1", "--expected-generation", "2",
        readers_only=True,
    )
    assert result.stdout.startswith("PY wrkslots heartbeat")
    assert "cmd=heartbeat edition=python" in log.read_text()


def test_every_route_keeps_the_callers_directory_through_a_symlinked_launcher(
    tmp_path: Path,
) -> None:
    """#984: no launcher layer resolves the project root from its own location.

    A harness calls wrkslots through its own `bin/wrkslots` symlink from a
    slot directory.  Every route (the Python primary, the WRKSLOTS_ENGINE=rust
    opt-in, the soak and the per-process readers soak) must hand the edition
    the caller's working directory and add no --root, so each edition finds
    the root from the caller's directory upward, as `--root`-less discovery
    is specified.
    """

    root, log = _sandbox(tmp_path)
    (root / "py" / "bin" / "wrkslots").write_text(
        "import os, sys\nprint('PY cwd=' + os.getcwd() + ' args=' + ' '.join(sys.argv[1:]))\n"
    )
    (root / "rs" / "bin" / "wrkslotsd").write_text(
        "#!/bin/sh\n"
        "if [ \"$2\" = \"--help\" ]; then echo \"Usage: --root --machine --all-machines "
        "--format\"; exit 0; fi\n"
        "echo \"RS cwd=$(pwd -P) args=$*\"\n"
    )
    harness = tmp_path / "harness"
    slot = harness / "worktrees" / "slot-a" / "repo"
    slot.mkdir(parents=True)
    (harness / "bin").mkdir()
    launcher = harness / "bin" / "wrkslots"
    launcher.symlink_to(root / "common" / "bin" / "wrkslots")
    base = {
        key: value
        for key, value in os.environ.items()
        if key
        not in (
            "DAGRUN_ENGINE",
            "AGENT_UTILS_ENGINE",
            "WRKSLOTS_ENGINE",
            "WRKSLOTS_RUST_READERS",
            "WRKSLOTS_SOAK_READERS_ONLY",
            "CI_HUB_PYTHON",
        )
    }
    base["WRKSLOTS_RUST_READERS_FILE"] = str(tmp_path / "no-readers-file")
    base["WRKSLOTS_SOAK_LOG"] = str(log)
    routes = {
        "python": {},
        "rust": {"WRKSLOTS_ENGINE": "rust"},
        "rust-soak": {"WRKSLOTS_ENGINE": "rust-soak"},
        "readers": {"WRKSLOTS_RUST_READERS": "1"},
    }
    for route, extra in routes.items():
        result = subprocess.run(
            [str(launcher), "doctor", "--format", "json"],
            cwd=slot, capture_output=True, text=True, check=False, timeout=30,
            env={**base, **extra},
        )
        assert result.returncode == 0, (route, result.stderr)
        lines = [line for line in result.stdout.splitlines() if " cwd=" in line]
        assert len(lines) == 1, (route, result.stdout)
        assert f" cwd={slot.resolve()} " in lines[0], (route, lines[0])
        assert "--root" not in lines[0], (route, lines[0])


def test_no_build_cold_readers_serves_python(tmp_path: Path) -> None:
    """A no-build install never pays a lazy build for a reader: cold serves Python."""
    root, log = _sandbox(tmp_path)
    (root / "rust-cold").write_text("cold\n")
    result = _run(
        root,
        log,
        "status",
        readers_only=True,
        extra_env={"AGENT_UTILS_RS_NO_BUILD": "1"},
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout.startswith("PY wrkslots status")
    assert "rust snapshot not prebuilt" in result.stderr
    assert "cmd=status edition=python" in log.read_text()


def test_no_build_warm_readers_runs_rust(tmp_path: Path) -> None:
    root, log = _sandbox(tmp_path)
    result = _run(
        root,
        log,
        "status",
        readers_only=True,
        extra_env={"AGENT_UTILS_RS_NO_BUILD": "1"},
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout.startswith("RS status-text status-text")
    assert "cmd=status edition=rust rc=0" in log.read_text()


def test_no_build_cold_full_soak_refuses(tmp_path: Path) -> None:
    """Full soak asked for Rust by name: a cold no-build refusal is final, not a fallback."""
    root, log = _sandbox(tmp_path)
    (root / "rust-cold").write_text("cold\n")
    result = _run(
        root, log, "status", extra_env={"AGENT_UTILS_RS_NO_BUILD": "1"}
    )
    assert result.returncode == 127
    assert result.stdout == ""
    assert "refusing" in result.stderr
    assert "cmd=status edition=rust rc=127" in log.read_text()


def test_cold_without_no_build_still_dispatches_rust(tmp_path: Path) -> None:
    """Without the no-build contract the probe gate is inert (development checkouts build)."""
    root, log = _sandbox(tmp_path)
    (root / "rust-cold").write_text("cold\n")
    result = _run(root, log, "status", readers_only=True)
    assert result.returncode == 0, result.stderr
    assert result.stdout.startswith("RS status-text status-text")


def test_rust_disable_pulls_named_reader_back_to_python(tmp_path: Path) -> None:
    """WRKSLOTS_RUST_DISABLE is the per-command rollback, in readers and full soak."""
    root, log = _sandbox(tmp_path)
    readers = _run(
        root,
        log,
        "status",
        readers_only=True,
        extra_env={"WRKSLOTS_RUST_DISABLE": "status"},
    )
    assert readers.returncode == 0, readers.stderr
    assert readers.stdout.startswith("PY wrkslots status")
    assert "disabled by WRKSLOTS_RUST_DISABLE" in readers.stderr
    full = _run(root, log, "status", extra_env={"WRKSLOTS_RUST_DISABLE": "status"})
    assert full.stdout.startswith("PY wrkslots status")
    assert "cmd=status edition=python" in log.read_text()


def test_rust_disable_family_action_and_unrelated_names(tmp_path: Path) -> None:
    root, log = _sandbox(tmp_path)
    # image:status disabled; sandbox show-config (spaced list) untouched.
    image = _run(
        root,
        log,
        "image",
        "status",
        readers_only=True,
        extra_env={"WRKSLOTS_RUST_DISABLE": "doctor, image:status"},
    )
    assert image.stdout.startswith("PY wrkslots image status")
    sandbox = _run(
        root,
        log,
        "sandbox",
        "show-config",
        readers_only=True,
        extra_env={"WRKSLOTS_RUST_DISABLE": "doctor, image:status"},
    )
    assert sandbox.stdout.startswith("RS sandbox-show-config")
    # A bare family word disables its wired action too.
    bare = _run(
        root,
        log,
        "image",
        "status",
        readers_only=True,
        extra_env={"WRKSLOTS_RUST_DISABLE": "image"},
    )
    assert bare.stdout.startswith("PY wrkslots image status")


def _shadow_sandbox(tmp_path: Path) -> tuple[Path, Path, Path, Path]:
    """Sandbox whose Python and Rust status outputs agree after age scrubbing."""
    root, log = _sandbox(tmp_path)
    (root / "py" / "bin" / "wrkslots").write_text(
        "print('{\"active\": [], \"heartbeat_age_seconds\": 3}')\n"
    )
    _chmod_x(root / "py" / "bin" / "wrkslots")
    project = tmp_path / "proj"
    control = project / "worktrees"
    control.mkdir(parents=True)
    (project / ".wrkslots.yml").write_text("schema: 2\nworktrees_dir: worktrees\n")
    (control / "ACTIVE.node-a.json").write_text("{}\n")
    rust_stub = root / "rs" / "bin" / "wrkslotsd"
    rust_stub.write_text(
        "#!/bin/sh\n"
        'if [ "$AGENT_UTILS_RS_PROBE_ONLY" = "1" ]; then echo stub-snapshot; exit 0; fi\n'
        'case "$1" in\n'
        '  status-text)\n'
        '    if [ "$2" = "--help" ]; then echo "Usage: --root --machine --format"; exit 0; fi\n'
        f"    [ -f ./rs-mutate ] && printf 'x' >>\"{control}/ACTIVE.node-a.json\"\n"
        "    if [ -f ./rs-diverge ]; then echo '{\"active\": [{\"x\": 1}]}';\n"
        "    else echo '{\"active\": [], \"heartbeat_age_seconds\": 7}'; fi ;;\n"
        '  *) echo "RS $@" ;;\n'
        "esac\n"
    )
    _chmod_x(rust_stub)
    return root, log, project, tmp_path / "divergence.jsonl"


def _shadow_run(root, log, project, divergence, **env):
    return _run(
        root,
        log,
        "--project-root",
        str(project),
        "status",
        "--format",
        "json",
        readers_only=True,
        extra_env={"WRKSLOTS_DIVERGENCE_LOG": str(divergence), **env},
    )


def test_shadow_all_serves_python_and_logs_agreement(tmp_path: Path) -> None:
    root, log, project, divergence = _shadow_sandbox(tmp_path)
    result = _shadow_run(root, log, project, divergence, WRKSLOTS_SHADOW="all")
    assert result.returncode == 0, result.stderr
    # Served bytes are the Python edition's (age 3, not the Rust stub's 7).
    assert '"heartbeat_age_seconds": 3' in result.stdout
    assert "shadow-sampled" in result.stderr
    assert "cmd=status edition=python" in log.read_text()
    lines = divergence.read_text().splitlines()
    assert len(lines) == 1
    assert '"result": "agree"' in lines[0]


def test_shadow_logs_first_divergent_field(tmp_path: Path) -> None:
    root, log, project, divergence = _shadow_sandbox(tmp_path)
    (root / "rs-diverge").write_text("x\n")
    result = _shadow_run(root, log, project, divergence, WRKSLOTS_SHADOW="all")
    assert result.returncode == 0, result.stderr
    record = divergence.read_text()
    assert '"result": "diverge"' in record
    assert '"field": "active"' in record


def test_shadow_registry_change_is_skipped(tmp_path: Path) -> None:
    root, log, project, divergence = _shadow_sandbox(tmp_path)
    (root / "rs-mutate").write_text("x\n")
    result = _shadow_run(root, log, project, divergence, WRKSLOTS_SHADOW="all")
    assert result.returncode == 0, result.stderr
    assert '"reason": "registry-changed"' in divergence.read_text()


def test_shadow_sampled_n1_and_determinism(tmp_path: Path) -> None:
    root, log, project, divergence = _shadow_sandbox(tmp_path)
    first = _shadow_run(
        root, log, project, divergence,
        WRKSLOTS_SHADOW="sampled", WRKSLOTS_SHADOW_N="1",
    )
    assert "shadow-sampled" in first.stderr
    # Same argv, same sampling decision both times (whatever N=2 decides).
    one = _shadow_run(
        root, log, project, divergence,
        WRKSLOTS_SHADOW="sampled", WRKSLOTS_SHADOW_N="2",
    )
    two = _shadow_run(
        root, log, project, divergence,
        WRKSLOTS_SHADOW="sampled", WRKSLOTS_SHADOW_N="2",
    )
    assert ("shadow-sampled" in one.stderr) == ("shadow-sampled" in two.stderr)
