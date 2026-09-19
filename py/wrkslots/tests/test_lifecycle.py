"""Focused safety and lifecycle tests for the wrkslots distribution."""

from __future__ import annotations

import argparse
import contextlib
import fnmatch
import hashlib
import datetime as dt
import errno
import io
import itertools
import json
import os
import select
import selectors
import signal
import shlex
import shutil
import stat
import subprocess
import sys
import tempfile
import textwrap
import time
from collections.abc import Callable, Iterator, Mapping, Sequence, Set as AbstractSet
from dataclasses import replace
from pathlib import Path

import pytest


PACKAGE_ROOT = Path(__file__).resolve().parents[1]
PY_ROOT = PACKAGE_ROOT.parent
WRKSLOTS = PACKAGE_ROOT / "__main__.py"
COMPATIBILITY_COMMAND = PY_ROOT / "wrkslots.py"
sys.path.insert(0, str(PY_ROOT))
from wrkslots import cli as wrkslots  # noqa: E402


def source_environment(extra: dict[str, str] | None = None) -> dict[str, str]:
    environment = os.environ.copy()
    previous = environment.get("PYTHONPATH")
    environment["PYTHONPATH"] = (
        str(PY_ROOT) if not previous else f"{PY_ROOT}{os.pathsep}{previous}"
    )
    if extra:
        environment.update(extra)
    return environment


def git(path: Path, *args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    completed = subprocess.run(
        ["git", "-C", str(path), *args],
        text=True,
        capture_output=True,
        check=False,
    )
    if check and completed.returncode != 0:
        raise AssertionError(
            f"git {' '.join(args)} failed in {path}: {completed.stderr or completed.stdout}"
        )
    return completed


_FROZEN_RECORD_PARSER = r'''#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

import service_result


FIELDS = frozenset({
    "admission", "admission_result", "agent", "cargo_home", "checkout",
    "current_main_before_launch", "detail", "e2e_result_root", "executed_nodes",
    "executed_tests", "exit_code", "final_validate_status", "finished_at", "host",
    "kind", "log", "measured_against_superseded_tip",
    "observed_safe_ci_cgroups",
    "pane_id", "pane_role", "pane_title", "parent_checkout_head", "pr",
    "process_identity", "producer", "qualifying_receipt", "repo", "result",
    "result_record", "result_source", "safe_ci_dag_runner_log_dir", "schema_version",
    "scorecard_handoff", "scorecard_writeback", "scorecard_writeback_files",
    "selection_mode", "service_result_schema", "source_checkout",
    "started_at", "state", "tab_id", "target",
    "target_contains_current_main_before_launch", "temporary_checkout", "unit",
    "validate_lock_child_deadline_seconds", "validation_kind", "workspace_id",
})


def refuse(detail: str) -> None:
    print(f"run_registry: REFUSED: {detail}", file=sys.stderr)
    raise SystemExit(2)


parser = argparse.ArgumentParser()
parser.add_argument("command", choices=("inspect",))
parser.add_argument("--path", type=Path, required=True)
args = parser.parse_args()
try:
    value = json.loads(args.path.read_text(encoding="utf-8"))
except Exception as error:
    refuse(str(error))
if not isinstance(value, dict) or set(value) != FIELDS:
    refuse("record does not have the canonical frozen-validation field set")
if (
    value.get("schema_version") != 1
    or value.get("kind") != "frozen-validate"
    or value.get("producer") != "ci-hub/validate/run_registry.py"
    or value.get("admission") != "frozen-validate"
    or value.get("validation_kind") != "frozen-validate"
    or value.get("pane_role") != "observer-only"
    or value.get("temporary_checkout") is not True
    or value.get("measured_against_superseded_tip") is not True
    or value.get("target_contains_current_main_before_launch") is not False
    or value.get("qualifying_receipt") is not False
    or value.get("service_result_schema") != service_result.SELECTION_SCHEMA_VERSION
):
    refuse("record violates the canonical frozen-validation schema")
for field in (
    "unit", "target", "checkout", "source_checkout", "cargo_home", "log", "agent",
    "started_at", "result_record", "e2e_result_root", "safe_ci_dag_runner_log_dir",
):
    if not isinstance(value.get(field), str) or not value[field]:
        refuse(f"{field} must be non-empty text")
if not value["unit"].endswith(".service") or len(value["target"]) != 40:
    refuse("unit or target is invalid")
if value.get("admission_result", {}).get("state") != "admitted":
    refuse("admission result is invalid")
state = value.get("state")
if state == "completed":
    outcomes = {
        "PASSED": ("success", 0),
        "FAILED": ("failure", 1),
        "COULD_NOT_RUN": ("no-result", 75),
    }
    if outcomes.get(value.get("final_validate_status")) != (
        value.get("result"), value.get("exit_code")
    ):
        refuse("completed outcome is inconsistent")
elif state == "unknown":
    if value.get("result") != "unknown" or not value.get("detail"):
        refuse("unknown outcome is inconsistent")
elif state == "killed":
    if value.get("result") != "terminated" or value.get("exit_code") != 143:
        refuse("killed outcome is inconsistent")
elif state == "refused":
    if value.get("result") != "systemd-launch-refused" or value.get("exit_code") == 0:
        refuse("refused outcome is inconsistent")
else:
    refuse("unsupported state")
print(json.dumps({
    "schema_version": value["schema_version"],
    "kind": value["kind"],
    "state": value["state"],
    "unit": value["unit"],
    "target": value["target"],
    "repo": value["repo"],
    "branch": value.get("branch"),
    "agent": value["agent"],
    "started_at": value["started_at"],
    "host": value.get("host"),
    "process_identity": value.get("process_identity"),
    "admission_result": value.get("admission_result"),
    "termination_requests": value.get("termination_requests", []),
    "lock_admissible": state in {"launching", "running"},
    "counts_as_validation": True,
}, sort_keys=True, separators=(",", ":")))
'''

_FROZEN_SERVICE_RESULT = '''\
from __future__ import annotations
from final_validate_status import FINAL_VALIDATE_STATUS_EXIT_CODES
HISTORICAL_SCHEMA_VERSION = 1
WRITEBACK_SCHEMA_VERSION = 2
SELECTION_SCHEMA_VERSION = 3
SCHEMA_VERSION = 4
'''

_FROZEN_FINAL_VALIDATE_STATUS = '''\
from __future__ import annotations
FINAL_VALIDATE_STATUS_EXIT_CODES = {"PASSED": 0, "FAILED": 1, "COULD_NOT_RUN": 75}
'''


def install_frozen_validation_parser(
    project: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Install a tracked three-module producer boundary for isolated fixture roots."""

    parser_directory = project / "ci-hub" / "validate"
    parser_directory.mkdir(parents=True, exist_ok=True)
    (parser_directory / "run_registry.py").write_text(
        textwrap.dedent(_FROZEN_RECORD_PARSER), encoding="utf-8"
    )
    (parser_directory / "service_result.py").write_text(
        textwrap.dedent(_FROZEN_SERVICE_RESULT), encoding="utf-8"
    )
    (parser_directory / "final_validate_status.py").write_text(
        textwrap.dedent(_FROZEN_FINAL_VALIDATE_STATUS), encoding="utf-8"
    )
    if not (project / ".git").exists():
        git(project, "init", "--initial-branch=main")
        git(project, "config", "user.name", "Wrkslots Test")
        git(project, "config", "user.email", "wrkslots@example.invalid")
    git(project, "add", "--", "ci-hub/validate/run_registry.py", "ci-hub/validate/service_result.py", "ci-hub/validate/final_validate_status.py")
    if git(project, "diff", "--cached", "--quiet", check=False).returncode != 0:
        git(project, "commit", "-m", "install frozen validation parser fixture")

    production_authority = wrkslots._frozen_validation_authority_commit

    def fixture_authority(config: wrkslots.Config) -> tuple[Path, str]:
        if config.root == project:
            return project, git(project, "rev-parse", "HEAD").stdout.strip()
        return production_authority(config)

    monkeypatch.setattr(wrkslots, "_frozen_validation_authority_commit", fixture_authority)


def command(
    project: Path,
    *args: str,
    machine: str | None = None,
    env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    command_args = list(args)
    if command_args and command_args[0] in {"create", "register", "import-existing"}:
        if "--slot-type" not in command_args:
            command_args.extend(("--slot-type", "agent"))
        if "--coordinator-authorized" not in command_args:
            command_args.append("--coordinator-authorized")
    if command_args and command_args[0] in {"remove", "recover"}:
        if "--coordinator-authorized" not in command_args:
            command_args.append("--coordinator-authorized")
    return raw_command(project, *command_args, machine=machine, env=env)


def raw_command(
    project: Path,
    *args: str,
    machine: str | None = None,
    env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    argv = [sys.executable, "-m", "wrkslots", "--project-root", str(project)]
    if machine is not None:
        argv.extend(("--machine", machine))
    argv.extend(args)
    process_env = source_environment(env)
    return subprocess.run(
        argv,
        text=True,
        capture_output=True,
        check=False,
        env=process_env,
    )


_VALIDATION_EXCLUSION_GUARD_PREFIX = ".wrkslots-ownerless-validation."
_MAPPED_VALIDATION_EXCLUSION_PROJECTS: set[Path] = set()


def validation_exclusion_guard_identities(
    roots: Sequence[Path],
) -> dict[Path, tuple[int, int, int, int, int]]:
    """Snapshot exclusion guards without searching unrelated temporary trees."""

    result: dict[Path, tuple[int, int, int, int, int]] = {}
    for root in roots:
        if not root.is_dir():
            continue
        for child in root.iterdir():
            if not child.name.startswith(_VALIDATION_EXCLUSION_GUARD_PREFIX):
                continue
            metadata = child.stat(follow_symlinks=False)
            result[child] = (
                metadata.st_dev,
                metadata.st_ino,
                stat.S_IMODE(metadata.st_mode),
                metadata.st_uid,
                metadata.st_gid,
            )
    return result


def install_validation_exclusion_fixture(
    project: Path,
    monkeypatch: pytest.MonkeyPatch,
    request: pytest.FixtureRequest,
) -> Path:
    """Install a trusted-root fixture only in this test interpreter.

    The fast local lifecycle partition runs as root in a mapped user namespace,
    while hosted CI can forbid that namespace and fall back to an ordinary
    identity-namespace process.  In the latter environment, use the same
    passwordless, root-owned command boundary as production instead of assuming
    that ``/data/misc`` exists or that the test interpreter is namespace root.
    """

    identity_namespace = wrkslots._has_identity_user_namespace()
    mapped_namespace_root = os.geteuid() == 0 and not identity_namespace
    identity_namespace_user = os.geteuid() != 0 and identity_namespace
    if not mapped_namespace_root and not identity_namespace_user:
        raise AssertionError(
            "validation exclusion fixture requires mapped namespace root or an "
            "ordinary user in the initial identity namespace"
        )
    token = hashlib.sha256(os.fsencode(str(project))).hexdigest()[:24]
    root = Path("/tmp") / f".wrkslots-validation-exclusion.{token}"
    if project in _MAPPED_VALIDATION_EXCLUSION_PROJECTS:
        return root
    if mapped_namespace_root:
        root.mkdir(mode=0o1777)
        root.chmod(0o1777)
    else:
        wrkslots._root_validation_exclusion_command(
            Path("/usr/bin/mkdir"),
            ("--mode=1777", "--", str(root)),
            "create the validation exclusion fixture root",
        )

    setup_complete = False
    observed_roots: tuple[Path, ...] = (root,)
    guards_before: dict[Path, tuple[int, int, int, int, int]] = {}

    def remove_empty_fixture_root() -> None:
        if not root.exists():
            return
        assert root.is_dir() and not root.is_symlink()
        entries = tuple(root.iterdir())
        assert entries == (), (
            "refusing to remove nonempty validation exclusion fixture root: "
            f"{root}: {entries}"
        )
        if mapped_namespace_root:
            root.rmdir()
        else:
            wrkslots._root_validation_exclusion_command(
                Path("/usr/bin/rmdir"),
                ("--", str(root)),
                "remove the validation exclusion fixture root",
            )
        assert not root.exists()

    def restore_created_guard() -> None:
        try:
            if setup_complete:
                guards_during_teardown = validation_exclusion_guard_identities(
                    observed_roots
                )
                created = set(guards_during_teardown).difference(guards_before)
                assert all(guard.parent == root for guard in created)
                if created:
                    assert len(created) == 1
                    guard = created.pop()
                    journal_path = control_directory(project) / "ACTIVE.testhost.journal"
                    assert journal_path.is_file()
                    journal_value = json.loads(
                        journal_path.read_text(encoding="utf-8")
                    )
                    assert isinstance(journal_value, dict)
                    config = wrkslots._load_config(str(project), "testhost")
                    _authorization, _target, fenced, phase = (
                        wrkslots._ownerless_validation_paths(config, journal_value)
                    )
                    assert phase == "removing"
                    exclusion_value = journal_value.get("exclusion")
                    assert exclusion_value is not None
                    exclusion = wrkslots._ownerless_validation_exclusion_from_obj(
                        config, fenced, exclusion_value
                    )
                    assert exclusion.guard == guard
                    assert (
                        guards_during_teardown[guard][:2]
                        == exclusion.guard_identity[:2]
                    )
                    restored = wrkslots._restore_ownerless_validation_exclusion(
                        config, journal_value
                    )
                    assert "exclusion" not in restored
                assert (
                    validation_exclusion_guard_identities(observed_roots)
                    == guards_before
                )
            remove_empty_fixture_root()
        finally:
            _MAPPED_VALIDATION_EXCLUSION_PROJECTS.discard(project)

    # Register cleanup immediately after creation.  If later fixture setup
    # fails, an empty root is removed; any unexpected contents are retained.
    request.addfinalizer(restore_created_guard)

    metadata = root.stat(follow_symlinks=False)
    assert metadata.st_uid == 0
    assert metadata.st_gid == 0
    assert stat.S_IMODE(metadata.st_mode) == 0o1777
    mount_rows = wrkslots._self_mountinfo_records()
    deduplicated: dict[
        tuple[str, Path, Path, str, str],
        tuple[int, str, Path, Path, str, str],
    ] = {}
    for row in mount_rows:
        deduplicated[(row[1], row[2], row[3], row[4], row[5])] = row

    production_roots = wrkslots._OWNERLESS_VALIDATION_EXCLUSION_ROOTS
    observed_roots = (*production_roots, root)
    assert all(
        candidate == Path("/data/misc") or candidate.is_relative_to(Path("/tmp"))
        for candidate in observed_roots
    )
    guards_before = validation_exclusion_guard_identities(observed_roots)

    def run_as_namespace_root(
        program: Path,
        arguments: Sequence[str],
        *,
        input_data: bytes | None = None,
        budget: wrkslots._ReadOnlyCommandBudget | None = None,
    ) -> tuple[int, bytes, bytes]:
        selected_budget = budget or wrkslots._ReadOnlyCommandBudget.start(
            timeout_seconds=60,
            stdout_limit=16 * 1024 * 1024,
            stderr_limit=64 * 1024,
        )
        selected_budget.reserve_input(0 if input_data is None else len(input_data))
        completed = subprocess.run(
            [str(program), *arguments],
            input=input_data,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
            cwd="/",
            env={"PATH": "/usr/sbin:/usr/bin:/sbin:/bin", "LC_ALL": "C"},
            timeout=selected_budget.remaining_seconds(),
        )
        selected_budget.consume_output(completed.stdout, completed.stderr)
        return completed.returncode, completed.stdout, completed.stderr

    monkeypatch.setattr(wrkslots, "_OWNERLESS_VALIDATION_EXCLUSION_ROOTS", (root,))
    monkeypatch.setattr(
        wrkslots, "_self_mountinfo_records", lambda: tuple(deduplicated.values())
    )
    if mapped_namespace_root:
        monkeypatch.setattr(
            wrkslots, "_run_root_owned_command", run_as_namespace_root
        )
        monkeypatch.setattr(
            wrkslots, "_run_same_uid_command", run_as_namespace_root
        )
    _MAPPED_VALIDATION_EXCLUSION_PROJECTS.add(project)
    setup_complete = True
    return root


def validation_exclusion_command_arguments(
    project: Path,
    args: Sequence[str],
    *,
    add_coordinator_authorization: bool,
) -> list[str]:
    command_args = list(args)
    if add_coordinator_authorization:
        assert "--coordinator-authorized" not in command_args
        command_args.append("--coordinator-authorized")
    return ["--project-root", str(project), *command_args]


def in_process_validation_exclusion_command(
    project: Path,
    monkeypatch: pytest.MonkeyPatch,
    request: pytest.FixtureRequest,
    *args: str,
    add_coordinator_authorization: bool,
) -> subprocess.CompletedProcess[str]:
    """Run cleanup with an isolated trusted-root fixture."""

    root = install_validation_exclusion_fixture(project, monkeypatch, request)
    argv = validation_exclusion_command_arguments(
        project,
        args,
        add_coordinator_authorization=add_coordinator_authorization,
    )
    assert str(root) not in argv
    stdout = io.StringIO()
    stderr = io.StringIO()
    with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
        returncode = wrkslots.main(argv)
    return subprocess.CompletedProcess(
        [sys.executable, "-m", "wrkslots", *argv],
        returncode,
        stdout.getvalue(),
        stderr.getvalue(),
    )


def forked_validation_exclusion_command(
    project: Path,
    monkeypatch: pytest.MonkeyPatch,
    request: pytest.FixtureRequest,
    *args: str,
    interrupt: str,
    add_coordinator_authorization: bool,
) -> subprocess.CompletedProcess[str]:
    """Crash an in-process cleanup without crossing an exec boundary."""

    root = install_validation_exclusion_fixture(project, monkeypatch, request)
    argv = validation_exclusion_command_arguments(
        project,
        args,
        add_coordinator_authorization=add_coordinator_authorization,
    )
    assert str(root) not in argv
    child = os.fork()
    if child == 0:
        os.environ["WRKSLOTS_TEST_INTERRUPT"] = interrupt
        try:
            returncode = wrkslots.main(argv)
        except BaseException:
            os._exit(255)
        os._exit(returncode)
    _pid, status = os.waitpid(child, 0)
    return subprocess.CompletedProcess(
        [sys.executable, "-m", "wrkslots", *argv],
        os.waitstatus_to_exitcode(status),
        "",
        "",
    )


def raw_command_with_census_authority_stub(
    project: Path,
    *args: str,
    env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    """Run one crash/recovery boundary without depending on host-root helpers."""

    launcher = """
import os
import subprocess
import sys
from pathlib import Path
from wrkslots import cli

def test_slot_unused(slot_path, _record=None, **_kwargs):
    for pid_dir in Path('/proc').iterdir():
        if not pid_dir.name.isdigit() or int(pid_dir.name) == os.getpid():
            continue
        try:
            observed = Path(os.readlink(pid_dir / 'cwd'))
        except OSError:
            continue
        if observed == slot_path or slot_path in observed.parents:
            raise cli.Refusal(
                f'live process {pid_dir.name} uses slot {slot_path}: cwd={observed}'
            )

def test_census(paths, **_kwargs):
    for path in paths:
        test_slot_unused(path)
    return cli._ProcessPathCensus((), ())

def test_same_uid_census(_paths, **_kwargs):
    return cli._ProcessPathCensus((), ())

def test_frozen_authority(config):
    completed = subprocess.run(
        ['git', '-C', str(config.root), 'rev-parse', 'HEAD'],
        text=True,
        capture_output=True,
        check=False,
    )
    if completed.returncode != 0:
        return cli._production_frozen_validation_authority_commit(config)
    return config.root, completed.stdout.strip()

cli._assert_slot_unused = test_slot_unused
cli._capture_process_path_census = test_census
cli._capture_lsof_process_path_census = test_census
cli._capture_same_uid_process_path_census = test_same_uid_census
cli._production_frozen_validation_authority_commit = cli._frozen_validation_authority_commit
cli._frozen_validation_authority_commit = test_frozen_authority
cli._OWNERLESS_VALIDATION_EXCLUSION_ROOTS = (Path('/tmp'),)
raise SystemExit(cli.main(sys.argv[1:]))
"""
    argv = [
        sys.executable,
        "-c",
        launcher,
        "--project-root",
        str(project),
        *args,
    ]
    return subprocess.run(
        argv,
        text=True,
        capture_output=True,
        check=False,
        env=source_environment(env),
    )


def self_coordinator_command(
    project: Path,
    *args: str,
    env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    argv = [
        sys.executable,
        "-m",
        "wrkslots",
        "--project-root",
        str(project),
        *args,
    ]
    launcher = (
        "import os, sys; argv = sys.argv[1:] + "
        "['--coordinator-pid', str(os.getpid())]; os.execv(argv[0], argv)"
    )
    return subprocess.run(
        [sys.executable, "-c", launcher, *argv],
        text=True,
        capture_output=True,
        check=False,
        env=source_environment(env),
    )


def terminate_process(process: subprocess.Popen[str]) -> None:
    """Stop one fixture process even when PID-namespace SIGTERM is delayed."""
    if process.poll() is not None:
        return
    process.send_signal(signal.SIGTERM)
    try:
        process.wait(timeout=2)
    except subprocess.TimeoutExpired:
        process.send_signal(signal.SIGKILL)
        process.wait(timeout=10)


def spawn_process_outside_coordinator() -> int:
    """Start a live same-user process whose current ancestry excludes this test."""
    launcher = (
        "import os, subprocess; "
        "child = subprocess.Popen(['sleep', '60'], stdin=subprocess.DEVNULL, "
        "stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL); "
        "print(os.getpid(), child.pid, flush=True)"
    )
    completed = subprocess.run(
        [sys.executable, "-c", launcher],
        text=True,
        capture_output=True,
        check=False,
    )
    assert completed.returncode == 0, completed.stderr
    launcher_pid, process_pid = (int(value) for value in completed.stdout.split())
    deadline = time.monotonic() + 2
    while time.monotonic() < deadline:
        if wrkslots._read_process_parent(process_pid) != launcher_pid:
            return process_pid
        time.sleep(0.01)
    raise AssertionError(f"process {process_pid} was not reparented")


def terminate_process_id(process_id: int) -> None:
    try:
        os.kill(process_id, signal.SIGTERM)
    except ProcessLookupError:
        return
    deadline = time.monotonic() + 2
    while time.monotonic() < deadline:
        if not Path(f"/proc/{process_id}").exists():
            return
        time.sleep(0.01)
    try:
        os.kill(process_id, signal.SIGKILL)
    except ProcessLookupError:
        pass


def initialize(
    project: Path,
    *,
    machine: str = "testhost",
    directory: str = "worktrees",
    layout: str | None = None,
    cache_globs: tuple[str, ...] = (),
    repo_cache_globs: tuple[tuple[str, str], ...] = (),
    post_provision_hooks: tuple[str, ...] = (),
    disk_thresholds_gib: tuple[int, int, int] | None = None,
) -> None:
    liveness = project / "liveness.py"
    if not liveness.exists():
        liveness.write_text(
            "#!/usr/bin/env python3\n"
            "import os, pathlib, sys\n"
            "root = pathlib.Path(os.environ['WRKSLOTS_PROJECT_ROOT'])\n"
            "state = (root / 'liveness-result').read_text().strip()\n"
            "codes = {'dead': 0, 'alive': 1, 'unverifiable': 2}\n"
            "print(state)\n"
            "raise SystemExit(codes.get(state, 3))\n",
            encoding="utf-8",
        )
        liveness.chmod(0o755)
    (project / "liveness-result").write_text("alive\n", encoding="utf-8")
    argv = [
        sys.executable,
        str(WRKSLOTS),
        "--machine",
        machine,
        "init",
        str(project),
        "--worktrees-dir",
        directory,
        "--liveness-command",
        "liveness.py",
    ]
    if layout is not None:
        argv.extend(("--layout", layout))
    for cache_glob in cache_globs:
        argv.extend(("--cache-glob", cache_glob))
    for name, cache_glob in repo_cache_globs:
        argv.extend(("--repo-cache-glob", f"{name}={cache_glob}"))
    for hook in post_provision_hooks:
        argv.extend(("--post-provision-hook", hook))
    if disk_thresholds_gib is not None:
        advisory, provisioning_floor, emergency = disk_thresholds_gib
        argv.extend(
            (
                "--disk-advisory-gib",
                str(advisory),
                "--disk-provisioning-floor-gib",
                str(provisioning_floor),
                "--disk-emergency-gib",
                str(emergency),
            )
        )
    completed = subprocess.run(
        argv,
        text=True,
        capture_output=True,
        check=False,
        env=source_environment(),
    )
    assert completed.returncode == 0, completed.stderr


def make_project(
    tmp_path: Path,
    *,
    machine: str = "testhost",
    worktrees_directory: str = "worktrees",
    layout: str | None = None,
    cache_globs: tuple[str, ...] = (),
    repo_cache_globs: tuple[tuple[str, str], ...] = (),
    post_provision_hooks: tuple[str, ...] = (),
    disk_thresholds_gib: tuple[int, int, int] | None = None,
) -> tuple[Path, Path, Path]:
    remote = tmp_path / "remote.git"
    project = tmp_path / "project"
    repository = project / "repo"
    project.mkdir()
    subprocess.run(
        ["git", "init", "--bare", "--initial-branch=main", str(remote)],
        check=True,
        capture_output=True,
        text=True,
    )
    subprocess.run(
        ["git", "clone", str(remote), str(repository)],
        check=True,
        capture_output=True,
        text=True,
    )
    git(repository, "config", "user.name", "Wrkslots Test")
    git(repository, "config", "user.email", "wrkslots@example.invalid")
    (repository / "seed.txt").write_text("seed\n", encoding="utf-8")
    git(repository, "add", "seed.txt")
    git(repository, "commit", "-m", "seed")
    git(repository, "push", "-u", "origin", "main")
    initialize(
        project,
        machine=machine,
        directory=worktrees_directory,
        layout=layout,
        cache_globs=cache_globs,
        repo_cache_globs=repo_cache_globs,
        post_provision_hooks=post_provision_hooks,
        disk_thresholds_gib=disk_thresholds_gib,
    )
    return project, repository, remote


def add_recursive_submodules(
    tmp_path: Path, project: Path, repository: Path
) -> tuple[str, str]:
    leaf_remote = tmp_path / "leaf.git"
    leaf_source = tmp_path / "leaf-source"
    component_remote = tmp_path / "component.git"
    component_source = tmp_path / "component-source"
    for remote, source in (
        (leaf_remote, leaf_source),
        (component_remote, component_source),
    ):
        subprocess.run(
            ["git", "init", "--bare", "--initial-branch=main", str(remote)],
            check=True,
            capture_output=True,
            text=True,
        )
        subprocess.run(
            ["git", "clone", str(remote), str(source)],
            check=True,
            capture_output=True,
            text=True,
        )
        git(source, "config", "user.name", "Wrkslots Test")
        git(source, "config", "user.email", "wrkslots@example.invalid")
    (leaf_source / "leaf.txt").write_text("leaf sentinel\n", encoding="utf-8")
    git(leaf_source, "add", "leaf.txt")
    git(leaf_source, "commit", "-m", "leaf base")
    git(leaf_source, "push", "-u", "origin", "main")
    git(
        component_source,
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        str(leaf_remote),
        "leaf",
    )
    (component_source / "component.txt").write_text(
        "component sentinel\n", encoding="utf-8"
    )
    git(component_source, "add", ".gitmodules", "component.txt", "leaf")
    git(component_source, "commit", "-m", "component with leaf")
    git(component_source, "push", "-u", "origin", "main")
    git(
        repository,
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        str(component_remote),
        "component",
    )
    git(repository, "commit", "-am", "add recursive component")
    git(repository, "push", "origin", "main")
    update_configuration(
        project,
        post_provision_hooks=[
            "git -c protocol.file.allow=always submodule update --init --recursive"
        ],
    )
    git(
        repository,
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "update",
        "--init",
        "--recursive",
    )
    component_head = git(repository / "component", "rev-parse", "HEAD").stdout.strip()
    leaf_head = git(repository / "component" / "leaf", "rev-parse", "HEAD").stdout.strip()
    return component_head, leaf_head


def submodule_peer_snapshot(repository: Path, peer: Path) -> tuple[object, ...]:
    common = Path(
        git(
            repository,
            "rev-parse",
            "--path-format=absolute",
            "--git-common-dir",
        ).stdout.strip()
    )
    roots = (repository, peer)
    statuses = tuple(git(root, "submodule", "status", "--recursive").stdout for root in roots)
    assert statuses and all(status.strip() for status in statuses)
    assert all(
        not line.startswith("-")
        for status in statuses
        for line in status.splitlines()
    )
    return (
        (common / "config").read_bytes(),
        statuses,
        tuple(git(root / "component", "rev-parse", "HEAD").stdout.strip() for root in roots),
        tuple(
            git(root / "component" / "leaf", "rev-parse", "HEAD").stdout.strip()
            for root in roots
        ),
        tuple((root / "component" / "component.txt").read_bytes() for root in roots),
        tuple((root / "component" / "leaf" / "leaf.txt").read_bytes() for root in roots),
    )


def create(
    project: Path,
    *,
    slot: str = "slot01",
    agent: str = "codex-1",
    branch: str | None = "codex/task",
    repository_name: str = "repo",
    checkout_name: str = "product",
    slot_type: str = "agent",
    machine: str | None = None,
    owner_pid: int | None = None,
    bind_owner: bool = True,
    env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    pid = os.getpid() if owner_pid is None else owner_pid
    owner_args = ["--owner-pid", str(pid)] if bind_owner else []
    branch_args = [] if branch is None else ["--branch", f"{checkout_name}={branch}"]
    return command(
        project,
        "create",
        slot,
        "--slot-type",
        slot_type,
        "--agent",
        agent,
        "--task",
        f"task-{slot}",
        "--purpose",
        f"test {slot}",
        *owner_args,
        "--coordinator-pid",
        str(os.getpid()),
        "--repo",
        f"{checkout_name}={repository_name}",
        *branch_args,
        machine=machine,
        env=env,
    )


def configuration(project: Path) -> dict[str, object]:
    value: object = json.loads(
        (project / ".wrkslots.yml").read_text(encoding="utf-8")
    )
    assert isinstance(value, dict)
    return value


def update_configuration(project: Path, **updates: object) -> None:
    path = project / ".wrkslots.yml"
    config = configuration(project)
    config.update(updates)
    path.write_text(
        json.dumps(config, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )


def slots_directory(project: Path) -> Path:
    configured = configuration(project)["worktrees_dir"]
    assert isinstance(configured, str)
    return project / configured


def control_directory(project: Path) -> Path:
    config = configuration(project)
    slots = slots_directory(project)
    return slots.parent if config.get("layout", "nested") == "flat" else slots


def create_journal_path(
    project: Path, slot: str = "slot01", machine: str = "testhost"
) -> Path:
    config = wrkslots._load_config(str(project), machine)
    return wrkslots._create_journal_path(config, slot, machine)


def finish_journal_path(
    project: Path, slot: str = "slot01", machine: str = "testhost"
) -> Path:
    config = wrkslots._load_config(str(project), machine)
    return wrkslots._finish_journal_path(config, slot, machine)


def install_legacy_create_journal_with_matching_event(
    project: Path, slot: str = "slot01", machine: str = "testhost"
) -> Path:
    config = wrkslots._load_config(str(project), machine)
    scoped = create_journal_path(project, slot, machine)
    raw: object = json.loads(scoped.read_text(encoding="utf-8"))
    assert isinstance(raw, dict)
    legacy = wrkslots._journal_path(config, machine)
    os.replace(scoped, legacy)
    shutil.rmtree(wrkslots._event_directory(config, machine))
    wrkslots._ensure_event_log(config, machine)
    wrkslots._write_event_file(
        config,
        machine,
        "operation-progress-recorded",
        {
            "slot": raw["slot"],
            "operation": raw["kind"],
            "journal": raw,
        },
    )
    return legacy


def active(project: Path, machine: str = "testhost") -> dict[str, object]:
    path = control_directory(project) / f"ACTIVE.{machine}.json"
    value: object = json.loads(path.read_text(encoding="utf-8"))
    assert isinstance(value, dict)
    return value


def active_slots(project: Path, machine: str = "testhost") -> list[object]:
    value = active(project, machine)["slots"]
    assert isinstance(value, list)
    return value


def checkout(
    project: Path,
    slot: str = "slot01",
    name: str = "product",
    slot_type: str = "agent",
) -> Path:
    slots = slots_directory(project)
    slot_root = slots
    if slot_type == "validate":
        slot_root = slots.parent / "validate" if slots.name == "slots" else slots / "validate"
    if configuration(project).get("layout", "nested") == "flat":
        return slot_root / slot
    return slot_root / slot / name


def commit_task(repository: Path, worktree: Path, branch: str) -> str:
    git(worktree, "config", "user.name", "Wrkslots Test")
    git(worktree, "config", "user.email", "wrkslots@example.invalid")
    (worktree / "task.txt").write_text(f"{branch}\n", encoding="utf-8")
    git(worktree, "add", "task.txt")
    git(worktree, "commit", "-m", "task")
    head = git(worktree, "rev-parse", "HEAD").stdout.strip()
    git(worktree, "push", "-u", "origin", branch)
    git(repository, "merge", "--no-ff", branch, "-m", "land task")
    git(repository, "push", "origin", "main")
    git(worktree, "fetch", "origin", "main")
    return head


def commit_local(worktree: Path, filename: str, content: str, subject: str) -> str:
    (worktree / filename).write_text(content, encoding="utf-8")
    git(worktree, "add", filename)
    git(worktree, "commit", "-m", subject)
    return git(worktree, "rev-parse", "HEAD").stdout.strip()


def python_hook(source: str) -> str:
    return f"{shlex.quote(sys.executable)} -c {shlex.quote(source)}"


def finish(
    project: Path,
    slot: str = "slot01",
    agent: str = "codex-1",
    env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    return command(
        project,
        "finish",
        slot,
        "--agent",
        agent,
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        "--validation",
        "pytest",
        env=env,
    )


def set_liveness(project: Path, state: str) -> None:
    (project / "liveness-result").write_text(f"{state}\n", encoding="utf-8")


def remove(project: Path, slot: str = "slot01") -> subprocess.CompletedProcess[str]:
    return command(
        project,
        "remove",
        slot,
        "--coordinator-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
    )


def mark_owner_dead(
    project: Path,
    machine: str = "testhost",
    *,
    expire: bool = True,
    slot: str | None = None,
) -> None:
    config = wrkslots._load_config(str(project), machine)
    state = wrkslots._load_active(config)
    record = state.slots[0] if slot is None else wrkslots._find_record(state, slot)
    assert record.owner is not None
    owner = replace(
        record.owner,
        boot_id="finished-boot",
        cgroup_path="/wrkslots-test-finished",
    )
    heartbeat_at = record.heartbeat_at
    if expire:
        heartbeat_at = (
            dt.datetime.now(dt.timezone.utc)
            - dt.timedelta(seconds=record.heartbeat_ttl_seconds + 1)
        ).isoformat(timespec="seconds")
    updated = replace(
        record, owner=owner, heartbeat_at=heartbeat_at
    )
    wrkslots._write_active_state(
        config,
        wrkslots._replace_record(state, updated),
        action="test-owner-exited",
        slot=record.slot,
    )


def mark_owner_dead_in_current_cgroup(
    project: Path, machine: str = "testhost", *, expire: bool = True
) -> None:
    """Make owner identity provably dead while retaining its real shared cgroup."""
    config = wrkslots._load_config(str(project), machine)
    state = wrkslots._load_active(config)
    record = state.slots[0]
    assert record.owner is not None
    heartbeat_at = record.heartbeat_at
    if expire:
        heartbeat_at = (
            dt.datetime.now(dt.timezone.utc)
            - dt.timedelta(seconds=record.heartbeat_ttl_seconds + 1)
        ).isoformat(timespec="seconds")
    updated = replace(
        record,
        owner=replace(record.owner, boot_id="finished-boot"),
        heartbeat_at=heartbeat_at,
    )
    wrkslots._write_active_state(
        config,
        wrkslots._replace_record(state, updated),
        action="test-owner-exited-in-shared-cgroup",
        slot=record.slot,
    )


def prepare_dead_validate_slots(project: Path, slots: Sequence[str]) -> dict[str, Path]:
    """Create validation slots whose exact recorded owners are provably dead."""

    for slot in slots:
        made = create(project, slot=slot, slot_type="validate", branch=None)
        assert made.returncode == 0, made.stderr
    config = wrkslots._load_config(str(project), "testhost")
    for slot in slots:
        state = wrkslots._load_active(config)
        record = next(item for item in state.slots if item.slot == slot)
        assert record.owner is not None
        wrkslots._write_active_state(
            config,
            wrkslots._replace_record(
                state,
                replace(record, owner=replace(record.owner, boot_id="finished-boot")),
            ),
            action="test-owner-exited",
            slot=slot,
        )
    set_liveness(project, "dead")
    return {
        slot: wrkslots._slot_directory(config, slot, "validate")
        for slot in slots
    }


def prepare_validation_removal_proof(
    project: Path,
    slot: str,
    *,
    generation: int = 1,
    checkout_path: Path | None = None,
    run_record: Path | None = None,
    producer_schema: Path | None = None,
) -> tuple[Path, dict[str, Path]]:
    """Create one exact four-artifact proof for a managed validation checkout."""

    registered_checkout = checkout_path is None
    if checkout_path is None:
        checkout_path = checkout(project, slot=slot, slot_type="validate")
    if run_record is None:
        run_record = (
            project / "ignored" / "validate" / "runs" / f"validate-{slot}.json"
        )
    if producer_schema is None:
        producer_schema = (
            checkout_path
            / "ci"
            / "manifest-plan"
            / "validation-service-result-schema.json"
        )
    schema_value: object = json.loads(producer_schema.read_text(encoding="utf-8"))
    assert isinstance(schema_value, dict)
    schema_version = schema_value.get("schema_version")
    assert isinstance(schema_version, int) and not isinstance(schema_version, bool)

    if run_record.exists():
        record_value: object = json.loads(run_record.read_text(encoding="utf-8"))
        assert isinstance(record_value, dict)
        record = record_value
    else:
        record = {}
    unit = record.get("unit", f"validate-{slot}.service")
    assert isinstance(unit, str) and unit.startswith("validate-")
    unit_name = unit.removesuffix(".service")
    assert run_record.name == f"{unit_name}.json"
    target = git(checkout_path, "rev-parse", "HEAD").stdout.strip()
    handoff_directory = (
        project
        / "ignored"
        / "validate"
        / "scorecard-writebacks"
        / unit_name
    )
    scorecard_payloads = {
        "SCORECARD.md": b"# Validation scorecard\n\nExact test fixture.\n",
        "ci/compat-envelope/cells.json": b'{"cells":[]}\n',
    }
    scorecard_rows: list[dict[str, object]] = []
    for relative, payload in scorecard_payloads.items():
        path = handoff_directory / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(payload)
        scorecard_rows.append(
            {
                "path": relative,
                "sha256": hashlib.sha256(payload).hexdigest(),
                "size": len(payload),
            }
        )

    historical = schema_version in {1, 2, 3}
    record.update(
        {
            "schema_version": 1,
            "kind": record.get("kind", "validate"),
            "producer": "ci-hub/validate/run_registry.py",
            "admission": record.get("admission", "ci-hub validate-lock"),
            "admission_result": record.get(
                "admission_result",
                {
                    "state": "admitted",
                    "recorded_at": "2026-09-08T11:28:07+00:00",
                    "run_number": 1,
                },
            ),
            "temporary_checkout": True,
            "unit": unit,
            "checkout": str(checkout_path),
            "target": target,
            "state": "unknown" if historical else "completed",
            "result": "unknown" if historical else "success",
            "result_source": (
                "historical-validation-service-result"
                if historical
                else "validation-service-result"
            ),
            "service_result_schema": schema_version,
            "final_validate_status": "PASSED",
            "exit_code": 0,
            "executed_nodes": 1,
            "executed_tests": 1,
            "scorecard_writeback_files": scorecard_rows,
            "scorecard_handoff": str(handoff_directory),
        }
    )
    if schema_version >= 3:
        record["selection_mode"] = "full"
    else:
        record.pop("selection_mode", None)
    if schema_version >= 2:
        record["scorecard_writeback"] = {"status": "completed"}
    else:
        record.pop("scorecard_writeback", None)
    if schema_version >= 4:
        record["passed_tests"] = 1
    elif historical and registered_checkout:
        record["passed_tests"] = None
    else:
        record.pop("passed_tests", None)
    if historical:
        record["detail"] = (
            "validation-service-result-schema_version: historical schema "
            f"{schema_version} is readable but has no current authority"
        )
    elif schema_version == 5 or record.get("kind") == "frozen-validate":
        record["detail"] = None
    else:
        record.pop("detail", None)
    if registered_checkout:
        record["wrkslots_slot"] = slot
        record["wrkslots_generation"] = generation
    run_record.parent.mkdir(parents=True, exist_ok=True)
    run_record.write_text(
        json.dumps(record, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )

    service_result = {
        "schema_version": schema_version,
        "commit": target,
        "profile": "full",
        "selection_mode": "full",
        "final_validate_status": "PASSED",
        "detail": None,
        "exit_code": 0,
        "executed_nodes": 1,
        "executed_tests": 1,
        "passed_tests": 1,
        "scorecard_writeback": {"status": "completed"},
    }
    schema_fields = schema_value.get("fields")
    assert isinstance(schema_fields, list)
    service_result = {
        field: service_result[field]
        for field in schema_fields
        if isinstance(field, str)
    }
    artifacts = {
        "run-record": run_record,
        "service-result": run_record.with_name(
            f"{run_record.stem}.service-result.json"
        ),
        "producer-schema": producer_schema,
        "scorecard-handoff": handoff_directory / "handoff.json",
    }
    artifacts["service-result"].write_text(
        json.dumps(service_result, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    artifacts["scorecard-handoff"].write_text(
        json.dumps(
            {
                "schema_version": 1,
                "target": target,
                "unit": unit_name,
                "writeback_completed": True,
                "files": scorecard_rows,
            },
            indent=2,
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )

    rows = []
    for role, path in artifacts.items():
        metadata = path.stat()
        payload = path.read_bytes()
        rows.append(
            {
                "role": role,
                "path": (
                    path.relative_to(project).as_posix()
                    if path.is_relative_to(project)
                    else str(path)
                ),
                "identity": [metadata.st_dev, metadata.st_ino],
                "size": len(payload),
                "sha256": hashlib.sha256(payload).hexdigest(),
            }
        )
    manifest = (
        project
        / "ignored"
        / "validate"
        / "removal-proofs"
        / f"{slot}-{generation}.json"
    )
    manifest.parent.mkdir(parents=True, exist_ok=True)
    manifest.write_text(
        json.dumps(
            {
                "schema": 1,
                "kind": "validation-removal-proof",
                "slot": slot,
                "generation": generation,
                "checkout": (
                    checkout_path.relative_to(project).as_posix()
                    if checkout_path.is_relative_to(project)
                    else str(checkout_path)
                ),
                "artifacts": rows,
            },
            indent=2,
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )
    if registered_checkout:
        slot_root = wrkslots._slot_directory(
            wrkslots._load_config(str(project), "testhost"), slot, "validate"
        )
        assert slot_root in checkout_path.parents or slot_root == checkout_path
    return manifest, artifacts


def validation_service_result_schema(schema_version: int = 4) -> dict[str, object]:
    fields = {
        1: [
            "schema_version",
            "commit",
            "profile",
            "final_validate_status",
            "exit_code",
            "executed_nodes",
            "executed_tests",
        ],
        2: [
            "schema_version",
            "commit",
            "profile",
            "final_validate_status",
            "exit_code",
            "executed_nodes",
            "executed_tests",
            "scorecard_writeback",
        ],
        3: [
            "schema_version",
            "commit",
            "profile",
            "selection_mode",
            "final_validate_status",
            "exit_code",
            "executed_nodes",
            "executed_tests",
            "scorecard_writeback",
        ],
        4: [
            "schema_version",
            "commit",
            "profile",
            "selection_mode",
            "final_validate_status",
            "exit_code",
            "executed_nodes",
            "executed_tests",
            "passed_tests",
            "scorecard_writeback",
        ],
        5: [
            "schema_version",
            "commit",
            "profile",
            "selection_mode",
            "final_validate_status",
            "detail",
            "exit_code",
            "executed_nodes",
            "executed_tests",
            "passed_tests",
            "scorecard_writeback",
        ],
    }[schema_version]
    exit_field = "exit_code" if schema_version == 1 else "validation_exit_code"
    result: dict[str, object] = {
        "schema_version": schema_version,
        "fields": fields,
        "outcomes": {
            "PASSED": {exit_field: 0, "state": "completed", "result": "success"},
            "FAILED": {exit_field: 1, "state": "completed", "result": "failure"},
            "COULD_NOT_RUN": {
                exit_field: 75,
                "state": "completed",
                "result": "no-result",
            },
        },
    }
    if schema_version >= 2:
        result["scorecard_writeback"] = {
            "nullable": True,
            "variants": {
                "completed": ["status"],
                "failed": ["status", "error"],
            },
        }
    return result


def commit_validation_removal_schema(
    repository: Path, *, schema_version: int = 4
) -> None:
    schema = (
        repository
        / "ci"
        / "manifest-plan"
        / "validation-service-result-schema.json"
    )
    schema.parent.mkdir(parents=True, exist_ok=True)
    schema.write_text(
        json.dumps(
            validation_service_result_schema(schema_version),
            indent=2,
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )
    git(repository, "add", schema.relative_to(repository).as_posix())
    git(repository, "commit", "-m", "add validation result schema")
    git(repository, "push", "origin", "main")


def rebind_validation_removal_artifact(
    project: Path, manifest: Path, role: str, path: Path
) -> None:
    value: object = json.loads(manifest.read_text(encoding="utf-8"))
    assert isinstance(value, dict)
    rows = value["artifacts"]
    assert isinstance(rows, list)
    row = next(
        item
        for item in rows
        if isinstance(item, dict) and item.get("role") == role
    )
    metadata = path.stat()
    payload = path.read_bytes()
    row.update(
        {
            "path": (
                path.relative_to(project).as_posix()
                if path.is_relative_to(project)
                else str(path)
            ),
            "identity": [metadata.st_dev, metadata.st_ino],
            "size": len(payload),
            "sha256": hashlib.sha256(payload).hexdigest(),
        }
    )
    manifest.write_text(
        json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )


def stub_validate_batch_censuses(
    monkeypatch: pytest.MonkeyPatch,
    privileged: Callable[[Sequence[Path]], wrkslots._ProcessPathCensus] | None = None,
) -> None:
    empty = lambda _paths: wrkslots._ProcessPathCensus((), ())

    def shared(
        paths: Sequence[Path],
        *,
        budget: wrkslots._ReadOnlyCommandBudget | None = None,
        include_owner_cgroups: bool = True,
    ) -> wrkslots._ProcessPathCensus:
        del budget
        result = (privileged or empty)(paths)
        return replace(result, owner_cgroup_complete=include_owner_cgroups)

    def fresh(
        _paths: Sequence[Path], *, budget: wrkslots._ReadOnlyCommandBudget
    ) -> wrkslots._ProcessPathCensus:
        del budget
        return wrkslots._ProcessPathCensus((), ())

    monkeypatch.setattr(wrkslots, "_capture_process_path_census", shared)
    monkeypatch.setattr(wrkslots, "_capture_lsof_process_path_census", shared)
    monkeypatch.setattr(wrkslots, "_capture_same_uid_process_path_census", fresh)


def validation_exclusion_guard(project: Path) -> Path:
    journal = wrkslots._journal_path(wrkslots._load_config(str(project), "testhost"))
    value = json.loads(journal.read_text(encoding="utf-8"))
    exclusion = value.get("exclusion")
    assert isinstance(exclusion, dict)
    guard = exclusion.get("guard")
    assert isinstance(guard, str)
    return Path(guard)


def interrupt_validate_batch(
    project: Path,
    monkeypatch: pytest.MonkeyPatch,
    boundary: str,
    slots: Sequence[str],
    *,
    occurrence: int = 1,
) -> None:
    class Interrupted(RuntimeError):
        pass

    seen = 0

    def interrupt(point: str) -> None:
        nonlocal seen
        if point == boundary:
            seen += 1
        if point == boundary and seen == occurrence:
            raise Interrupted

    monkeypatch.setattr(wrkslots, "_interrupt_for_test", interrupt)
    arguments = [
        "--project-root",
        str(project),
        "remove-validate-batch",
        "--coordinator-pid",
        str(os.getpid()),
    ]
    for slot in slots:
        arguments.extend(("--slot", f"{slot}=1"))
    with pytest.raises(Interrupted):
        wrkslots.main(arguments)
    assert seen == occurrence
    monkeypatch.setattr(wrkslots, "_interrupt_for_test", lambda _point: None)


def prepare_cross_repository_target(tmp_path: Path) -> tuple[Path, Path, Path]:
    """Create one removable target and one independently registered repository."""

    project, _repository, remote = make_project(tmp_path)
    assert create(project, slot="target", slot_type="validate", branch=None).returncode == 0
    other = project / "repo-other"
    subprocess.run(
        ["git", "clone", str(remote), str(other)],
        check=True,
        capture_output=True,
        text=True,
    )
    assert create(
        project,
        slot="other",
        agent="other",
        branch="other/task",
        repository_name="repo-other",
    ).returncode == 0
    mark_owner_dead(project)
    set_liveness(project, "dead")
    return project, other, checkout(project, slot="target", slot_type="validate")


def replace_owner(project: Path, owner: wrkslots.ProcessIdentity) -> None:
    config = wrkslots._load_config(str(project), "testhost")
    state = wrkslots._load_active(config)
    record = state.slots[0]
    wrkslots._write_active_state(
        config,
        wrkslots._replace_record(state, replace(record, owner=owner)),
        action="test-owner-replaced",
        slot=record.slot,
    )


def expire_heartbeat(project: Path, machine: str = "testhost") -> None:
    config = wrkslots._load_config(str(project), machine)
    state = wrkslots._load_active(config)
    record = state.slots[0]
    heartbeat_at = (
        dt.datetime.now(dt.timezone.utc)
        - dt.timedelta(seconds=record.heartbeat_ttl_seconds + 1)
    ).isoformat(timespec="seconds")
    updated = replace(record, heartbeat_at=heartbeat_at)
    wrkslots._write_active_state(
        config,
        wrkslots._replace_record(state, updated),
        action="test-ttl-expired",
        slot=record.slot,
    )


def set_owner_other_machine(project: Path, machine: str = "testhost") -> None:
    config = wrkslots._load_config(str(project), machine)
    state = wrkslots._load_active(config)
    record = state.slots[0]
    assert record.owner is not None
    updated = replace(
        record,
        owner=replace(
            record.owner, host_id="a-different-machine"
        ),
    )
    wrkslots._write_active_state(
        config,
        wrkslots._replace_record(state, updated),
        action="test-owner-other-machine",
        slot=record.slot,
    )


def write_historical_state(
    project: Path,
    slot: str,
    *,
    checkout_names: tuple[str, ...] = ("product",),
    include_owner: bool = True,
    status: str = "lease-quarantined",
    task: str | None = "task-old",
    owner_task: str | None = "task-old",
    purpose: str | None = "historical fixture",
    live_owner: bool = False,
) -> Path:
    identity = wrkslots._read_process_identity(os.getpid())
    recorded_at = (dt.datetime.now(dt.timezone.utc) - dt.timedelta(days=2)).isoformat(
        timespec="seconds"
    )
    row: dict[str, object] = {
        "agents": [
            {
                "name": "codex-old",
                "read_only": False,
                "task": owner_task,
            }
        ],
        "allocated": recorded_at,
        "purpose": purpose,
        "status": status,
        "task": task,
        "updated": recorded_at,
    }
    for name in checkout_names:
        row[f"{name}_branch"] = "codex/old"
        row[f"{name}_path"] = f"worktrees/slots/{slot}/{name}"
    if include_owner:
        boot_id = identity.boot_id if live_owner else "historical-boot"
        row["owner_sidecar"] = {
            "schema_version": 1,
            "slot": slot,
            "agent": "codex-old",
            "task": owner_task,
            "recorded_at": recorded_at,
            "source": "test-owner-record-v1",
            "supervisor_pid": identity.pid,
            "boot_id": boot_id,
            "start_ticks": identity.start_ticks,
            "cgroup_path": (
                identity.cgroup_path if live_owner else "/wrkslots-test-finished"
            ),
            "generation": f"{boot_id}:{identity.pid}:{identity.start_ticks}",
        }
    path = project / "worktree-state.json"
    path.write_text(
        json.dumps(
            {
                "version": 3,
                "updated": recorded_at,
                "slots": {slot: row},
                "lease_history": [],
            },
            indent=2,
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )
    return path


def set_original_coordinator_dead(project: Path, machine: str = "testhost") -> None:
    config = wrkslots._load_config(str(project), machine)
    state = wrkslots._load_active(config)
    record = state.slots[0]
    coordinator = replace(
        record.coordinator_lease,
        boot_id="finished-coordinator-boot",
        cgroup_path="/wrkslots-test-finished-coordinator",
    )
    wrkslots._write_active_state(
        config,
        wrkslots._replace_record(state, replace(record, coordinator_lease=coordinator)),
        action="test-original-coordinator-exited",
        slot=record.slot,
    )


def prepare_removal_journal(project: Path) -> Path:
    handed_off = finish(project)
    assert handed_off.returncode == 0, handed_off.stderr
    mark_owner_dead(project)
    set_liveness(project, "dead")
    interrupted = command(
        project,
        "remove",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        env={"WRKSLOTS_TEST_INTERRUPT": "after-finish-journal"},
    )
    assert interrupted.returncode == 86
    journal = control_directory(project) / "ACTIVE.testhost.journal"
    assert journal.is_file()
    return journal


def prepare_legacy_validate_checkout(
    project: Path,
    repository: Path,
    *,
    state: str = "completed",
    name: str = "legacy",
) -> tuple[Path, Path]:
    checkout_path = project / "ignored" / f"validate-fresh-{name}"
    checkout_path.parent.mkdir(parents=True, exist_ok=True)
    git(repository, "worktree", "add", "--detach", str(checkout_path), "HEAD")
    record_path = project / "ignored" / "validate" / "runs" / f"validate-{name}.json"
    record_path.parent.mkdir(parents=True, exist_ok=True)
    record_path.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "unit": f"validate-{name}.service",
                "checkout": str(checkout_path),
                "source_checkout": str(repository),
                "repo": "example/project",
                "state": state,
                "exit_code": 0 if state == "completed" else None,
                "final_validate_status": "PASSED" if state == "completed" else None,
            },
            indent=2,
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )
    return checkout_path, record_path


def prepare_terminal_validation_record(
    project: Path,
    target: Path,
    *,
    field: str,
    state: str = "completed",
    name: str = "validate-ownerless",
) -> Path:
    record_path = project / "ignored" / "validate" / "runs" / f"{name}.json"
    record_path.parent.mkdir(parents=True, exist_ok=True)
    record_path.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "unit": f"{name}.service",
                field: str(target),
                "state": state,
                "exit_code": 0 if state == "completed" else None,
                "final_validate_status": "PASSED" if state == "completed" else None,
            },
            indent=2,
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )
    return record_path


def prepare_frozen_validation_checkout(
    project: Path,
    repository: Path,
    *,
    monkeypatch: pytest.MonkeyPatch,
    name: str = "frozen",
) -> tuple[Path, Path, str]:
    """Create the exact standalone checkout and terminal handle used by frozen runs."""

    monkeypatch.setattr(
        wrkslots, "_OWNERLESS_VALIDATION_EXCLUSION_ROOTS", (Path("/tmp"),)
    )
    target_sha = git(repository, "rev-parse", "HEAD").stdout.strip()
    install_frozen_validation_parser(project, monkeypatch)
    origin_url = git(repository, "remote", "get-url", "origin").stdout.strip()
    parent = project.parent / f".{project.name}-frozen-validate"
    parent.mkdir(mode=0o700)
    checkout = parent / f"validate-fresh-{name}"
    subprocess.run(
        ["git", "clone", "--no-checkout", "--no-tags", str(repository), str(checkout)],
        check=True,
        text=True,
        capture_output=True,
    )
    git(checkout, "remote", "set-url", "origin", origin_url)
    git(checkout, "checkout", "--detach", target_sha)
    checkout.chmod(0o700)
    record = project / "ignored" / "validate" / "runs" / f"validate-{name}.json"
    record.parent.mkdir(parents=True, exist_ok=True)
    record.write_text(
        json.dumps(
            {
                "admission": "frozen-validate",
                "admission_result": {
                    "state": "admitted",
                    "recorded_at": "2026-09-08T11:28:07+00:00",
                    "run_number": 1,
                },
                "agent": "test-agent",
                "cargo_home": str(project / "worktrees" / "validate-cargo-fixture"),
                "checkout": str(checkout),
                "current_main_before_launch": target_sha,
                "detail": "validation-service-result-read: retained result is unavailable",
                "e2e_result_root": str(project / "ignored" / "validate" / "artifacts" / name / "e2e"),
                "executed_nodes": None,
                "executed_tests": None,
                "exit_code": None,
                "final_validate_status": None,
                "finished_at": "2026-09-08T11:29:07+00:00",
                "host": "testhost",
                "kind": "frozen-validate",
                "log": str(project / "ignored" / "validate" / f"validate-{name}.log"),
                "measured_against_superseded_tip": True,
                "observed_safe_ci_cgroups": [],
                "pane_id": "test-pane",
                "pane_role": "observer-only",
                "pane_title": f"validate-{name}",
                "parent_checkout_head": target_sha,
                "pr": None,
                "process_identity": {
                    "pid": os.getpid(),
                    "start_ticks": 1,
                    "boot_id": "historical-boot",
                },
                "producer": "ci-hub/validate/run_registry.py",
                "qualifying_receipt": False,
                "repo": "consumer-a/project",
                "result": "unknown",
                "result_record": str(project / "ignored" / "validate" / "frozen" / f"validate-{name}.jsonl"),
                "result_source": "validation-service-result",
                "safe_ci_dag_runner_log_dir": str(project / "ignored" / "validate" / "artifacts" / name / "safe-ci-dag-runner"),
                "schema_version": 1,
                "scorecard_handoff": None,
                "scorecard_writeback": None,
                "scorecard_writeback_files": [],
                "selection_mode": None,
                "service_result_schema": 3,
                "source_checkout": str(repository),
                "started_at": "2026-09-08T11:28:07+00:00",
                "state": "unknown",
                "tab_id": "test-tab",
                "target": target_sha,
                "target_contains_current_main_before_launch": False,
                "temporary_checkout": True,
                "unit": f"validate-{name}.service",
                "validate_lock_child_deadline_seconds": 3600,
                "validation_kind": "frozen-validate",
                "workspace_id": "test-workspace",
            },
            indent=2,
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )
    return checkout, record, target_sha


def prepare_run1773_historical_frozen_checkout(
    project: Path,
    repository: Path,
    *,
    monkeypatch: pytest.MonkeyPatch,
    name: str = "run1773",
    target_contained_by_remote: bool = True,
    service_result_schema: int = 3,
) -> tuple[Path, Path, Path, str]:
    """Create run 1773's shape, optionally using the sibling schema-2 contract."""

    assert service_result_schema in {2, 3}
    commit_validation_removal_schema(
        repository, schema_version=service_result_schema
    )
    if not target_contained_by_remote:
        marker = repository / "historical-unpublished-target.txt"
        marker.write_text("retained historical target\n", encoding="utf-8")
        git(repository, "add", marker.name)
        git(repository, "commit", "-m", "create uncontained historical target")
    checkout, record, target_sha = prepare_frozen_validation_checkout(
        project, repository, monkeypatch=monkeypatch, name=name
    )
    value: object = json.loads(record.read_text(encoding="utf-8"))
    assert isinstance(value, dict)
    value.update(
        {
            "materialized_target": False,
            "state": "unknown",
            "result": "unknown",
            "result_source": "historical-validation-service-result",
            "detail": wrkslots._historical_validation_result_detail(
                service_result_schema
            ),
            "final_validate_status": "FAILED",
            "exit_code": 1,
            "executed_nodes": 14,
            "executed_tests": None,
            "passed_tests": None,
            "service_result_schema": service_result_schema,
            "scorecard_writeback": {"status": "completed"},
        }
    )
    if service_result_schema == 3:
        value["selection_mode"] = "only"
    else:
        value.pop("selection_mode", None)
    value.pop("scorecard_handoff", None)
    value.pop("scorecard_writeback_files", None)
    record.write_text(
        json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    sidecar = record.with_name(
        f"{record.stem}{wrkslots._VALIDATION_SERVICE_RESULT_SUFFIX}"
    )
    service_result = {
        "schema_version": service_result_schema,
        "commit": target_sha,
        "profile": "only-portable",
        "final_validate_status": "FAILED",
        "exit_code": 1,
        "executed_nodes": 14,
        "executed_tests": None,
        "scorecard_writeback": {"status": "completed"},
    }
    if service_result_schema == 3:
        service_result["selection_mode"] = "only"
    sidecar.write_text(
        json.dumps(service_result, indent=2, sort_keys=True)
        + "\n",
        encoding="utf-8",
    )

    def canonical_record(
        _config: wrkslots.Config, _path: Path, contents: bytes
    ) -> tuple[Mapping[str, object], Mapping[str, object]]:
        parsed = wrkslots._strict_json_object(
            contents, "run1773 frozen validation fixture"
        )
        assert parsed.get("schema_version") == 1
        assert parsed.get("kind") == "frozen-validate"
        assert parsed.get("producer") == "ci-hub/validate/run_registry.py"
        assert parsed.get("admission") == "frozen-validate"
        assert parsed.get("validation_kind") == "frozen-validate"
        return (
            {
                "schema_version": parsed["schema_version"],
                "kind": parsed["kind"],
                "state": parsed["state"],
                "target": parsed["target"],
                "repo": parsed["repo"],
            },
            parsed,
        )

    monkeypatch.setattr(
        wrkslots, "_canonical_frozen_validation_record", canonical_record
    )
    return checkout, record, sidecar, target_sha


def stub_canonical_frozen_record_parser(
    monkeypatch: pytest.MonkeyPatch, *, label: str
) -> None:
    def canonical_record(
        _config: wrkslots.Config, _path: Path, contents: bytes
    ) -> tuple[Mapping[str, object], Mapping[str, object]]:
        parsed = wrkslots._strict_json_object(contents, label)
        return (
            {
                "schema_version": parsed["schema_version"],
                "kind": parsed["kind"],
                "state": parsed["state"],
                "target": parsed["target"],
                "repo": parsed["repo"],
            },
            parsed,
        )

    monkeypatch.setattr(
        wrkslots, "_canonical_frozen_validation_record", canonical_record
    )


def run_frozen_validation_batch(
    project: Path, repository: Path, checkout: Path, record: Path
) -> int:
    return wrkslots.main(
        [
            "--project-root",
            str(project),
            "recover-ownerless-validate-batch",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--frozen-validate-checkout",
            str(checkout),
            "--completed-record",
            record.relative_to(project).as_posix(),
            "--repository",
            repository.relative_to(project).as_posix(),
            "--format",
            "json",
        ]
    )


def git_metadata_snapshot(checkout: Path) -> dict[str, tuple[int, int, int, str]]:
    """Capture retained Git metadata without following links or reading atime."""

    git_directory = checkout / ".git"
    snapshot: dict[str, tuple[int, int, int, str]] = {}
    for path in (git_directory, *sorted(git_directory.rglob("*"))):
        metadata = path.lstat()
        relative = path.relative_to(git_directory).as_posix()
        if path.is_symlink():
            payload = os.readlink(path)
        elif path.is_file():
            payload = hashlib.sha256(path.read_bytes()).hexdigest()
        else:
            payload = ""
        snapshot[relative] = (
            stat.S_IFMT(metadata.st_mode),
            metadata.st_size,
            metadata.st_mtime_ns,
            payload,
        )
    return snapshot


@pytest.mark.ordinary_environment
def test_run1773_historical_frozen_checkout_is_retained_without_blocking_entry(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout, record, _sidecar, target_sha = (
        prepare_run1773_historical_frozen_checkout(
            project,
            repository,
            monkeypatch=monkeypatch,
            target_contained_by_remote=False,
        )
    )
    assert not git(
        repository,
        "branch",
        "--remotes",
        "--contains",
        target_sha,
    ).stdout.strip()
    stub_validate_batch_censuses(monkeypatch)
    git_before = git_metadata_snapshot(checkout)

    rc = run_frozen_validation_batch(project, repository, checkout, record)

    assert rc == 0
    report = json.loads(capsys.readouterr().out)
    assert report["schema"] == 2
    assert report["removed"] == []
    assert report["retained"] == [
        {
            "blocks_entry": False,
            "checkout": str(checkout),
            "reason": (
                "historical schema 3 frozen validation has no removal proof; "
                "the inactive checkout remains retained"
            ),
        }
    ]
    assert report["shared_process_censuses"] == 1
    assert report["same_uid_process_censuses"] == 1
    assert checkout.is_dir()
    assert git_metadata_snapshot(checkout) == git_before
    config = wrkslots._load_config(str(project), "testhost")
    assert not wrkslots._journal_path(config).exists()
    assert not any(
        item["kind"] == "ownerless-validate-path-removed"
        for item in wrkslots._load_events(config)
    )


@pytest.mark.ordinary_environment
def test_historical_frozen_retention_reports_schema_two_exactly(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout, record, _sidecar, _target_sha = (
        prepare_run1773_historical_frozen_checkout(
            project,
            repository,
            monkeypatch=monkeypatch,
            name="historical-schema-two",
            service_result_schema=2,
        )
    )
    stub_validate_batch_censuses(monkeypatch)

    rc = run_frozen_validation_batch(project, repository, checkout, record)

    assert rc == 0
    report = json.loads(capsys.readouterr().out)
    assert report["removed"] == []
    assert report["retained"] == [
        {
            "blocks_entry": False,
            "checkout": str(checkout),
            "reason": (
                "historical schema 2 frozen validation has no removal proof; "
                "the inactive checkout remains retained"
            ),
        }
    ]


@pytest.mark.ordinary_environment
@pytest.mark.parametrize(
    ("field", "replacement", "message"),
    (
        ("target", "0" * 40, "HEAD changed"),
        ("source_checkout", "/tmp/not-the-source", "exact source checkout"),
    ),
)
def test_uncontained_historical_frozen_retention_binds_head_and_source(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    field: str,
    replacement: str,
    message: str,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout, record, _sidecar, target_sha = (
        prepare_run1773_historical_frozen_checkout(
            project,
            repository,
            monkeypatch=monkeypatch,
            name=f"binding-{field.replace('_', '-')}",
            target_contained_by_remote=False,
        )
    )
    assert not git(
        repository, "branch", "--remotes", "--contains", target_sha
    ).stdout.strip()
    value: object = json.loads(record.read_text(encoding="utf-8"))
    assert isinstance(value, dict)
    value[field] = replacement
    record.write_text(
        json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    stub_validate_batch_censuses(monkeypatch)

    rc = run_frozen_validation_batch(project, repository, checkout, record)

    assert rc == 0
    report = json.loads(capsys.readouterr().out)
    assert report["removed"] == []
    assert len(report["retained"]) == 1
    assert report["retained"][0]["blocks_entry"] is True
    assert message in report["retained"][0]["reason"]
    assert checkout.is_dir()


@pytest.mark.ordinary_environment
def test_uncontained_current_frozen_checkout_still_cannot_be_removed(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    commit_validation_removal_schema(repository, schema_version=5)
    marker = repository / "current-unpublished-target.txt"
    marker.write_text("preserve current target\n", encoding="utf-8")
    git(repository, "add", marker.name)
    git(repository, "commit", "-m", "create uncontained current target")
    checkout, record, target_sha = prepare_frozen_validation_checkout(
        project,
        repository,
        monkeypatch=monkeypatch,
        name="current-uncontained",
    )
    manifest, _artifacts = prepare_validation_removal_proof(
        project,
        checkout.name,
        checkout_path=checkout,
        run_record=record,
    )
    stub_canonical_frozen_record_parser(
        monkeypatch, label="uncontained current frozen validation fixture"
    )
    stub_validate_batch_censuses(monkeypatch)
    assert not git(
        repository, "branch", "--remotes", "--contains", target_sha
    ).stdout.strip()

    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "recover-ownerless-validate-batch",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--frozen-validate-checkout",
            str(checkout),
            "--completed-record",
            record.relative_to(project).as_posix(),
            "--repository",
            repository.relative_to(project).as_posix(),
            "--validation-proof-manifest",
            manifest.relative_to(project).as_posix(),
            "--format",
            "json",
        ]
    )

    assert rc == 0
    report = json.loads(capsys.readouterr().out)
    assert report["removed"] == []
    assert len(report["retained"]) == 1
    assert report["retained"][0]["blocks_entry"] is True
    assert "not contained by remote origin" in report["retained"][0]["reason"]
    assert checkout.is_dir()


@pytest.mark.ordinary_environment
@pytest.mark.parametrize(
    ("field", "replacement"),
    (
        ("service_result_schema", "missing"),
        ("service_result_schema", None),
        ("service_result_schema", True),
        ("service_result_schema", 4),
        ("materialized_target", "missing"),
        ("materialized_target", None),
        ("materialized_target", True),
        ("scorecard_handoff", None),
        ("scorecard_writeback_files", None),
    ),
)
def test_historical_frozen_entry_classification_fails_closed_on_shape_changes(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    field: str,
    replacement: object,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout, record, _sidecar, _target_sha = (
        prepare_run1773_historical_frozen_checkout(
            project,
            repository,
            monkeypatch=monkeypatch,
            name=f"shape-{field.replace('_', '-')}",
        )
    )
    value: object = json.loads(record.read_text(encoding="utf-8"))
    assert isinstance(value, dict)
    if replacement == "missing":
        value.pop(field)
    else:
        value[field] = replacement
    record.write_text(
        json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    stub_validate_batch_censuses(monkeypatch)

    rc = run_frozen_validation_batch(project, repository, checkout, record)

    assert rc == 0
    report = json.loads(capsys.readouterr().out)
    assert report["removed"] == []
    assert len(report["retained"]) == 1
    assert report["retained"][0]["blocks_entry"] is True
    assert checkout.is_dir()


@pytest.mark.ordinary_environment
@pytest.mark.parametrize("schema", (4, 5))
def test_current_frozen_result_without_removal_proof_blocks_entry(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    schema: int,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    commit_validation_removal_schema(repository, schema_version=schema)
    checkout, record, _target_sha = prepare_frozen_validation_checkout(
        project,
        repository,
        monkeypatch=monkeypatch,
        name=f"current-schema-{schema}",
    )
    _manifest, _artifacts = prepare_validation_removal_proof(
        project,
        checkout.name,
        checkout_path=checkout,
        run_record=record,
    )
    if schema == 4:
        value: object = json.loads(record.read_text(encoding="utf-8"))
        assert isinstance(value, dict)
        value.pop("detail", None)
        record.write_text(
            json.dumps(value, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )

    def canonical_record(
        _config: wrkslots.Config, _path: Path, contents: bytes
    ) -> tuple[Mapping[str, object], Mapping[str, object]]:
        parsed = wrkslots._strict_json_object(
            contents, "current frozen validation fixture"
        )
        return (
            {
                "schema_version": parsed["schema_version"],
                "kind": parsed["kind"],
                "state": parsed["state"],
                "target": parsed["target"],
                "repo": parsed["repo"],
            },
            parsed,
        )

    monkeypatch.setattr(
        wrkslots, "_canonical_frozen_validation_record", canonical_record
    )
    stub_validate_batch_censuses(monkeypatch)

    rc = run_frozen_validation_batch(project, repository, checkout, record)

    assert rc == 0
    report = json.loads(capsys.readouterr().out)
    assert report["removed"] == []
    assert len(report["retained"]) == 1
    assert report["retained"][0]["blocks_entry"] is True
    assert "no typed removal proof" in report["retained"][0]["reason"]
    assert checkout.is_dir()


@pytest.mark.ordinary_environment
def test_direct_current_frozen_recovery_requires_removal_proof(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    commit_validation_removal_schema(repository, schema_version=5)
    checkout, record, _target_sha = prepare_frozen_validation_checkout(
        project, repository, monkeypatch=monkeypatch, name="current-direct-no-proof"
    )
    _manifest, _artifacts = prepare_validation_removal_proof(
        project,
        checkout.name,
        checkout_path=checkout,
        run_record=record,
    )
    stub_canonical_frozen_record_parser(
        monkeypatch, label="current direct frozen validation fixture"
    )

    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "recover",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--frozen-validate-checkout",
            str(checkout),
            "--completed-record",
            record.relative_to(project).as_posix(),
            "--repository",
            repository.relative_to(project).as_posix(),
        ]
    )

    assert rc == 3
    assert "requires --validation-proof-manifest" in capsys.readouterr().err
    assert checkout.is_dir()
    assert not (control_directory(project) / "ACTIVE.testhost.journal").exists()


@pytest.mark.ordinary_environment
def test_direct_historical_projection_cannot_use_batch_only_nonblocking_result(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout, record, _sidecar, _target_sha = (
        prepare_run1773_historical_frozen_checkout(
            project, repository, monkeypatch=monkeypatch, name="historical-direct"
        )
    )
    stub_canonical_frozen_record_parser(
        monkeypatch, label="direct historical frozen validation fixture"
    )

    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "recover",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--frozen-validate-checkout",
            str(checkout),
            "--completed-record",
            record.relative_to(project).as_posix(),
            "--repository",
            repository.relative_to(project).as_posix(),
        ]
    )

    assert rc == 3
    assert "bounded batch command" in capsys.readouterr().err
    assert checkout.is_dir()
    assert not (control_directory(project) / "ACTIVE.testhost.journal").exists()


@pytest.mark.ordinary_environment
@pytest.mark.parametrize("schema", (1, 2, 3))
def test_direct_frozen_recovery_preserves_proofless_legacy_schemas(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    schema: int,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout, record, _target_sha = prepare_frozen_validation_checkout(
        project,
        repository,
        monkeypatch=monkeypatch,
        name=f"legacy-schema-{schema}",
    )
    value: object = json.loads(record.read_text(encoding="utf-8"))
    assert isinstance(value, dict)
    value["service_result_schema"] = schema
    record.write_text(
        json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    stub_canonical_frozen_record_parser(
        monkeypatch, label="direct legacy frozen validation fixture"
    )
    stub_validate_batch_censuses(monkeypatch)

    recovered = wrkslots.main(
        [
            "--project-root",
            str(project),
            "recover",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--frozen-validate-checkout",
            str(checkout),
            "--completed-record",
            record.relative_to(project).as_posix(),
            "--repository",
            repository.relative_to(project).as_posix(),
        ]
    )

    assert recovered == 0
    assert not checkout.exists()


@pytest.mark.ordinary_environment
@pytest.mark.parametrize("mutation", ("missing", "null"))
def test_direct_current_frozen_recovery_refuses_lost_journal_proof(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    mutation: str,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    commit_validation_removal_schema(repository, schema_version=5)
    checkout, record, _target_sha = prepare_frozen_validation_checkout(
        project,
        repository,
        monkeypatch=monkeypatch,
        name=f"current-journal-{mutation}",
    )
    manifest, _artifacts = prepare_validation_removal_proof(
        project,
        checkout.name,
        checkout_path=checkout,
        run_record=record,
    )
    stub_canonical_frozen_record_parser(
        monkeypatch, label="journaled current frozen validation fixture"
    )
    stub_validate_batch_censuses(monkeypatch)

    class Interrupted(RuntimeError):
        pass

    def interrupt(point: str) -> None:
        if point == "after-ownerless-validate-journal":
            raise Interrupted

    monkeypatch.setattr(wrkslots, "_interrupt_for_test", interrupt)
    with pytest.raises(Interrupted):
        wrkslots.main(
            [
                "--project-root",
                str(project),
                "recover",
                "--coordinator-authorized",
                "--coordinator-pid",
                str(os.getpid()),
                "--frozen-validate-checkout",
                str(checkout),
                "--completed-record",
                record.relative_to(project).as_posix(),
                "--repository",
                repository.relative_to(project).as_posix(),
                "--validation-proof-manifest",
                manifest.relative_to(project).as_posix(),
            ]
        )
    journal = control_directory(project) / "ACTIVE.testhost.journal"
    value: object = json.loads(journal.read_text(encoding="utf-8"))
    assert isinstance(value, dict)
    authorization = value["authorization"]
    assert isinstance(authorization, dict)
    if mutation == "missing":
        authorization.pop("removal_proof")
    else:
        authorization["removal_proof"] = None
    journal.write_text(
        json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    shutil.rmtree(control_directory(project) / "EVENTS.testhost")
    monkeypatch.setattr(wrkslots, "_interrupt_for_test", lambda _point: None)

    refused = wrkslots.main(
        [
            "--project-root",
            str(project),
            "recover",
            "--coordinator-pid",
            str(os.getpid()),
        ]
    )

    assert refused == 3
    assert "requires --validation-proof-manifest" in capsys.readouterr().err
    assert checkout.is_dir()
    assert journal.is_file()


@pytest.mark.ordinary_environment
def test_direct_current_frozen_recovery_refuses_tampered_proof(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    commit_validation_removal_schema(repository, schema_version=5)
    checkout, record, _target_sha = prepare_frozen_validation_checkout(
        project, repository, monkeypatch=monkeypatch, name="current-tampered-proof"
    )
    manifest, _artifacts = prepare_validation_removal_proof(
        project,
        checkout.name,
        checkout_path=checkout,
        run_record=record,
    )
    stub_canonical_frozen_record_parser(
        monkeypatch, label="tampered current frozen validation fixture"
    )
    stub_validate_batch_censuses(monkeypatch)
    value: object = json.loads(manifest.read_text(encoding="utf-8"))
    assert isinstance(value, dict)
    value["checkout"] = str(checkout.with_name("another-checkout"))
    manifest.write_text(
        json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )

    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "recover",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--frozen-validate-checkout",
            str(checkout),
            "--completed-record",
            record.relative_to(project).as_posix(),
            "--repository",
            repository.relative_to(project).as_posix(),
            "--validation-proof-manifest",
            manifest.relative_to(project).as_posix(),
        ]
    )

    assert rc == 3
    assert "does not name the exact checkout" in capsys.readouterr().err
    assert checkout.is_dir()
    assert not (control_directory(project) / "ACTIVE.testhost.journal").exists()


@pytest.mark.ordinary_environment
def test_historical_frozen_live_process_identity_blocks_entry(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout, record, _sidecar, _target_sha = (
        prepare_run1773_historical_frozen_checkout(
            project, repository, monkeypatch=monkeypatch, name="live-process"
        )
    )
    identity = wrkslots._read_process_identity(os.getpid())
    value: object = json.loads(record.read_text(encoding="utf-8"))
    assert isinstance(value, dict)
    value["process_identity"] = {
        "pid": identity.pid,
        "start_ticks": identity.start_ticks,
        "boot_id": identity.boot_id,
    }
    record.write_text(
        json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    stub_validate_batch_censuses(monkeypatch)

    rc = run_frozen_validation_batch(project, repository, checkout, record)

    assert rc == 0
    report = json.loads(capsys.readouterr().out)
    assert report["removed"] == []
    assert len(report["retained"]) == 1
    assert report["retained"][0]["blocks_entry"] is True
    assert "process state is live or uncertain" in report["retained"][0]["reason"]
    assert checkout.is_dir()


@pytest.mark.ordinary_environment
@pytest.mark.parametrize("artifact", ("service-result", "producer-schema"))
def test_historical_frozen_entry_classification_fails_closed_on_evidence_change(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    artifact: str,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout, record, sidecar, _target_sha = (
        prepare_run1773_historical_frozen_checkout(
            project,
            repository,
            monkeypatch=monkeypatch,
            name=f"changed-{artifact}",
        )
    )
    if artifact == "service-result":
        value: object = json.loads(sidecar.read_text(encoding="utf-8"))
        assert isinstance(value, dict)
        value["executed_nodes"] = 15
        sidecar.write_text(
            json.dumps(value, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
    else:
        producer = checkout / wrkslots._VALIDATION_SERVICE_RESULT_SCHEMA_PATH
        value = json.loads(producer.read_text(encoding="utf-8"))
        assert isinstance(value, dict)
        value["schema_version"] = 2
        producer.write_text(
            json.dumps(value, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
    stub_validate_batch_censuses(monkeypatch)

    rc = run_frozen_validation_batch(project, repository, checkout, record)

    assert rc == 0
    report = json.loads(capsys.readouterr().out)
    assert report["removed"] == []
    assert len(report["retained"]) == 1
    assert report["retained"][0]["blocks_entry"] is True
    assert checkout.is_dir()


@pytest.mark.ordinary_environment
def test_frozen_parser_and_consumer_use_exact_real_projection_shape(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout, record, target_sha = prepare_frozen_validation_checkout(
        project,
        repository,
        monkeypatch=monkeypatch,
        name="real-projection",
    )
    config = wrkslots._load_config(str(project), "testhost")
    contents = record.read_bytes()
    raw = wrkslots._as_mapping(
        json.loads(contents), "frozen validation fixture record"
    )

    projected, rebound = wrkslots._canonical_frozen_validation_record(
        config, record, contents
    )

    assert projected == {
        "schema_version": 1,
        "kind": "frozen-validate",
        "state": "unknown",
        "unit": "validate-real-projection.service",
        "target": target_sha,
        "repo": raw["repo"],
        "branch": None,
        "agent": "test-agent",
        "started_at": "2026-09-08T11:28:07+00:00",
        "host": "testhost",
        "process_identity": raw["process_identity"],
        "admission_result": raw["admission_result"],
        "termination_requests": [],
        "lock_admissible": False,
        "counts_as_validation": True,
    }
    assert rebound == raw
    assert wrkslots._terminal_validation_record(
        config,
        record.relative_to(project).as_posix(),
        checkout,
        "frozen-checkout",
        repository=repository,
    ) == (
        record.relative_to(project).as_posix(),
        hashlib.sha256(contents).hexdigest(),
        target_sha,
    )


def test_init_is_idempotent_and_installs_relative_symlink(tmp_path: Path) -> None:
    project = tmp_path / "project"
    project.mkdir()
    initialize(project, directory="opaque")
    before = {
        path.name: path.read_bytes()
        for path in sorted((project / "opaque").glob("*.json"))
    }

    initialize(project, directory="opaque")

    after = {
        path.name: path.read_bytes()
        for path in sorted((project / "opaque").glob("*.json"))
    }
    assert after == before
    link = project / "opaque" / "wrkslots"
    assert link.is_symlink()
    assert not Path(os.readlink(link)).is_absolute()
    assert link.resolve() == WRKSLOTS.resolve()
    config = json.loads((project / ".wrkslots.yml").read_text(encoding="utf-8"))
    assert config["machine"] == "testhost"
    assert config["worktrees_dir"] == "opaque"


def test_default_layout_separates_agent_and_validate_slots_under_worktrees(
    tmp_path: Path,
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    liveness = project / "liveness.py"
    liveness.write_text("#!/usr/bin/env python3\nraise SystemExit(2)\n", encoding="utf-8")
    liveness.chmod(0o755)

    initialized = subprocess.run(
        [
            sys.executable,
            str(WRKSLOTS),
            "--machine",
            "testhost",
            "init",
            str(project),
            "--liveness-command",
            "liveness.py",
        ],
        text=True,
        capture_output=True,
        check=False,
        env=source_environment(),
    )

    assert initialized.returncode == 0, initialized.stderr
    assert configuration(project)["worktrees_dir"] == "worktrees/slots"
    assert (project / "worktrees" / "slots").is_dir()
    assert (project / "worktrees" / "ACTIVE.testhost.json").is_file()
    assert (project / "worktrees" / "ARCHIVED.testhost.json").is_file()
    assert (project / "worktrees" / "EVENTS.testhost").is_dir()
    assert (project / "worktrees" / "wrkslots").is_symlink()
    config = wrkslots._load_config(str(project), "testhost")
    assert wrkslots._slot_directory(config, "agent01", "agent") == (
        project / "worktrees" / "slots" / "agent01"
    )
    assert wrkslots._slot_directory(config, "validate01", "validate") == (
        project / "worktrees" / "validate" / "validate01"
    )


def test_create_without_coordinator_authorization_names_boundary_and_remedy(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    events = project / "worktrees" / "EVENTS.testhost"
    before = sorted(path.read_bytes() for path in events.glob("*.json"))

    refused = raw_command(
        project,
        "create",
        "slot01",
        "--slot-type",
        "agent",
        "--agent",
        "codex-1",
        "--task",
        "task-slot01",
        "--purpose",
        "test slot01",
        "--owner-pid",
        str(os.getpid()),
        "--coordinator-pid",
        str(os.getpid()),
        "--repo",
        "product=repo",
        "--branch",
        "product=codex/task",
    )

    assert refused.returncode == 3
    assert "worktree creation requires a coordinator assignment" in refused.stderr
    assert "no worktree or lifecycle record was changed" in refused.stderr
    assert "ask the coordinator" in refused.stderr
    assert "an assigned agent may rerun" in refused.stderr
    assert "its own --owner-pid" in refused.stderr
    assert refused.stdout == ""
    assert not checkout(project).exists()
    assert active_slots(project) == []
    assert sorted(path.read_bytes() for path in events.glob("*.json")) == before


def test_create_binds_another_child_of_the_invoking_coordinator(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)
    owner = subprocess.Popen(["sleep", "60"], cwd=project, text=True)
    try:
        created = create(project, owner_pid=owner.pid)

        assert created.returncode == 0, created.stderr
        row = active_slots(project)[0]
        assert isinstance(row, dict)
        recorded_owner = row["owner"]
        assert isinstance(recorded_owner, dict)
        assert recorded_owner["pid"] == owner.pid
    finally:
        terminate_process(owner)


def delegated_create_arguments(coordinator_pid: int, owner_pid: int | None) -> list[str]:
    arguments = [
        "create",
        "slot01",
        "--slot-type",
        "agent",
        "--coordinator-authorized",
        "--agent",
        "codex-1",
        "--task",
        "task-slot01",
        "--purpose",
        "test slot01",
        "--coordinator-pid",
        str(coordinator_pid),
        "--repo",
        "product=repo",
        "--branch",
        "product=codex/task",
    ]
    if owner_pid is not None:
        arguments.extend(("--owner-pid", str(owner_pid)))
    return arguments


@pytest.mark.ordinary_environment
def test_create_binds_owner_running_beside_its_assigned_coordinator(tmp_path: Path) -> None:
    project, repository, _remote = make_project(tmp_path)
    coordinator = subprocess.Popen(["sleep", "60"], text=True)
    script = """
import contextlib, io, json, os, sys
from wrkslots import cli
arguments = json.loads(sys.argv[1]) + ['--owner-pid', str(os.getpid())]
output = io.StringIO()
with contextlib.redirect_stdout(output):
    result = cli.main(arguments)
print(json.dumps({'result': result, 'output': output.getvalue()}), flush=True)
sys.stdin.readline()
raise SystemExit(result)
"""
    arguments = ["--project-root", str(project)] + delegated_create_arguments(
        coordinator.pid, None
    )
    owner = subprocess.Popen(
        [sys.executable, "-c", script, json.dumps(arguments)],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=source_environment(),
    )
    try:
        assert owner.stdout is not None
        report = json.loads(owner.stdout.readline())
        assert report["result"] == 0, report
        assert wrkslots._read_process_parent(owner.pid) == os.getpid()
        assert wrkslots._read_process_parent(coordinator.pid) == os.getpid()
        row = active_slots(project)[0]
        assert isinstance(row, dict)
        assert row["owner"] == wrkslots._identity_to_obj(
            wrkslots._read_process_identity(owner.pid)
        )
        assert row["coordinator_lease"] == wrkslots._identity_to_obj(
            wrkslots._read_process_identity(coordinator.pid)
        )
        assert (
            git(checkout(project), "symbolic-ref", "--short", "HEAD").stdout.strip()
            == "codex/task"
        )
        registrations = git(repository, "worktree", "list", "--porcelain").stdout
        assert f"worktree {checkout(project)}\n" in registrations
        assert not create_journal_path(project).exists()
        stdout, stderr = owner.communicate("done\n", timeout=10)
        assert owner.returncode == 0, (stdout, stderr)
    finally:
        terminate_process(owner)
        terminate_process(coordinator)


@pytest.mark.parametrize("bind_unrelated_owner", (False, True))
def test_create_refuses_borrowed_coordinator_without_self_binding(
    tmp_path: Path, bind_unrelated_owner: bool
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    coordinator = subprocess.Popen(["sleep", "60"], text=True)
    try:
        refused = command(
            project,
            *delegated_create_arguments(
                coordinator.pid,
                coordinator.pid if bind_unrelated_owner else None,
            ),
        )
        assert refused.returncode == 3
        assert (
            f"coordinator PID {coordinator.pid} is not in the invoking process ancestry"
            in refused.stderr
        )
        assert not checkout(project).exists()
        assert active_slots(project) == []
        assert not create_journal_path(project).exists()
    finally:
        terminate_process(coordinator)


def test_create_refuses_dead_assigned_coordinator_before_mutation(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)
    coordinator = subprocess.Popen(["sleep", "60"], text=True)
    terminate_process(coordinator)

    refused = command(
        project, *delegated_create_arguments(coordinator.pid, os.getpid())
    )

    assert refused.returncode == 3
    assert "not live" in refused.stderr
    assert not checkout(project).exists()
    assert active_slots(project) == []
    assert not create_journal_path(project).exists()


def test_interrupted_delegated_create_recovers_after_assigned_coordinator_exits(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    coordinator = subprocess.Popen(["sleep", "60"], text=True)
    coordinator_identity = wrkslots._identity_to_obj(
        wrkslots._read_process_identity(coordinator.pid)
    )
    try:
        interrupted = command(
            project,
            *delegated_create_arguments(coordinator.pid, os.getpid()),
            env={"WRKSLOTS_TEST_INTERRUPT": "after-create-worktree"},
        )
        assert interrupted.returncode == 86, interrupted.stderr
        assert active_slots(project) == []
        assert create_journal_path(project).exists()
    finally:
        terminate_process(coordinator)

    recovered = command(
        project, "recover", "--coordinator-pid", str(os.getpid()), "--slot", "slot01"
    )

    assert recovered.returncode == 0, recovered.stderr
    row = active_slots(project)[0]
    assert isinstance(row, dict)
    assert row["coordinator_lease"] == coordinator_identity
    assert row["owner"] == wrkslots._identity_to_obj(
        wrkslots._read_process_identity(os.getpid())
    )
    assert checkout(project).is_dir()
    assert not create_journal_path(project).exists()


@pytest.mark.parametrize("boundary", ("mutation", "publication"))
@pytest.mark.parametrize("change", ("exit", "generation"))
def test_create_rechecks_assigned_coordinator_at_mutation_and_publication(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    boundary: str,
    change: str,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    coordinator = subprocess.Popen(["sleep", "60"], text=True)
    captured = wrkslots._read_process_identity(coordinator.pid)
    original_read = wrkslots._read_process_identity
    original_locks = wrkslots._mutation_locks
    mutation_started = False

    @contextlib.contextmanager
    def observe_lock(config: wrkslots.Config, wait_seconds: float) -> Iterator[None]:
        nonlocal mutation_started
        with original_locks(config, wait_seconds):
            mutation_started = True
            yield

    def changed_process(pid: int) -> wrkslots.ProcessIdentity:
        reached_boundary = (
            mutation_started if boundary == "mutation" else checkout(project).exists()
        )
        if pid == coordinator.pid and reached_boundary:
            if change == "exit":
                raise wrkslots.Refusal(f"owner PID {pid} is not live")
            return replace(captured, start_ticks=captured.start_ticks + 1)
        return original_read(pid)

    monkeypatch.setattr(wrkslots, "_mutation_locks", observe_lock)
    monkeypatch.setattr(wrkslots, "_read_process_identity", changed_process)
    try:
        result = wrkslots.main(
            ["--project-root", str(project)]
            + delegated_create_arguments(coordinator.pid, os.getpid())
        )
        error = capsys.readouterr().err
        assert result == 3
        expected = (
            "coordinator process changed"
            if change == "exit"
            else "coordinator process generation changed"
        )
        assert expected in error
        assert active_slots(project) == []
        assert checkout(project).exists() == (boundary == "publication")
        assert create_journal_path(project).exists() == (boundary == "publication")
    finally:
        terminate_process(coordinator)


def test_create_refuses_live_owner_outside_the_invoking_coordinator(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    owner_pid = spawn_process_outside_coordinator()
    try:
        refused = create(project, owner_pid=owner_pid)

        assert refused.returncode == 3
        assert (
            f"owner PID {owner_pid} does not descend from coordinator PID {os.getpid()}"
            in refused.stderr
        )
        assert not checkout(project).exists()
        assert active_slots(project) == []
    finally:
        terminate_process_id(owner_pid)


def test_recover_removes_only_evidenced_completed_legacy_validate_checkout(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout_path, record_path = prepare_legacy_validate_checkout(project, repository)

    recovered = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--legacy-validate-checkout",
        checkout_path.relative_to(project).as_posix(),
        "--completed-record",
        record_path.relative_to(project).as_posix(),
        "--repository",
        repository.relative_to(project).as_posix(),
    )

    assert recovered.returncode == 0, recovered.stderr
    assert "recovered ownerless validation checkout" in recovered.stdout
    assert not checkout_path.exists()
    assert checkout_path.absolute() not in wrkslots._GitVcs().listed_worktrees(repository)
    assert active_slots(project) == []
    config = wrkslots._load_config(str(project), "testhost")
    events = wrkslots._load_events(config)
    assert any(event["kind"] == "ownerless-validate-path-removed" for event in events)


def test_ownerless_validate_batch_shares_one_census_and_removes_unheld_checkouts(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    first, first_record = prepare_legacy_validate_checkout(
        project, repository, name="first"
    )
    second, second_record = prepare_legacy_validate_checkout(
        project, repository, name="second"
    )
    first.chmod(0o700)
    second.chmod(0o700)
    shared_calls: list[tuple[Path, ...]] = []
    fresh_calls: list[tuple[Path, ...]] = []

    def shared(
        paths: Sequence[Path], **_kwargs: object
    ) -> wrkslots._ProcessPathCensus:
        shared_calls.append(tuple(paths))
        budget = _kwargs.get("budget")
        assert isinstance(budget, wrkslots._ReadOnlyCommandBudget)
        budget.input_remaining = 1
        return wrkslots._ProcessPathCensus((), (), owner_cgroup_complete=False)

    def fresh(
        paths: Sequence[Path], **_kwargs: object
    ) -> wrkslots._ProcessPathCensus:
        fresh_calls.append(tuple(paths))
        budget = _kwargs.get("budget")
        assert isinstance(budget, wrkslots._ReadOnlyCommandBudget)
        assert budget.input_remaining == wrkslots._MOUNTINFO_CENSUS_BYTES_LIMIT
        return wrkslots._ProcessPathCensus((), (), owner_cgroup_complete=False)

    monkeypatch.setattr(wrkslots, "_capture_lsof_process_path_census", shared)
    monkeypatch.setattr(wrkslots, "_capture_same_uid_process_path_census", fresh)

    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "recover-ownerless-validate-batch",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--checkout",
            first.relative_to(project).as_posix(),
            "--checkout",
            second.relative_to(project).as_posix(),
            "--completed-record",
            first_record.relative_to(project).as_posix(),
            "--completed-record",
            second_record.relative_to(project).as_posix(),
            "--repository",
            repository.relative_to(project).as_posix(),
            "--repository",
            repository.relative_to(project).as_posix(),
            "--format",
            "json",
        ]
    )

    assert rc == 0
    report = json.loads(capsys.readouterr().out)
    assert report["process_censuses"] == 1
    assert report["shared_process_censuses"] == 1
    assert report["same_uid_process_censuses"] == 2
    assert report["removed"] == [
        {"checkout": first.relative_to(project).as_posix()},
        {"checkout": second.relative_to(project).as_posix()},
    ]
    assert report["retained"] == []
    assert shared_calls == [(first, second)]
    assert len(fresh_calls) == 2
    assert not first.exists()
    assert not second.exists()


def test_ownerless_batch_classification_is_read_only_beside_live_dirty_create(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    interrupted = create(
        project, env={"WRKSLOTS_TEST_INTERRUPT": "after-create-worktree"}
    )
    assert interrupted.returncode == 86
    legacy = control_directory(project) / "ACTIVE.testhost.journal"
    os.replace(create_journal_path(project), legacy)
    shutil.rmtree(control_directory(project) / "EVENTS.testhost")
    (checkout(project) / "unfinished.txt").write_text(
        "preserve this\n", encoding="utf-8"
    )
    retained, record = prepare_legacy_validate_checkout(
        project, repository, name="classification"
    )
    retained.chmod(0o700)
    before = legacy.read_bytes()
    inode = legacy.stat(follow_symlinks=False).st_ino

    sibling = subprocess.Popen(["sleep", "60"], text=True)
    try:
        refused = command(
            project,
            "recover-ownerless-validate-batch",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(sibling.pid),
            "--checkout",
            retained.relative_to(project).as_posix(),
            "--completed-record",
            record.relative_to(project).as_posix(),
            "--repository",
            repository.relative_to(project).as_posix(),
        )
    finally:
        terminate_process(sibling)
    assert refused.returncode == 3
    assert "not in the invoking process ancestry" in refused.stderr
    assert retained.is_dir()
    assert legacy.read_bytes() == before
    assert legacy.stat(follow_symlinks=False).st_ino == inode

    def forbidden(*_args: object, **_kwargs: object) -> object:
        raise AssertionError("classification attempted a mutation")

    status_refreshes: list[bool] = []
    original_status = wrkslots._GitVcs.status

    def observe_status(
        vcs: wrkslots._GitVcs,
        target: Path,
        cache_globs: Sequence[str] = (),
        *,
        refresh_index: bool = True,
    ) -> str:
        status_refreshes.append(refresh_index)
        return original_status(
            vcs, target, cache_globs, refresh_index=refresh_index
        )

    empty_census = lambda _paths, **_kwargs: wrkslots._ProcessPathCensus((), ())
    monkeypatch.setattr(wrkslots, "_mutation_locks", forbidden)
    for name in (
        "_atomic_write_json",
        "_clear_journal",
        "_remove_control_file",
        "_write_active_state",
        "_write_archive_state",
        "_write_event_file",
        "_write_journal",
    ):
        monkeypatch.setattr(wrkslots, name, forbidden)
    monkeypatch.setattr(wrkslots._GitVcs, "status", observe_status)
    monkeypatch.setattr(wrkslots, "_capture_lsof_process_path_census", empty_census)
    monkeypatch.setattr(
        wrkslots, "_capture_same_uid_process_path_census", empty_census
    )
    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "classify-ownerless-validate-batch",
            "--checkout",
            retained.relative_to(project).as_posix(),
            "--completed-record",
            record.relative_to(project).as_posix(),
            "--repository",
            repository.relative_to(project).as_posix(),
            "--format",
            "json",
        ]
    )
    assert rc == 0
    report = json.loads(capsys.readouterr().out)
    assert report["classifications"][0]["blocks_entry"] is False
    assert report["classifications"][0]["state"] == "terminal-retained"
    assert report["create_journals"][0]["dirty_checkouts"] == ["product"]
    assert status_refreshes and not any(status_refreshes)
    assert retained.is_dir()
    assert legacy.read_bytes() == before
    assert legacy.stat(follow_symlinks=False).st_ino == inode


@pytest.mark.ordinary_environment
def test_ownerless_validate_batch_removes_terminal_frozen_checkout(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout, record, target_sha = prepare_frozen_validation_checkout(
        project, repository, monkeypatch=monkeypatch
    )
    stub_validate_batch_censuses(monkeypatch)

    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "recover-ownerless-validate-batch",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--frozen-validate-checkout",
            str(checkout),
            "--completed-record",
            record.relative_to(project).as_posix(),
            "--repository",
            repository.relative_to(project).as_posix(),
            "--format",
            "json",
        ]
    )

    assert rc == 0
    report = json.loads(capsys.readouterr().out)
    assert report["retained"] == []
    assert report["removed"] == [{"checkout": str(checkout)}]
    assert report["shared_process_censuses"] == 1
    assert report["same_uid_process_censuses"] == 1
    assert not checkout.exists()
    config = wrkslots._load_config(str(project), "testhost")
    assert not wrkslots._journal_path(config).exists()
    assert list(control_directory(project).glob("*.journal")) == []
    assert not list(checkout.parent.glob(f".{checkout.name}.ownerless-validate.*"))
    removal_events = [
        item
        for item in wrkslots._load_events(config)
        if item["kind"] == "ownerless-validate-path-removed"
    ]
    assert len(removal_events) == 1
    event = removal_events[0]
    payload = wrkslots._as_mapping(event["payload"], "test removal payload")
    authorization = wrkslots._as_mapping(
        payload["authorization"], "test removal authorization"
    )
    assert authorization["target_kind"] == "frozen-checkout"
    assert authorization["head"] == target_sha
    evidence = wrkslots._as_mapping(
        authorization["evidence"], "test removal evidence"
    )
    assert evidence["target"] == target_sha
    assert len(
        wrkslots._as_list(
            authorization["stable_identity"], "test stable identity"
        )
    ) == 4


@pytest.mark.ordinary_environment
def test_frozen_validate_batch_closes_operation_owned_fds_before_censuses(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout, record, _target_sha = prepare_frozen_validation_checkout(
        project, repository, monkeypatch=monkeypatch, name="closed-fds"
    )
    census_stages: list[str] = []

    def assert_expected_target_fds(paths: Sequence[Path], *, stage: str) -> None:
        target_fds: list[str] = []
        for entry in (Path("/proc") / "self" / "fd").iterdir():
            try:
                observed = Path(os.readlink(entry).removesuffix(" (deleted)"))
            except OSError:
                continue
            if any(observed == target or target in observed.parents for target in paths):
                target_fds.append(f"{entry.name}={observed}")
        if stage == "shared":
            assert len(paths) == 1
            assert target_fds == []
        else:
            # The exclusion phase intentionally carries exactly one authenticated
            # payload FD across its fresh census. The production helper separately
            # verifies that descriptor's inode and rejects every other self-held FD.
            assert len(paths) >= 3
            assert len(target_fds) == 1
        census_stages.append(stage)

    def shared(
        paths: Sequence[Path], **_kwargs: object
    ) -> wrkslots._ProcessPathCensus:
        assert_expected_target_fds(paths, stage="shared")
        return wrkslots._ProcessPathCensus((), (), owner_cgroup_complete=False)

    def fresh(
        paths: Sequence[Path], **_kwargs: object
    ) -> wrkslots._ProcessPathCensus:
        assert_expected_target_fds(paths, stage="fresh")
        return wrkslots._ProcessPathCensus((), (), owner_cgroup_complete=False)

    monkeypatch.setattr(wrkslots, "_capture_lsof_process_path_census", shared)
    monkeypatch.setattr(
        wrkslots, "_capture_same_uid_process_path_census", fresh
    )

    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "recover-ownerless-validate-batch",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--frozen-validate-checkout",
            str(checkout),
            "--completed-record",
            record.relative_to(project).as_posix(),
            "--repository",
            repository.relative_to(project).as_posix(),
            "--format",
            "json",
        ]
    )

    assert rc == 0
    report = json.loads(capsys.readouterr().out)
    assert report["removed"] == [{"checkout": str(checkout)}]
    assert report["retained"] == []
    assert census_stages == ["shared", "fresh"]
    config = wrkslots._load_config(str(project), "testhost")
    assert list(control_directory(project).glob("*.journal")) == []
    assert len(
        [
            item
            for item in wrkslots._load_events(config)
            if item["kind"] == "ownerless-validate-path-removed"
        ]
    ) == 1


@pytest.mark.ordinary_environment
def test_historical_frozen_entry_classification_closes_operation_owned_fds(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout, record, _sidecar, _target_sha = (
        prepare_run1773_historical_frozen_checkout(
            project, repository, monkeypatch=monkeypatch, name="closed-fds"
        )
    )
    census_stages: list[str] = []

    def assert_expected_target_fds(paths: Sequence[Path], *, stage: str) -> None:
        target_fds: list[str] = []
        for entry in (Path("/proc") / "self" / "fd").iterdir():
            try:
                observed = Path(os.readlink(entry).removesuffix(" (deleted)"))
            except OSError:
                continue
            if any(observed == target or target in observed.parents for target in paths):
                target_fds.append(f"{entry.name}={observed}")
        assert len(paths) == 1
        assert target_fds == []
        census_stages.append(stage)

    def shared(
        paths: Sequence[Path], **_kwargs: object
    ) -> wrkslots._ProcessPathCensus:
        assert_expected_target_fds(paths, stage="shared")
        return wrkslots._ProcessPathCensus((), (), owner_cgroup_complete=False)

    def fresh(
        paths: Sequence[Path], **_kwargs: object
    ) -> wrkslots._ProcessPathCensus:
        assert_expected_target_fds(paths, stage="fresh")
        return wrkslots._ProcessPathCensus((), (), owner_cgroup_complete=False)

    monkeypatch.setattr(wrkslots, "_capture_lsof_process_path_census", shared)
    monkeypatch.setattr(
        wrkslots, "_capture_same_uid_process_path_census", fresh
    )

    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "recover-ownerless-validate-batch",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--frozen-validate-checkout",
            str(checkout),
            "--completed-record",
            record.relative_to(project).as_posix(),
            "--repository",
            repository.relative_to(project).as_posix(),
            "--format",
            "json",
        ]
    )

    assert rc == 0
    report = json.loads(capsys.readouterr().out)
    assert report["removed"] == []
    assert report["retained"][0]["blocks_entry"] is False
    assert census_stages == ["shared", "fresh"]
    config = wrkslots._load_config(str(project), "testhost")
    assert list(control_directory(project).glob("*.journal")) == []
    assert not any(
        item["kind"] == "ownerless-validate-path-removed"
        for item in wrkslots._load_events(config)
    )


def test_frozen_validate_batch_does_not_ignore_current_process_census_use(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout, record, _target_sha = prepare_frozen_validation_checkout(
        project, repository, monkeypatch=monkeypatch, name="self-use"
    )

    monkeypatch.setattr(
        wrkslots,
        "_capture_lsof_process_path_census",
        lambda paths, **_kwargs: wrkslots._ProcessPathCensus(
            (),
            ((os.getpid(), str(paths[0]), "fd", str(paths[0])),),
            owner_cgroup_complete=False,
        ),
    )
    monkeypatch.setattr(
        wrkslots,
        "_capture_same_uid_process_path_census",
        lambda _paths, **_kwargs: wrkslots._ProcessPathCensus(
            (), (), owner_cgroup_complete=False
        ),
    )

    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "recover-ownerless-validate-batch",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--frozen-validate-checkout",
            str(checkout),
            "--completed-record",
            record.relative_to(project).as_posix(),
            "--repository",
            repository.relative_to(project).as_posix(),
            "--format",
            "json",
        ]
    )

    assert rc == 0
    report = json.loads(capsys.readouterr().out)
    assert report["removed"] == []
    assert report["retained"][0]["blocks_entry"] is True
    assert f"live process {os.getpid()}" in report["retained"][0]["reason"]
    assert checkout.is_dir()
    assert not (control_directory(project) / "ACTIVE.testhost.journal").exists()
    assert not any(
        item["kind"] == "ownerless-validate-path-removed"
        for item in wrkslots._load_events(
            wrkslots._load_config(str(project), "testhost")
        )
    )


@pytest.mark.ordinary_environment
def test_frozen_validate_batch_fresh_census_does_not_ignore_current_process(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout, record, _sidecar, _target_sha = (
        prepare_run1773_historical_frozen_checkout(
            project, repository, monkeypatch=monkeypatch, name="late-self-use"
        )
    )
    monkeypatch.setattr(
        wrkslots,
        "_capture_lsof_process_path_census",
        lambda _paths, **_kwargs: wrkslots._ProcessPathCensus(
            (), (), owner_cgroup_complete=False
        ),
    )
    monkeypatch.setattr(
        wrkslots,
        "_capture_same_uid_process_path_census",
        lambda paths, **_kwargs: wrkslots._ProcessPathCensus(
            (),
            ((os.getpid(), str(paths[0]), "fd", str(paths[0])),),
            owner_cgroup_complete=False,
        ),
    )

    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "recover-ownerless-validate-batch",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--frozen-validate-checkout",
            str(checkout),
            "--completed-record",
            record.relative_to(project).as_posix(),
            "--repository",
            repository.relative_to(project).as_posix(),
            "--format",
            "json",
        ]
    )

    assert rc == 0
    report = json.loads(capsys.readouterr().out)
    assert report["removed"] == []
    assert report["same_uid_process_censuses"] == 1
    assert report["retained"][0]["blocks_entry"] is True
    assert f"live process {os.getpid()}" in report["retained"][0]["reason"]
    assert checkout.is_dir()
    assert not (control_directory(project) / "ACTIVE.testhost.journal").exists()
    assert not any(
        item["kind"] == "ownerless-validate-path-removed"
        for item in wrkslots._load_events(
            wrkslots._load_config(str(project), "testhost")
        )
    )


@pytest.mark.parametrize(
    "relative",
    (
        "validate-fresh-wrong-parent",
        ".project-frozen-validate/not-a-validation-checkout",
        ".project-frozen-validate/nested/validate-fresh-too-deep",
    ),
)
def test_frozen_validate_checkout_refuses_every_noncanonical_path(
    tmp_path: Path, relative: str
) -> None:
    project, repository, _remote = make_project(tmp_path)
    candidate = tmp_path / relative
    candidate.mkdir(parents=True)
    refused = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--frozen-validate-checkout",
        str(candidate),
        "--completed-record",
        "ignored/validate/runs/unused.json",
        "--repository",
        repository.relative_to(project).as_posix(),
    )

    assert refused.returncode == 3
    assert "outside the exact sibling validation directory" in refused.stderr
    assert "preserve it or move it under" in refused.stderr
    assert candidate.is_dir()


@pytest.mark.parametrize("alias", ("dot", "double-slash", "trailing-slash"))
def test_frozen_validate_checkout_refuses_noncanonical_lexical_aliases(
    tmp_path: Path, alias: str
) -> None:
    project, repository, _remote = make_project(tmp_path)
    parent = project.parent / f".{project.name}-frozen-validate"
    parent.mkdir(mode=0o700)
    candidate = parent / "validate-fresh-alias"
    candidate.mkdir(mode=0o700)
    raw = {
        "dot": f"{parent}/./{candidate.name}",
        "double-slash": f"{parent}//{candidate.name}",
        "trailing-slash": f"{candidate}/",
    }[alias]

    refused = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--frozen-validate-checkout",
        raw,
        "--completed-record",
        "ignored/validate/runs/unused.json",
        "--repository",
        repository.relative_to(project).as_posix(),
    )

    assert refused.returncode == 3
    assert "exact canonical absolute spelling" in refused.stderr
    assert candidate.is_dir()
    assert not (control_directory(project) / "ACTIVE.testhost.journal").exists()


@pytest.mark.ordinary_environment
@pytest.mark.parametrize(
    ("field", "replacement", "message"),
    (
        ("kind", "validate", "authoritative frozen-validation handle"),
        ("admission", "ci-hub validate-lock", "authoritative frozen-validation handle"),
        ("validation_kind", "validate", "authoritative frozen-validation handle"),
        ("producer", "another producer", "authoritative frozen-validation handle"),
        ("temporary_checkout", False, "checkout is not temporary"),
        ("checkout", "/tmp/not-the-checkout", "exact checkout"),
        ("target", "0" * 40, "HEAD changed"),
        ("source_checkout", "/tmp/not-the-source", "exact source checkout"),
    ),
)
def test_frozen_validate_checkout_binds_terminal_record_fields(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    field: str,
    replacement: object,
    message: str,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout, record, _target_sha = prepare_frozen_validation_checkout(
        project,
        repository,
        monkeypatch=monkeypatch,
        name=field.replace("_", "-"),
    )
    value = json.loads(record.read_text(encoding="utf-8"))
    value[field] = replacement
    record.write_text(json.dumps(value), encoding="utf-8")
    stub_validate_batch_censuses(monkeypatch)

    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "recover-ownerless-validate-batch",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--frozen-validate-checkout",
            str(checkout),
            "--completed-record",
            record.relative_to(project).as_posix(),
            "--repository",
            repository.relative_to(project).as_posix(),
            "--format",
            "json",
        ]
    )

    assert rc == 0
    report = json.loads(capsys.readouterr().out)
    assert report["removed"] == []
    assert message in report["retained"][0]["reason"]
    assert checkout.is_dir()


@pytest.mark.ordinary_environment
@pytest.mark.parametrize(
    ("state", "result"),
    (
        ("killed", "terminated"),
        ("not-run", "not-run"),
        ("refused", "systemd-launch-refused"),
    ),
)
def test_frozen_validate_rejects_counterfeit_minimal_terminal_records(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    state: str,
    result: str,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout, record, target_sha = prepare_frozen_validation_checkout(
        project,
        repository,
        monkeypatch=monkeypatch,
        name=f"counterfeit-{state}",
    )
    record.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "kind": "frozen-validate",
                "producer": "ci-hub/validate/run_registry.py",
                "admission": "frozen-validate",
                "validation_kind": "frozen-validate",
                "temporary_checkout": True,
                "checkout": str(checkout),
                "source_checkout": str(repository),
                "target": target_sha,
                "state": state,
                "result": result,
            }
        ),
        encoding="utf-8",
    )
    stub_validate_batch_censuses(monkeypatch)

    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "recover-ownerless-validate-batch",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--frozen-validate-checkout",
            str(checkout),
            "--completed-record",
            record.relative_to(project).as_posix(),
            "--repository",
            repository.relative_to(project).as_posix(),
            "--format",
            "json",
        ]
    )

    assert rc == 0
    report = json.loads(capsys.readouterr().out)
    assert report["removed"] == []
    assert "canonical frozen validation record parser refused" in report["retained"][0][
        "reason"
    ]
    assert checkout.is_dir()
    config = wrkslots._load_config(str(project), "testhost")
    assert not wrkslots._journal_path(config).exists()
    assert not any(
        event["kind"] == "ownerless-validate-path-removed"
        for event in wrkslots._load_events(config)
    )


@pytest.mark.parametrize(
    ("failure", "message"),
    (
        ("config-root", "project root does not share the authenticated outer authority"),
        ("consumer-root", "agent-utils superproject is not a Git worktree root"),
        ("authority-root", "consumer superproject is not a Git worktree root"),
        ("agent-superproject", "Git returned a relative path"),
        ("outer-superproject", "Git returned a relative path"),
        ("agent-gitlink", "agent-utils HEAD differs"),
        ("outer-gitlink", "enclosing checkout HEAD differs"),
    ),
)
def test_frozen_parser_authority_requires_both_enclosing_gitlinks(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    failure: str,
    message: str,
) -> None:
    authority = tmp_path / "project"
    consumer = authority / "consumer-a"
    agent_checkout = consumer / "agent-utils"
    common = authority / ".git"
    wrong_root = tmp_path / "wrong-root"
    other_common = wrong_root / ".git"
    loaded = agent_checkout / "py" / "wrkslots" / "cli.py"
    common.mkdir(parents=True)
    other_common.mkdir(parents=True)
    loaded.parent.mkdir(parents=True)
    loaded.write_text("# fixture\n", encoding="utf-8")
    authority.mkdir(exist_ok=True)
    monkeypatch.setattr(wrkslots, "__file__", str(loaded))

    def fake_git_text(
        repository: Path, arguments: Sequence[str], *, label: str
    ) -> str:
        del label
        if "--show-toplevel" in arguments:
            if repository == consumer and failure == "consumer-root":
                return str(authority)
            if repository == authority and failure == "authority-root":
                return str(authority.parent)
            return {
                loaded.parent: str(agent_checkout),
                consumer: str(consumer),
                authority: str(authority),
                wrong_root: str(wrong_root),
            }[repository]
        if "--show-superproject-working-tree" in arguments:
            if repository == agent_checkout and failure == "agent-superproject":
                return "relative-consumer"
            if repository == consumer and failure == "outer-superproject":
                return "relative-authority"
            return {agent_checkout: str(consumer), consumer: str(authority)}[
                repository
            ]
        if "--git-common-dir" in arguments:
            return str(other_common if repository == wrong_root else common)
        raise AssertionError(arguments)

    agent_head = "a" * 40
    consumer_head = "b" * 40
    authority_head = "c" * 40

    def fake_head(repository: Path, _label: str) -> str:
        return {
            agent_checkout: agent_head,
            consumer: consumer_head,
            authority: authority_head,
        }[repository]

    def fake_gitlink(
        repository: Path,
        _commit: str,
        relative: str,
        *,
        label: str,
    ) -> str:
        del label
        if repository == consumer and relative == "agent-utils":
            return "d" * 40 if failure == "agent-gitlink" else agent_head
        if repository == authority and relative == consumer.name:
            return "d" * 40 if failure == "outer-gitlink" else consumer_head
        raise AssertionError((repository, relative))

    monkeypatch.setattr(wrkslots, "_trusted_git_text", fake_git_text)
    monkeypatch.setattr(wrkslots, "_trusted_git_head", fake_head)
    monkeypatch.setattr(wrkslots, "_trusted_gitlink", fake_gitlink)
    config_root = wrong_root if failure == "config-root" else authority

    with pytest.raises(wrkslots.Refusal, match=message):
        wrkslots._frozen_validation_authority_commit(
            wrkslots.Config(
                root=config_root,
                config_path=config_root / "wrkslots.toml",
                worktrees=config_root / "worktrees",
                control=config_root / "worktrees",
                machine="testhost",
                default_remote="origin",
                default_landed_ref="refs/remotes/origin/main",
                heartbeat_ttl_seconds=60,
                liveness_command=config_root / "liveness.py",
            )
        )


def test_frozen_parser_authority_accepts_linked_outer_worktree(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    primary = tmp_path / "project"
    authority = tmp_path / "slots" / "tool-root"
    consumer = authority / "consumer"
    agent_checkout = consumer / "agent-utils"
    loaded = agent_checkout / "py" / "wrkslots" / "cli.py"
    common = primary / ".git"
    loaded.parent.mkdir(parents=True)
    loaded.write_text("# fixture\n", encoding="utf-8")
    common.mkdir(parents=True)
    monkeypatch.setattr(wrkslots, "__file__", str(loaded))

    def fake_git_text(
        repository: Path, arguments: Sequence[str], *, label: str
    ) -> str:
        del label
        operation = arguments[-1]
        if operation == "--show-toplevel":
            return {
                loaded.parent: str(agent_checkout),
                consumer: str(consumer),
                authority: str(authority),
                primary: str(primary),
            }[repository]
        if operation == "--show-superproject-working-tree":
            return {agent_checkout: str(consumer), consumer: str(authority)}[
                repository
            ]
        if operation == "--git-common-dir":
            return {
                authority: str(common),
                primary: str(common),
            }[repository]
        raise AssertionError((repository, arguments))

    agent_head = "a" * 40
    consumer_head = "b" * 40
    authority_head = "c" * 40

    monkeypatch.setattr(wrkslots, "_trusted_git_text", fake_git_text)
    monkeypatch.setattr(
        wrkslots,
        "_trusted_git_head",
        lambda repository, _label: {
            agent_checkout: agent_head,
            consumer: consumer_head,
            authority: authority_head,
        }[repository],
    )

    def fake_gitlink(
        repository: Path,
        _commit: str,
        relative: str,
        *,
        label: str,
    ) -> str:
        del label
        return {
            (consumer, "agent-utils"): agent_head,
            (authority, "consumer"): consumer_head,
        }[(repository, relative)]

    monkeypatch.setattr(wrkslots, "_trusted_gitlink", fake_gitlink)
    config = wrkslots.Config(
        root=primary,
        config_path=primary / "wrkslots.toml",
        worktrees=primary / "worktrees",
        control=primary / "worktrees",
        machine="testhost",
        default_remote="origin",
        default_landed_ref="refs/remotes/origin/main",
        heartbeat_ttl_seconds=60,
        liveness_command=primary / "liveness.py",
    )

    assert wrkslots._frozen_validation_authority_commit(config) == (
        authority,
        authority_head,
    )


@pytest.mark.ordinary_environment
def test_frozen_parser_authority_accepts_real_nested_linked_worktree(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    agent_source = tmp_path / "agent-source"
    agent_source.mkdir()
    git(agent_source, "init", "--initial-branch=main")
    git(agent_source, "config", "user.name", "Wrkslots Test")
    git(agent_source, "config", "user.email", "wrkslots@example.invalid")
    source_module = agent_source / "py" / "wrkslots" / "cli.py"
    source_module.parent.mkdir(parents=True)
    source_module.write_text("# fixture\n", encoding="utf-8")
    git(agent_source, "add", "--", source_module.relative_to(agent_source).as_posix())
    git(agent_source, "commit", "-m", "agent fixture")

    consumer_source = tmp_path / "consumer-source"
    consumer_source.mkdir()
    git(consumer_source, "init", "--initial-branch=main")
    git(consumer_source, "config", "user.name", "Wrkslots Test")
    git(consumer_source, "config", "user.email", "wrkslots@example.invalid")
    git(
        consumer_source,
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        str(agent_source),
        "agent-utils",
    )
    git(consumer_source, "commit", "-am", "consumer fixture")

    primary = tmp_path / "project"
    primary.mkdir()
    git(primary, "init", "--initial-branch=main")
    git(primary, "config", "user.name", "Wrkslots Test")
    git(primary, "config", "user.email", "wrkslots@example.invalid")
    git(
        primary,
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        str(consumer_source),
        "consumer",
    )
    git(primary, "commit", "-am", "outer fixture")

    authority = tmp_path / "slots" / "tool-root"
    authority.parent.mkdir()
    git(primary, "worktree", "add", "--detach", str(authority), "HEAD")
    git(
        authority,
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "update",
        "--init",
        "--recursive",
    )
    loaded = authority / "consumer" / "agent-utils" / "py" / "wrkslots" / "cli.py"
    monkeypatch.setattr(wrkslots, "__file__", str(loaded))
    config = wrkslots.Config(
        root=primary,
        config_path=primary / "wrkslots.toml",
        worktrees=primary / "worktrees",
        control=primary / "worktrees",
        machine="testhost",
        default_remote="origin",
        default_landed_ref="refs/remotes/origin/main",
        heartbeat_ttl_seconds=60,
        liveness_command=primary / "liveness.py",
    )

    assert wrkslots._frozen_validation_authority_commit(config) == (
        authority,
        git(authority, "rev-parse", "HEAD").stdout.strip(),
    )


@pytest.mark.ordinary_environment
@pytest.mark.parametrize(
    "relative",
    wrkslots._FROZEN_VALIDATION_PARSER_FILES,
)
@pytest.mark.parametrize(
    "tamper",
    ("before-bytes", "before-symlink", "after-bytes", "after-symlink"),
)
def test_frozen_parser_refuses_each_module_tampered_before_or_after_inspect(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    relative: str,
    tamper: str,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout, record, _target_sha = prepare_frozen_validation_checkout(
        project,
        repository,
        monkeypatch=monkeypatch,
        name=f"parser-{Path(relative).stem}-{tamper}",
    )
    stub_validate_batch_censuses(monkeypatch)
    module = project / relative
    original = module.read_bytes()

    def mutate() -> None:
        if tamper.endswith("bytes"):
            module.write_bytes(original + b"\n# unauthorized change\n")
            return
        replacement = module.with_name(f"{module.name}.replacement")
        replacement.write_bytes(original)
        module.unlink()
        module.symlink_to(replacement.name)

    if tamper.startswith("before"):
        mutate()
    else:
        bounded = wrkslots._run_bounded_read_only_command
        mutated = False

        def mutate_after_parser(
            command: Sequence[str],
            *,
            timeout_seconds: float = 60.0,
            stdout_limit: int = 16 * 1024 * 1024,
            stderr_limit: int = 64 * 1024,
            env_overrides: Mapping[str, str] | None = None,
            input_data: bytes | None = None,
            trusted_executables: Sequence[wrkslots._TrustedExecutablePath] = (),
        ) -> tuple[int, bytes, bytes]:
            nonlocal mutated
            result = bounded(
                command,
                timeout_seconds=timeout_seconds,
                stdout_limit=stdout_limit,
                stderr_limit=stderr_limit,
                env_overrides=env_overrides,
                input_data=input_data,
                trusted_executables=trusted_executables,
            )
            if (
                not mutated
                and wrkslots._FROZEN_VALIDATION_PARSER_BOOTSTRAP in command
            ):
                mutated = True
                mutate()
            return result

        monkeypatch.setattr(
            wrkslots, "_run_bounded_read_only_command", mutate_after_parser
        )

    rc = run_frozen_validation_batch(project, repository, checkout, record)

    assert rc == 0
    report = json.loads(capsys.readouterr().out)
    assert report["removed"] == []
    assert "frozen validation parser module" in report["retained"][0]["reason"]
    assert checkout.is_dir()
    config = wrkslots._load_config(str(project), "testhost")
    assert not wrkslots._journal_path(config).exists()
    assert not any(
        event["kind"] == "ownerless-validate-path-removed"
        for event in wrkslots._load_events(config)
    )


@pytest.mark.ordinary_environment
@pytest.mark.parametrize(
    ("failure", "expected"),
    (
        ("timeout", "exceeded 15s"),
        ("oversized", "stdout exceeded 1048576 bytes"),
        ("nonzero", "canonical frozen validation record parser refused"),
        ("stderr", "canonical frozen validation record parser refused"),
        ("invalid-json", "returned invalid JSON"),
    ),
)
def test_frozen_parser_failures_preserve_checkout_without_journal(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    failure: str,
    expected: str,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout, record, target_sha = prepare_frozen_validation_checkout(
        project,
        repository,
        monkeypatch=monkeypatch,
        name=f"parser-{failure}",
    )
    stub_validate_batch_censuses(monkeypatch)
    bounded = wrkslots._run_bounded_read_only_command

    def fail_parser(
        command: Sequence[str],
        *,
        timeout_seconds: float = 60.0,
        stdout_limit: int = 16 * 1024 * 1024,
        stderr_limit: int = 64 * 1024,
        env_overrides: Mapping[str, str] | None = None,
        input_data: bytes | None = None,
        trusted_executables: Sequence[wrkslots._TrustedExecutablePath] = (),
    ) -> tuple[int, bytes, bytes]:
        if wrkslots._FROZEN_VALIDATION_PARSER_BOOTSTRAP not in command:
            return bounded(
                command,
                timeout_seconds=timeout_seconds,
                stdout_limit=stdout_limit,
                stderr_limit=stderr_limit,
                env_overrides=env_overrides,
                input_data=input_data,
                trusted_executables=trusted_executables,
            )
        if failure == "timeout":
            raise wrkslots.Refusal("privileged read-only census exceeded 15s")
        if failure == "oversized":
            raise wrkslots.Refusal(
                "privileged read-only census stdout exceeded 1048576 bytes"
            )
        if failure == "nonzero":
            return 2, b"", b"run_registry: REFUSED: fixture refusal\n"
        if failure == "stderr":
            return (
                0,
                json.dumps(
                    {
                        "kind": "frozen-validate",
                        "state": "completed",
                        "target": target_sha,
                        "checkout": str(checkout),
                    }
                ).encode(),
                b"unexpected diagnostic\n",
            )
        return 0, b"{", b""

    monkeypatch.setattr(wrkslots, "_run_bounded_read_only_command", fail_parser)

    rc = run_frozen_validation_batch(project, repository, checkout, record)

    assert rc == 0
    report = json.loads(capsys.readouterr().out)
    assert report["removed"] == []
    assert expected in report["retained"][0]["reason"]
    assert checkout.is_dir()
    config = wrkslots._load_config(str(project), "testhost")
    assert not wrkslots._journal_path(config).exists()
    assert not any(
        event["kind"] == "ownerless-validate-path-removed"
        for event in wrkslots._load_events(config)
    )


@pytest.mark.ordinary_environment
@pytest.mark.parametrize("mutation", ("append", "checkout"))
def test_frozen_parser_refuses_record_mutation_after_inspect(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    mutation: str,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout, record, _target_sha = prepare_frozen_validation_checkout(
        project,
        repository,
        monkeypatch=monkeypatch,
        name=f"record-mutated-after-inspect-{mutation}",
    )
    stub_validate_batch_censuses(monkeypatch)
    bounded = wrkslots._run_bounded_read_only_command
    mutated = False

    def mutate_record_after_parser(
        command: Sequence[str],
        *,
        timeout_seconds: float = 60.0,
        stdout_limit: int = 16 * 1024 * 1024,
        stderr_limit: int = 64 * 1024,
        env_overrides: Mapping[str, str] | None = None,
        input_data: bytes | None = None,
        trusted_executables: Sequence[wrkslots._TrustedExecutablePath] = (),
    ) -> tuple[int, bytes, bytes]:
        nonlocal mutated
        result = bounded(
            command,
            timeout_seconds=timeout_seconds,
            stdout_limit=stdout_limit,
            stderr_limit=stderr_limit,
            env_overrides=env_overrides,
            input_data=input_data,
            trusted_executables=trusted_executables,
        )
        if not mutated and wrkslots._FROZEN_VALIDATION_PARSER_BOOTSTRAP in command:
            mutated = True
            if mutation == "checkout":
                value = json.loads(record.read_text(encoding="utf-8"))
                value["checkout"] = str(checkout.with_name("replacement-checkout"))
                record.write_text(json.dumps(value), encoding="utf-8")
            else:
                record.write_bytes(record.read_bytes() + b"\n")
        return result

    monkeypatch.setattr(
        wrkslots, "_run_bounded_read_only_command", mutate_record_after_parser
    )

    rc = run_frozen_validation_batch(project, repository, checkout, record)

    assert rc == 0
    report = json.loads(capsys.readouterr().out)
    assert report["removed"] == []
    assert "changed while its producer parsed it" in report["retained"][0]["reason"]
    assert checkout.is_dir()
    config = wrkslots._load_config(str(project), "testhost")
    assert not wrkslots._journal_path(config).exists()
    assert not any(
        event["kind"] == "ownerless-validate-path-removed"
        for event in wrkslots._load_events(config)
    )


@pytest.mark.ordinary_environment
def test_frozen_parser_reads_immutable_initial_snapshot_during_live_restore(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout, record, target_sha = prepare_frozen_validation_checkout(
        project,
        repository,
        monkeypatch=monkeypatch,
        name="immutable-snapshot",
    )
    config = wrkslots._load_config(str(project), "testhost")
    initial = record.read_bytes()
    bounded = wrkslots._run_bounded_read_only_command
    observed_snapshot = False

    def change_and_restore_live_record(
        command: Sequence[str],
        *,
        timeout_seconds: float = 60.0,
        stdout_limit: int = 16 * 1024 * 1024,
        stderr_limit: int = 64 * 1024,
        env_overrides: Mapping[str, str] | None = None,
        input_data: bytes | None = None,
        trusted_executables: Sequence[wrkslots._TrustedExecutablePath] = (),
    ) -> tuple[int, bytes, bytes]:
        nonlocal observed_snapshot
        if wrkslots._FROZEN_VALIDATION_PARSER_BOOTSTRAP not in command:
            return bounded(
                command,
                timeout_seconds=timeout_seconds,
                stdout_limit=stdout_limit,
                stderr_limit=stderr_limit,
                env_overrides=env_overrides,
                input_data=input_data,
                trusted_executables=trusted_executables,
            )
        arguments = list(command)
        snapshot = Path(arguments[arguments.index("--path") + 1])
        observed_snapshot = True
        assert snapshot != record
        assert snapshot.read_bytes() == initial
        assert stat.S_IMODE(snapshot.stat().st_mode) == 0o400
        assert stat.S_IMODE(snapshot.parent.stat().st_mode) == 0o500
        transient = json.loads(record.read_text(encoding="utf-8"))
        transient["checkout"] = ""
        record.write_text(json.dumps(transient), encoding="utf-8")
        try:
            result = bounded(
                command,
                timeout_seconds=timeout_seconds,
                stdout_limit=stdout_limit,
                stderr_limit=stderr_limit,
                env_overrides=env_overrides,
                input_data=input_data,
                trusted_executables=trusted_executables,
            )
        finally:
            record.write_bytes(initial)
        assert result[0] == 0
        assert snapshot.read_bytes() == initial
        return result

    monkeypatch.setattr(
        wrkslots, "_run_bounded_read_only_command", change_and_restore_live_record
    )

    assert wrkslots._terminal_validation_record(
        config,
        record.relative_to(project).as_posix(),
        checkout,
        "frozen-checkout",
        repository=repository,
    ) == (
        record.relative_to(project).as_posix(),
        hashlib.sha256(initial).hexdigest(),
        target_sha,
    )
    assert observed_snapshot
    assert record.read_bytes() == initial


@pytest.mark.ordinary_environment
@pytest.mark.parametrize(
    "boundary",
    (
        "after-ownerless-validate-journal",
        "after-ownerless-validate-path-fence-before-journal",
        "after-ownerless-validate-path-fence",
        "after-ownerless-validate-removing",
        "after-ownerless-validate-exclusion-directory",
        "after-ownerless-validate-exclusion-move",
        "after-ownerless-validate-exclusion-seal",
        "after-ownerless-validate-remove",
        "after-ownerless-validate-removal-event",
    ),
)
def test_frozen_validate_checkout_recovers_each_durable_crash_boundary(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, boundary: str
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout, record, _target_sha = prepare_frozen_validation_checkout(
        project,
        repository,
        monkeypatch=monkeypatch,
        name=boundary.removeprefix("after-"),
    )
    arguments = (
        "recover",
        "--coordinator-authorized",
        "--coordinator-pid",
        str(os.getpid()),
        "--frozen-validate-checkout",
        str(checkout),
        "--completed-record",
        record.relative_to(project).as_posix(),
        "--repository",
        repository.relative_to(project).as_posix(),
    )
    interrupted = raw_command_with_census_authority_stub(
        project,
        *arguments,
        env={"WRKSLOTS_TEST_INTERRUPT": boundary},
    )
    assert interrupted.returncode == 86, interrupted.stderr
    journal = control_directory(project) / "ACTIVE.testhost.journal"
    assert journal.is_file()
    assert len(list(control_directory(project).glob("*.journal"))) == 1
    journal_value = json.loads(journal.read_text(encoding="utf-8"))
    assert journal_value["phase"] == {
        "after-ownerless-validate-journal": "prepared",
        "after-ownerless-validate-path-fence-before-journal": "prepared",
        "after-ownerless-validate-path-fence": "fenced",
        "after-ownerless-validate-removing": "removing",
        "after-ownerless-validate-exclusion-directory": "removing",
        "after-ownerless-validate-exclusion-move": "removing",
        "after-ownerless-validate-exclusion-seal": "removing",
        "after-ownerless-validate-remove": "removed",
        "after-ownerless-validate-removal-event": "removed",
    }[boundary]
    before_removal_events = [
        item
        for item in wrkslots._load_events(
            wrkslots._load_config(str(project), "testhost")
        )
        if item["kind"] == "ownerless-validate-path-removed"
    ]
    assert len(before_removal_events) == (
        1 if boundary == "after-ownerless-validate-removal-event" else 0
    )

    resumed = raw_command_with_census_authority_stub(
        project, "recover", "--coordinator-pid", str(os.getpid())
    )

    assert resumed.returncode == 0, resumed.stderr
    assert not checkout.exists()
    assert not journal.exists()
    assert list(control_directory(project).glob("*.journal")) == []
    removal_events = [
        item
        for item in wrkslots._load_events(
            wrkslots._load_config(str(project), "testhost")
        )
        if item["kind"] == "ownerless-validate-path-removed"
    ]
    assert len(removal_events) == 1


@pytest.mark.ordinary_environment
@pytest.mark.parametrize("mutation", ("guard-identity", "source-alias"))
def test_frozen_validate_recovery_refuses_exclusion_identity_tampering(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    mutation: str,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout, record, _target_sha = prepare_frozen_validation_checkout(
        project,
        repository,
        monkeypatch=monkeypatch,
        name=f"exclusion-{mutation}",
    )
    interrupted = raw_command_with_census_authority_stub(
        project,
        "recover",
        "--coordinator-authorized",
        "--coordinator-pid",
        str(os.getpid()),
        "--frozen-validate-checkout",
        str(checkout),
        "--completed-record",
        record.relative_to(project).as_posix(),
        "--repository",
        repository.relative_to(project).as_posix(),
        env={"WRKSLOTS_TEST_INTERRUPT": "after-ownerless-validate-exclusion-move"},
    )
    assert interrupted.returncode == 86, interrupted.stderr
    journal = control_directory(project) / "ACTIVE.testhost.journal"
    original = journal.read_bytes()
    value = json.loads(original)
    exclusion = value["exclusion"]
    assert isinstance(exclusion, dict)
    if mutation == "guard-identity":
        identity = exclusion["guard_identity"]
        assert isinstance(identity, list)
        identity[1] += 1
    else:
        source_alias = exclusion["source_alias"]
        assert isinstance(source_alias, str)
        exclusion["source_alias"] = f"{source_alias}.replacement"
    journal.write_text(
        json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )

    refused = raw_command_with_census_authority_stub(
        project, "recover", "--coordinator-pid", str(os.getpid())
    )

    assert refused.returncode == 3
    assert (
        "standalone recovery journal differs from append-only operation evidence"
        in refused.stderr
    )
    assert not checkout.exists()
    journal.write_bytes(original)

    resumed = raw_command_with_census_authority_stub(
        project, "recover", "--coordinator-pid", str(os.getpid())
    )

    assert resumed.returncode == 0, resumed.stderr
    assert not journal.exists()
    assert not checkout.exists()


@pytest.mark.ordinary_environment
def test_frozen_validate_batch_accepts_exact_validation_removal_proof(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    commit_validation_removal_schema(repository, schema_version=3)
    checkout, record, _target_sha = prepare_frozen_validation_checkout(
        project,
        repository,
        monkeypatch=monkeypatch,
        name="proof",
    )
    manifest, _artifacts = prepare_validation_removal_proof(
        project,
        checkout.name,
        checkout_path=checkout,
        run_record=record,
    )
    stub_validate_batch_censuses(monkeypatch)
    monkeypatch.setattr(
        wrkslots,
        "_root_owned_executable",
        lambda path, _label: wrkslots._TrustedExecutablePath(
            path.resolve(strict=True), ()
        ),
    )

    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "recover-ownerless-validate-batch",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--frozen-validate-checkout",
            str(checkout),
            "--completed-record",
            record.relative_to(project).as_posix(),
            "--repository",
            repository.relative_to(project).as_posix(),
            "--validation-proof-manifest",
            manifest.relative_to(project).as_posix(),
            "--format",
            "json",
        ]
    )

    assert rc == 0
    report = json.loads(capsys.readouterr().out)
    assert report["retained"] == [], report
    assert report["removed"] == [{"checkout": str(checkout)}]
    assert not checkout.exists()


@pytest.mark.ordinary_environment
@pytest.mark.parametrize(
    ("mutation", "message"),
    (
        ("external-bytes", "service-result changed bytes"),
        ("external-inode", "scorecard-handoff regular-file identity changed"),
    ),
)
def test_frozen_validate_rebinds_external_proof_after_exclusion_census(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    mutation: str,
    message: str,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    commit_validation_removal_schema(repository, schema_version=3)
    checkout, record, _target_sha = prepare_frozen_validation_checkout(
        project,
        repository,
        monkeypatch=monkeypatch,
        name=f"proof-{mutation}",
    )
    manifest, artifacts = prepare_validation_removal_proof(
        project,
        checkout.name,
        checkout_path=checkout,
        run_record=record,
    )
    stub_validate_batch_censuses(monkeypatch)
    observed_guards: list[Path] = []

    def mutate_during_exclusion_census(
        _paths: Sequence[Path], *, budget: wrkslots._ReadOnlyCommandBudget
    ) -> wrkslots._ProcessPathCensus:
        del budget
        observed_guards.append(validation_exclusion_guard(project))
        if mutation == "external-bytes":
            service_result = artifacts["service-result"]
            contents = service_result.read_bytes()
            assert b'"profile": "full"' in contents
            service_result.write_bytes(
                contents.replace(b'"profile": "full"', b'"profile": "evil"', 1)
            )
        elif mutation == "external-inode":
            handoff = artifacts["scorecard-handoff"]
            replacement = handoff.with_name("replacement-handoff.json")
            replacement.write_bytes(handoff.read_bytes())
            os.replace(replacement, handoff)
        else:
            raise AssertionError(mutation)
        return wrkslots._ProcessPathCensus((), ())

    monkeypatch.setattr(
        wrkslots,
        "_capture_same_uid_process_path_census",
        mutate_during_exclusion_census,
    )

    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "recover-ownerless-validate-batch",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--frozen-validate-checkout",
            str(checkout),
            "--completed-record",
            record.relative_to(project).as_posix(),
            "--repository",
            repository.relative_to(project).as_posix(),
            "--validation-proof-manifest",
            manifest.relative_to(project).as_posix(),
            "--format",
            "json",
        ]
    )

    assert rc == 0
    report = json.loads(capsys.readouterr().out)
    assert report["removed"] == []
    assert len(report["retained"]) == 1
    assert message in report["retained"][0]["reason"]
    assert checkout.is_dir()
    assert (checkout / ".git").is_dir()
    assert len(observed_guards) == 1 and not observed_guards[0].exists()
    config = wrkslots._load_config(str(project), "testhost")
    assert not wrkslots._journal_path(config).exists()
    assert not any(
        item["kind"] == "ownerless-validate-path-removed"
        for item in wrkslots._load_events(config)
    )


@pytest.mark.ordinary_environment
def test_frozen_validate_excludes_late_same_uid_checkout_entry(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    commit_validation_removal_schema(repository, schema_version=3)
    checkout, record, _target_sha = prepare_frozen_validation_checkout(
        project, repository, monkeypatch=monkeypatch, name="late-same-uid"
    )
    manifest, _artifacts = prepare_validation_removal_proof(
        project,
        checkout.name,
        checkout_path=checkout,
        run_record=record,
    )
    stub_validate_batch_censuses(monkeypatch)
    attempted: list[tuple[int, int, int, str]] = []
    attacker: subprocess.Popen[str] | None = None

    def enter_after_final_check(point: str) -> None:
        nonlocal attacker
        if point != "after-ownerless-validate-final-check":
            return
        guard = validation_exclusion_guard(project)
        payload = guard / wrkslots._OWNERLESS_VALIDATION_EXCLUSION_ENTRY
        attacker = subprocess.Popen(
            [
                sys.executable,
                "-c",
                "import os,sys,time\n"
                "guard,payload,renamed=sys.argv[1:]\n"
                "results=[]\n"
                "try:\n"
                " os.chmod(guard,0o700); os.chdir(payload); os.chmod(guard,0); "
                "results.append(\"entered\")\n"
                "except OSError as error: results.append(f\"chmod/chdir={error.errno}\")\n"
                "for name,operation in ((\"rename\",lambda:os.rename(guard,renamed)),"
                "(\"reopen\",lambda:os.open(payload,os.O_RDONLY|os.O_DIRECTORY))):\n"
                " try: operation(); results.append(f\"{name}=opened\")\n"
                " except OSError as error: results.append(f\"{name}={error.errno}\")\n"
                "print(\",\".join(results),flush=True)\n"
                "time.sleep(60)\n",
                str(guard),
                str(payload),
                str(guard.with_name(f"{guard.name}.renamed")),
            ],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        assert attacker.stdout is not None
        attempted.append(
            (
                os.getuid(),
                guard.stat(follow_symlinks=False).st_uid,
                stat.S_IMODE(guard.stat(follow_symlinks=False).st_mode),
                attacker.stdout.readline().strip(),
            )
        )

    monkeypatch.setattr(wrkslots, "_interrupt_for_test", enter_after_final_check)
    try:
        rc = wrkslots.main(
            [
                "--project-root",
                str(project),
                "recover-ownerless-validate-batch",
                "--coordinator-authorized",
                "--coordinator-pid",
                str(os.getpid()),
                "--frozen-validate-checkout",
                str(checkout),
                "--completed-record",
                record.relative_to(project).as_posix(),
                "--repository",
                repository.relative_to(project).as_posix(),
                "--validation-proof-manifest",
                manifest.relative_to(project).as_posix(),
                "--format",
                "json",
            ]
        )

        assert rc == 0
        assert attempted == [(os.getuid(), 0, 0, "chmod/chdir=1,rename=1,reopen=13")]
        assert attacker is not None and attacker.poll() is None
        report = json.loads(capsys.readouterr().out)
        assert report["retained"] == []
        assert report["removed"] == [{"checkout": str(checkout)}]
        assert not checkout.exists()
    finally:
        if attacker is not None:
            terminate_process(attacker)


@pytest.mark.ordinary_environment
def test_frozen_validate_sealed_guard_fd_cannot_open_late_payload(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout, record, _target_sha = prepare_frozen_validation_checkout(
        project, repository, monkeypatch=monkeypatch, name="held-guard-fd"
    )
    stub_validate_batch_censuses(monkeypatch)
    holder: subprocess.Popen[str] | None = None
    attempts: list[str] = []

    def hold_guard_before_seal(point: str) -> None:
        nonlocal holder
        if point == "after-ownerless-validate-exclusion-directory":
            guard = validation_exclusion_guard(project)
            holder = subprocess.Popen(
                [
                    sys.executable,
                    "-c",
                    "import os,sys,time\n"
                    "fd=os.open(sys.argv[1],os.O_RDONLY|os.O_DIRECTORY)\n"
                    "print(\"ready\",flush=True)\n"
                    "sys.stdin.readline()\n"
                    "results=[]\n"
                    "try: os.fchmod(fd,0o700); results.append(\"fchmod=changed\")\n"
                    "except OSError as error: results.append(f\"fchmod={error.errno}\")\n"
                    "try: os.open(\"checkout\",os.O_RDONLY|os.O_DIRECTORY,dir_fd=fd)\n"
                    "except OSError as error: results.append(f\"openat={error.errno}\")\n"
                    "else: results.append(\"openat=opened\")\n"
                    "print(\",\".join(results),flush=True)\n"
                    "time.sleep(60)\n",
                    str(guard),
                ],
                text=True,
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            assert holder.stdout is not None
            assert holder.stdout.readline().strip() == "ready"
        elif point == "after-ownerless-validate-final-check":
            assert holder is not None and holder.stdin is not None
            assert holder.stdout is not None
            holder.stdin.write("go\n")
            holder.stdin.flush()
            attempts.append(holder.stdout.readline().strip())

    monkeypatch.setattr(wrkslots, "_interrupt_for_test", hold_guard_before_seal)
    try:
        rc = wrkslots.main(
            [
                "--project-root",
                str(project),
                "recover-ownerless-validate-batch",
                "--coordinator-authorized",
                "--coordinator-pid",
                str(os.getpid()),
                "--frozen-validate-checkout",
                str(checkout),
                "--completed-record",
                record.relative_to(project).as_posix(),
                "--repository",
                repository.relative_to(project).as_posix(),
                "--format",
                "json",
            ]
        )

        assert rc == 0
        assert attempts == [f"fchmod={errno.EPERM},openat={errno.EACCES}"]
        assert holder is not None and holder.poll() is None
        report = json.loads(capsys.readouterr().out)
        assert report["retained"] == []
        assert report["removed"] == [{"checkout": str(checkout)}]
    finally:
        if holder is not None:
            terminate_process(holder)


@pytest.mark.ordinary_environment
def test_frozen_validate_refuses_when_verified_root_context_is_unavailable(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout, record, _target_sha = prepare_frozen_validation_checkout(
        project, repository, monkeypatch=monkeypatch, name="no-root-exclusion"
    )
    stub_validate_batch_censuses(monkeypatch)
    monkeypatch.setattr(
        wrkslots,
        "_run_root_owned_command",
        lambda *_args, **_kwargs: (1, b"", b"sudo denied\n"),
    )

    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "recover-ownerless-validate-batch",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--frozen-validate-checkout",
            str(checkout),
            "--completed-record",
            record.relative_to(project).as_posix(),
            "--repository",
            repository.relative_to(project).as_posix(),
            "--format",
            "json",
        ]
    )

    assert rc == 0
    report = json.loads(capsys.readouterr().out)
    assert report["removed"] == []
    assert "verified root context could not identify itself" in report["retained"][0][
        "reason"
    ]
    assert checkout.is_dir()
    assert not any(
        item["kind"] == "ownerless-validate-path-removed"
        for item in wrkslots._load_events(
            wrkslots._load_config(str(project), "testhost")
        )
    )


@pytest.mark.ordinary_environment
def test_frozen_validate_refuses_cross_device_exclusion_root(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout, record, _target_sha = prepare_frozen_validation_checkout(
        project, repository, monkeypatch=monkeypatch, name="cross-device-exclusion"
    )
    stub_validate_batch_censuses(monkeypatch)
    monkeypatch.setattr(
        wrkslots,
        "_OWNERLESS_VALIDATION_EXCLUSION_ROOTS",
        (Path("/dev/shm"),),
    )

    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "recover-ownerless-validate-batch",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--frozen-validate-checkout",
            str(checkout),
            "--completed-record",
            record.relative_to(project).as_posix(),
            "--repository",
            repository.relative_to(project).as_posix(),
            "--format",
            "json",
        ]
    )

    assert rc == 0
    report = json.loads(capsys.readouterr().out)
    assert report["removed"] == []
    assert "found 0" in report["retained"][0]["reason"]
    assert checkout.is_dir()


def test_validation_exclusion_mount_alias_comes_from_mount_roots() -> None:
    records = (
        (101, "0:47", Path("/users/alice/home"), Path("/home/alice"), "btrfs", "/dev/md0"),
        (102, "0:47", Path("/"), Path("/data"), "btrfs", "/dev/md0"),
    )

    alias = wrkslots._mount_alias_path(
        Path("/home/alice/work/project/fenced"), Path("/data/misc"), records
    )

    assert alias == Path("/data/users/alice/home/work/project/fenced")


@pytest.mark.parametrize(
    "mutated",
    (
        (
            (101, "0:47", Path("/users/alice/home"), Path("/home/alice"), "btrfs", "/dev/md0"),
            (102, "0:48", Path("/"), Path("/data"), "btrfs", "/dev/other"),
        ),
        (
            (101, "0:47", Path("/users/alice/home"), Path("/home/alice"), "btrfs", "/dev/md0"),
            (102, "0:47", Path("/other"), Path("/data"), "btrfs", "/dev/md0"),
        ),
        (
            (101, "0:47", Path("/users/alice/home"), Path("/home/alice"), "btrfs", "/dev/md0"),
            (103, "0:47", Path("/users/alice/home"), Path("/home/alice"), "btrfs", "/dev/md0"),
            (102, "0:47", Path("/"), Path("/data"), "btrfs", "/dev/md0"),
        ),
    ),
)
def test_validation_exclusion_mount_alias_refuses_mutated_layout(
    mutated: tuple[tuple[int, str, Path, Path, str, str], ...],
) -> None:
    with pytest.raises(wrkslots.Refusal):
        wrkslots._mount_alias_path(
            Path("/home/alice/work/project/fenced"),
            Path("/data/misc"),
            mutated,
        )


def test_validation_exclusion_fixture_is_selected_only_in_process(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    request: pytest.FixtureRequest,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    production_roots = wrkslots._OWNERLESS_VALIDATION_EXCLUSION_ROOTS
    assert production_roots == (Path("/data/misc"),)
    if os.geteuid() != 0 and wrkslots._has_identity_user_namespace():
        unavailable = tmp_path / "missing-data-misc"
        assert not unavailable.exists()
        monkeypatch.setattr(
            wrkslots, "_OWNERLESS_VALIDATION_EXCLUSION_ROOTS", (unavailable,)
        )
    root = install_validation_exclusion_fixture(project, monkeypatch, request)
    fenced = project / "worktrees" / "validate" / "fixture-fenced"
    fenced.parent.mkdir(parents=True, exist_ok=True)

    selected, _identity, alias = wrkslots._trusted_validation_exclusion_root(fenced)

    assert root not in production_roots
    assert selected == root
    assert alias == fenced


@pytest.mark.mapped_root_namespace
def test_validation_exclusion_fixture_preserves_host_root_boundary(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    request: pytest.FixtureRequest,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    production_roots = wrkslots._OWNERLESS_VALIDATION_EXCLUSION_ROOTS
    assert production_roots == (Path("/data/misc"),)
    identity_namespace = wrkslots._has_identity_user_namespace()
    mapped_namespace_root = os.geteuid() == 0 and not identity_namespace
    if mapped_namespace_root:
        host_root = Path("/tmp")
        metadata = host_root.stat(follow_symlinks=False)
        assert metadata.st_uid == 65534
        assert metadata.st_uid != os.geteuid()
        monkeypatch.setattr(
            wrkslots, "_OWNERLESS_VALIDATION_EXCLUSION_ROOTS", (host_root,)
        )
    else:
        assert os.geteuid() != 0
        assert identity_namespace
        unavailable = tmp_path / "missing-data-misc"
        assert not unavailable.exists()
        monkeypatch.setattr(
            wrkslots, "_OWNERLESS_VALIDATION_EXCLUSION_ROOTS", (unavailable,)
        )
    expected_roots = wrkslots._OWNERLESS_VALIDATION_EXCLUSION_ROOTS
    monkeypatch.setenv("WRKSLOTS_VALIDATION_EXCLUSION_ROOT", str(tmp_path))
    assert wrkslots._OWNERLESS_VALIDATION_EXCLUSION_ROOTS == expected_roots

    with pytest.raises(wrkslots.Refusal):
        wrkslots._trusted_validation_exclusion_root(tmp_path / "fixture-fenced")
    budget = wrkslots._ReadOnlyCommandBudget.start(
        timeout_seconds=5,
        stdout_limit=4096,
        stderr_limit=4096,
    )
    if mapped_namespace_root:
        with pytest.raises(
            wrkslots.Refusal,
            match="find is not a root-owned, non-writable executable",
        ):
            wrkslots._run_same_uid_command(
                Path("/usr/bin/find"),
                (str(tmp_path), "-maxdepth", "0"),
                budget=budget,
            )
    else:
        returncode, stdout, stderr = wrkslots._run_same_uid_command(
            Path("/usr/bin/find"),
            (str(tmp_path), "-maxdepth", "0"),
            budget=budget,
        )
        assert returncode == 0
        assert stdout == os.fsencode(str(tmp_path)) + b"\n"
        assert stderr == b""

    parsers = [wrkslots._build_parser()]
    option_strings: set[str] = set()
    while parsers:
        parser = parsers.pop()
        for action in parser._actions:
            option_strings.update(action.option_strings)
            choices = getattr(action, "choices", None)
            if isinstance(choices, dict):
                parsers.extend(choices.values())
    assert "--validation-exclusion-root" not in option_strings

    install_validation_exclusion_fixture(project, monkeypatch, request)
    census = wrkslots._capture_same_uid_process_path_census(
        (tmp_path / "not-in-use",),
        budget=wrkslots._ReadOnlyCommandBudget.start(
            timeout_seconds=5,
            stdout_limit=16 * 1024 * 1024,
            stderr_limit=64 * 1024,
            input_limit=wrkslots._MOUNTINFO_CENSUS_BYTES_LIMIT,
        ),
    )
    assert census.matches == ()
    assert census.owner_cgroup_complete is False
    assert all(
        isinstance(process, wrkslots._AbsentProcessObservation)
        and process.pid > 0
        and process.start_ticks > 0
        and isinstance(process.cgroup_path, str)
        and (
            process.mount_namespace.startswith("mnt:[")
            or process.mount_namespace == f"pid:{process.pid}"
        )
        for process in census.processes
    )
    if mapped_namespace_root:
        assert all(
            process.mount_namespace.startswith("mnt:[")
            for process in census.processes
        )


@pytest.mark.ordinary_environment
def test_frozen_validate_detects_pre_exclusion_same_uid_holder_and_rolls_back(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout, record, _target_sha = prepare_frozen_validation_checkout(
        project, repository, monkeypatch=monkeypatch, name="pre-exclusion-holder"
    )
    stub_validate_batch_censuses(monkeypatch)
    holder: subprocess.Popen[str] | None = None
    guards: list[Path] = []

    def enter_before_exclusion_move(point: str) -> None:
        nonlocal holder
        if (
            point != "after-ownerless-validate-exclusion-directory"
            or holder is not None
        ):
            return
        guards.append(validation_exclusion_guard(project))
        fenced = next(checkout.parent.glob(f".{checkout.name}.ownerless-validate.*"))
        holder = subprocess.Popen(
            [
                sys.executable,
                "-c",
                "import os,sys,time; os.chdir(sys.argv[1]); "
                "print(os.getcwd(), flush=True); time.sleep(60)",
                str(fenced),
            ],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        assert holder.stdout is not None
        assert holder.stdout.readline().strip() == str(fenced)

    def observe_holder(
        paths: Sequence[Path], *, budget: wrkslots._ReadOnlyCommandBudget
    ) -> wrkslots._ProcessPathCensus:
        del budget
        assert holder is not None
        payload = paths[0]
        observed = os.readlink(f"/proc/{holder.pid}/cwd")
        return wrkslots._ProcessPathCensus(
            (), ((holder.pid, str(payload), "link", observed),)
        )

    monkeypatch.setattr(wrkslots, "_interrupt_for_test", enter_before_exclusion_move)
    monkeypatch.setattr(
        wrkslots, "_capture_same_uid_process_path_census", observe_holder
    )
    try:
        rc = wrkslots.main(
            [
                "--project-root",
                str(project),
                "recover-ownerless-validate-batch",
                "--coordinator-authorized",
                "--coordinator-pid",
                str(os.getpid()),
                "--frozen-validate-checkout",
                str(checkout),
                "--completed-record",
                record.relative_to(project).as_posix(),
                "--repository",
                repository.relative_to(project).as_posix(),
                "--format",
                "json",
            ]
        )

        assert rc == 0
        report = json.loads(capsys.readouterr().out)
        assert report["removed"] == []
        assert "live process" in report["retained"][0]["reason"]
        assert checkout.is_dir()
        assert holder is not None
        assert Path(os.readlink(f"/proc/{holder.pid}/cwd")) == checkout
        assert len(guards) == 1 and not guards[0].exists()
        assert not wrkslots._journal_path(
            wrkslots._load_config(str(project), "testhost")
        ).exists()
    finally:
        if holder is not None:
            terminate_process(holder)


@pytest.mark.ordinary_environment
def test_frozen_validate_recovery_refuses_changed_terminal_record_digest(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout, record, _target_sha = prepare_frozen_validation_checkout(
        project, repository, monkeypatch=monkeypatch, name="record-changed"
    )
    interrupted = raw_command_with_census_authority_stub(
        project,
        "recover",
        "--coordinator-authorized",
        "--coordinator-pid",
        str(os.getpid()),
        "--frozen-validate-checkout",
        str(checkout),
        "--completed-record",
        record.relative_to(project).as_posix(),
        "--repository",
        repository.relative_to(project).as_posix(),
        env={"WRKSLOTS_TEST_INTERRUPT": "after-ownerless-validate-journal"},
    )
    assert interrupted.returncode == 86, interrupted.stderr
    value = json.loads(record.read_text(encoding="utf-8"))
    value["detail"] = "changed after authorization"
    record.write_text(json.dumps(value), encoding="utf-8")

    refused = raw_command_with_census_authority_stub(
        project, "recover", "--coordinator-pid", str(os.getpid())
    )

    assert refused.returncode == 3
    assert "record changed after recovery was authorized" in refused.stderr
    assert checkout.is_dir()
    assert (control_directory(project) / "ACTIVE.testhost.journal").is_file()


@pytest.mark.ordinary_environment
def test_frozen_validate_refuses_replacement_after_final_check(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout, record, _target_sha = prepare_frozen_validation_checkout(
        project, repository, monkeypatch=monkeypatch, name="final-replacement"
    )
    stub_validate_batch_censuses(monkeypatch)
    replacement: Path | None = None
    original: Path | None = None

    def replace_after_final_check(point: str) -> None:
        nonlocal replacement, original
        if point != "after-ownerless-validate-final-check" or replacement is not None:
            return
        journal = json.loads(
            wrkslots._journal_path(
                wrkslots._load_config(str(project), "testhost")
            ).read_text(encoding="utf-8")
        )
        original = Path(journal["fenced"])
        checkout.mkdir(mode=0o700)
        (checkout / "replacement").write_text("preserve me\n", encoding="utf-8")
        replacement = checkout

    monkeypatch.setattr(wrkslots, "_interrupt_for_test", replace_after_final_check)
    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "recover-ownerless-validate-batch",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--frozen-validate-checkout",
            str(checkout),
            "--completed-record",
            record.relative_to(project).as_posix(),
            "--repository",
            repository.relative_to(project).as_posix(),
            "--format",
            "json",
        ]
    )

    assert rc == 0
    report = json.loads(capsys.readouterr().out)
    assert report["removed"] == []
    assert "canonical validation path reappeared before cleanup" in report["retained"][0][
        "reason"
    ]
    assert original is not None and (original / ".git").is_dir()
    assert replacement is not None and (replacement / "replacement").is_file()
    config = wrkslots._load_config(str(project), "testhost")
    assert wrkslots._journal_path(config).is_file()
    assert not any(
        item["kind"] == "ownerless-validate-path-removed"
        for item in wrkslots._load_events(config)
    )


@pytest.mark.ordinary_environment
def test_frozen_validate_refuses_fenced_replacement_after_final_check(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout, record, _target_sha = prepare_frozen_validation_checkout(
        project, repository, monkeypatch=monkeypatch, name="fenced-replacement"
    )
    stub_validate_batch_censuses(monkeypatch)
    replacement: Path | None = None
    original: Path | None = None

    def replace_fenced_after_final_check(point: str) -> None:
        nonlocal replacement, original
        if point != "after-ownerless-validate-final-check" or replacement is not None:
            return
        journal = json.loads(
            wrkslots._journal_path(
                wrkslots._load_config(str(project), "testhost")
            ).read_text(encoding="utf-8")
        )
        original = checkout
        fenced = Path(journal["fenced"])
        fenced.mkdir(mode=0o700)
        (fenced / "replacement").write_text("preserve me\n", encoding="utf-8")
        replacement = fenced

    monkeypatch.setattr(
        wrkslots, "_interrupt_for_test", replace_fenced_after_final_check
    )
    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "recover-ownerless-validate-batch",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--frozen-validate-checkout",
            str(checkout),
            "--completed-record",
            record.relative_to(project).as_posix(),
            "--repository",
            repository.relative_to(project).as_posix(),
            "--format",
            "json",
        ]
    )

    assert rc == 0
    report = json.loads(capsys.readouterr().out)
    assert report["removed"] == []
    assert "fenced validation path reappeared before cleanup" in report["retained"][0][
        "reason"
    ]
    assert original is not None and (original / ".git").is_dir()
    assert replacement is not None and (replacement / "replacement").is_file()
    config = wrkslots._load_config(str(project), "testhost")
    assert wrkslots._journal_path(config).is_file()
    assert not any(
        item["kind"] == "ownerless-validate-path-removed"
        for item in wrkslots._load_events(config)
    )


@pytest.mark.ordinary_environment
def test_frozen_validate_refuses_disappearance_after_final_check(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout, record, _target_sha = prepare_frozen_validation_checkout(
        project, repository, monkeypatch=monkeypatch, name="final-disappearance"
    )
    stub_validate_batch_censuses(monkeypatch)
    config = wrkslots._load_config(str(project), "testhost")
    expected_identity = wrkslots._open_directory_identity(checkout, "test checkout")
    preserved: Path | None = None

    def disappear_after_final_check(point: str) -> None:
        nonlocal preserved
        if point != "after-ownerless-validate-final-check" or preserved is not None:
            return
        guard = validation_exclusion_guard(project)
        preserved = guard.with_name(f"{guard.name}.privileged-move")
        wrkslots._root_validation_exclusion_command(
            Path("/usr/bin/mv"),
            ("--no-target-directory", "--", str(guard), str(preserved)),
            "inject a privileged guard disappearance",
        )

    monkeypatch.setattr(wrkslots, "_interrupt_for_test", disappear_after_final_check)
    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "recover-ownerless-validate-batch",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--frozen-validate-checkout",
            str(checkout),
            "--completed-record",
            record.relative_to(project).as_posix(),
            "--repository",
            repository.relative_to(project).as_posix(),
            "--format",
            "json",
        ]
    )

    assert rc == 0
    report = json.loads(capsys.readouterr().out)
    assert report["removed"] == []
    assert "disappeared before cleanup" in report["retained"][0]["reason"]
    assert preserved is not None
    preserved_metadata = preserved.stat(follow_symlinks=False)
    assert preserved_metadata.st_uid == 0
    assert stat.S_IMODE(preserved_metadata.st_mode) == 0
    preserved_payload = (
        preserved / wrkslots._OWNERLESS_VALIDATION_EXCLUSION_ENTRY
    )
    root_payload_metadata = wrkslots._root_validation_path_metadata(
        preserved_payload, "privileged preserved test checkout"
    )
    assert (root_payload_metadata.st_dev, root_payload_metadata.st_ino) == (
        expected_identity[:2]
    )
    journal = wrkslots._journal_path(config)
    assert json.loads(journal.read_text(encoding="utf-8"))["phase"] == "removing"
    assert not any(
        event["kind"] == "ownerless-validate-path-removed"
        for event in wrkslots._load_events(config)
    )

    recorded_guard = validation_exclusion_guard(project)
    wrkslots._root_validation_exclusion_command(
        Path("/usr/bin/mv"),
        ("--no-target-directory", "--", str(preserved), str(recorded_guard)),
        "restore the privileged moved guard",
    )
    resumed = raw_command_with_census_authority_stub(
        project, "recover", "--coordinator-pid", str(os.getpid())
    )

    assert resumed.returncode == 0, resumed.stderr
    assert not journal.exists()
    assert not checkout.exists()
    assert sum(
        event["kind"] == "ownerless-validate-path-removed"
        for event in wrkslots._load_events(config)
    ) == 1


@pytest.mark.ordinary_environment
def test_frozen_validate_recovery_refuses_absent_fenced_path(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout, record, _target_sha = prepare_frozen_validation_checkout(
        project, repository, monkeypatch=monkeypatch, name="fenced-disappearance"
    )
    interrupted = raw_command_with_census_authority_stub(
        project,
        "recover",
        "--coordinator-authorized",
        "--coordinator-pid",
        str(os.getpid()),
        "--frozen-validate-checkout",
        str(checkout),
        "--completed-record",
        record.relative_to(project).as_posix(),
        "--repository",
        repository.relative_to(project).as_posix(),
        env={"WRKSLOTS_TEST_INTERRUPT": "after-ownerless-validate-path-fence"},
    )
    assert interrupted.returncode == 86, interrupted.stderr
    config = wrkslots._load_config(str(project), "testhost")
    journal = wrkslots._journal_path(config)
    journal_value = json.loads(journal.read_text(encoding="utf-8"))
    assert journal_value["phase"] == "fenced"
    fenced = Path(journal_value["fenced"])
    authorization = wrkslots._validation_authorization_from_obj(
        journal_value["authorization"]
    )
    assert authorization.stable_identity is not None
    preserved = fenced.with_name("preserved-fenced-disappearance")
    fenced.rename(preserved)

    resumed = raw_command_with_census_authority_stub(
        project, "recover", "--coordinator-pid", str(os.getpid())
    )

    assert resumed.returncode == 3
    assert "removal outcome is UNKNOWN" in resumed.stderr
    assert json.loads(journal.read_text(encoding="utf-8"))["phase"] == "fenced"
    assert wrkslots._open_directory_identity(
        preserved, "preserved test checkout"
    ) == authorization.identity
    assert wrkslots._private_cleanup_path_identity(
        config,
        preserved,
        allowed_modes={0o700},
        managed_root=wrkslots._frozen_validation_directory(config),
    ) == authorization.stable_identity
    assert not any(
        event["kind"] == "ownerless-validate-path-removed"
        for event in wrkslots._load_events(config)
    )


@pytest.mark.ordinary_environment
def test_frozen_validate_recovery_refuses_absent_prepared_path_twice(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout, record, _target_sha = prepare_frozen_validation_checkout(
        project, repository, monkeypatch=monkeypatch, name="prepared-disappearance"
    )
    stub_validate_batch_censuses(monkeypatch)
    config = wrkslots._load_config(str(project), "testhost")
    expected_identity = wrkslots._open_directory_identity(checkout, "test checkout")
    expected_stable_identity = wrkslots._private_cleanup_path_identity(
        config,
        checkout,
        allowed_modes={0o700},
        managed_root=wrkslots._frozen_validation_directory(config),
    )
    preserved: Path | None = None

    def disappear_before_fenced_journal(point: str) -> None:
        nonlocal preserved
        if (
            point != "after-ownerless-validate-path-fence-before-journal"
            or preserved is not None
        ):
            return
        fenced = next(checkout.parent.glob(f".{checkout.name}.ownerless-validate.*"))
        preserved = checkout.parent / "preserved-prepared-disappearance"
        fenced.rename(preserved)
        raise wrkslots.Refusal("injected disappearance before fenced journal")

    monkeypatch.setattr(
        wrkslots, "_interrupt_for_test", disappear_before_fenced_journal
    )
    rc = run_frozen_validation_batch(project, repository, checkout, record)

    assert rc == 0
    report = json.loads(capsys.readouterr().out)
    assert report["removed"] == []
    assert "injected disappearance before fenced journal" in report["retained"][0][
        "reason"
    ]
    assert preserved is not None and (preserved / ".git").is_dir()
    assert wrkslots._open_directory_identity(
        preserved, "preserved test checkout"
    ) == expected_identity
    assert wrkslots._private_cleanup_path_identity(
        config,
        preserved,
        allowed_modes={0o700},
        managed_root=wrkslots._frozen_validation_directory(config),
    ) == expected_stable_identity
    journal = wrkslots._journal_path(config)
    assert json.loads(journal.read_text(encoding="utf-8"))["phase"] == "prepared"
    assert not any(
        event["kind"] == "ownerless-validate-path-removed"
        for event in wrkslots._load_events(config)
    )

    resumed = raw_command_with_census_authority_stub(
        project, "recover", "--coordinator-pid", str(os.getpid())
    )

    assert resumed.returncode == 3
    assert "removal outcome is UNKNOWN" in resumed.stderr
    assert json.loads(journal.read_text(encoding="utf-8"))["phase"] == "prepared"
    assert wrkslots._open_directory_identity(
        preserved, "preserved test checkout"
    ) == expected_identity
    assert wrkslots._private_cleanup_path_identity(
        config,
        preserved,
        allowed_modes={0o700},
        managed_root=wrkslots._frozen_validation_directory(config),
    ) == expected_stable_identity
    assert not any(
        event["kind"] == "ownerless-validate-path-removed"
        for event in wrkslots._load_events(config)
    )


@pytest.mark.ordinary_environment
@pytest.mark.parametrize("changed_binding", ["file-handle", "parent-mount"])
def test_frozen_validate_recovery_refuses_changed_identity_binding(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    changed_binding: str,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout, record, _target_sha = prepare_frozen_validation_checkout(
        project, repository, monkeypatch=monkeypatch, name=changed_binding
    )
    stub_validate_batch_censuses(monkeypatch)

    class Interrupted(RuntimeError):
        pass

    def interrupt(point: str) -> None:
        if point == "after-ownerless-validate-journal":
            raise Interrupted

    monkeypatch.setattr(wrkslots, "_interrupt_for_test", interrupt)
    with pytest.raises(Interrupted):
        wrkslots.main(
            [
                "--project-root",
                str(project),
                "recover",
                "--coordinator-authorized",
                "--coordinator-pid",
                str(os.getpid()),
                "--frozen-validate-checkout",
                str(checkout),
                "--completed-record",
                record.relative_to(project).as_posix(),
                "--repository",
                repository.relative_to(project).as_posix(),
            ]
        )
    monkeypatch.setattr(wrkslots, "_interrupt_for_test", lambda _point: None)
    capsys.readouterr()

    if changed_binding == "file-handle":
        original_file_handle = wrkslots._fd_file_handle

        def changed_file_handle(fd: int, label: str) -> tuple[int, str]:
            handle_type, handle = original_file_handle(fd, label)
            replacement = ("0" if handle[0] != "0" else "1") + handle[1:]
            return handle_type, replacement

        monkeypatch.setattr(wrkslots, "_fd_file_handle", changed_file_handle)
        expected = "file-handle identity changed"
    else:
        original_open_identity = wrkslots._open_directory_identity

        def changed_parent_mount(path: Path, label: str) -> tuple[int, int, int]:
            identity = original_open_identity(path, label)
            if path == checkout.parent:
                return identity[0], identity[1], identity[2] + 1
            return identity

        monkeypatch.setattr(
            wrkslots, "_open_directory_identity", changed_parent_mount
        )
        expected = "parent identity changed"

    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "recover",
            "--coordinator-pid",
            str(os.getpid()),
        ]
    )

    assert rc == 3
    assert expected in capsys.readouterr().err
    assert checkout.is_dir()
    assert (control_directory(project) / "ACTIVE.testhost.journal").is_file()


@pytest.mark.parametrize("flag", ["-h", "--help"])
@pytest.mark.parametrize("subcommand", ["recover", "recover-ownerless-validate-batch"])
def test_frozen_validate_checkout_flag_is_documented_in_both_help_forms(
    tmp_path: Path, subcommand: str, flag: str
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    helped = raw_command(project, subcommand, flag)

    assert helped.returncode == 0
    assert "--frozen-validate-checkout" in helped.stdout


def test_batched_lsof_census_attributes_live_process_to_exact_checkout(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    first = tmp_path / "validate-fresh-first"
    second = tmp_path / "validate-fresh-second"
    first.mkdir()
    second.mkdir()
    processes = (
        wrkslots._AbsentProcessObservation(4242, 17, "", "mnt:[1]"),
        wrkslots._AbsentProcessObservation(4343, 18, "", "mnt:[2]"),
    )
    calls: list[str] = []

    def catalog(
        targets: Mapping[Path, str], census_budget: wrkslots._ReadOnlyCommandBudget,
    ) -> tuple[dict[tuple[int, int], tuple[tuple[str, str], ...]], bool]:
        assert targets == {first: str(first), second: str(second)}
        assert census_budget is budget
        calls.append("selected-tree-catalog")
        return {}, False

    def snapshot(*, include_owner_cgroups: bool = True) -> tuple[wrkslots._AbsentProcessObservation, ...]:
        assert include_owner_cgroups is False
        calls.append("all-process-snapshot")
        return processes

    def mounts(observed: Sequence[wrkslots._AbsentProcessObservation], targets: Mapping[Path, str], _budget: wrkslots._ReadOnlyCommandBudget) -> tuple[tuple[int, str, str, str], ...]:
        assert tuple(observed) == processes
        assert targets == {first: str(first), second: str(second)}
        calls.append("all-mount-namespaces")
        return ()

    def links(observed: Sequence[wrkslots._AbsentProcessObservation], targets: Mapping[Path, str], _budget: wrkslots._ReadOnlyCommandBudget) -> tuple[tuple[int, str, str, str], ...]:
        assert tuple(observed) == processes
        assert targets == {first: str(first), second: str(second)}
        calls.append("all-process-paths")
        return ((4242, str(second), "link", str(second)),)

    monkeypatch.setattr(wrkslots, "_selected_tree_inodes", catalog)
    monkeypatch.setattr(wrkslots, "_absent_validate_process_snapshot", snapshot)
    monkeypatch.setattr(wrkslots, "_absent_validate_mount_matches", mounts)
    monkeypatch.setattr(wrkslots, "_batch_link_matches", links)
    monkeypatch.setattr(wrkslots, "_batch_inode_matches", lambda *_args: ((), {}))
    monkeypatch.setattr(wrkslots, "_batch_maps_inode_matches", lambda *_args: ())
    monkeypatch.setattr(wrkslots, "_batch_unix_socket_matches", lambda *_args, **_kwargs: ())
    budget = wrkslots._ReadOnlyCommandBudget.start(
        timeout_seconds=10, stdout_limit=1024, stderr_limit=1024, input_limit=1024,
    )
    census = wrkslots._capture_lsof_process_path_census((first, second), budget=budget)

    census.assert_slot_unused(first, None)
    with pytest.raises(wrkslots.Refusal, match="live process 4242"):
        census.assert_slot_unused(second, None)
    # The historical argv assertion excluded a PID filter. Its replacement
    # verifies the complete snapshot reaches both mount and process-path scans.
    assert calls == [
        "selected-tree-catalog", "all-process-snapshot",
        "all-mount-namespaces", "all-process-paths",
    ]
    assert census.processes == processes


@pytest.fixture
def lsof_tmpfs_fixture() -> Iterator[Path]:
    with tempfile.TemporaryDirectory(prefix="wrkslots-lsof-", dir="/dev/shm") as directory:
        yield Path(directory)


@pytest.mark.ordinary_environment
def test_bounded_lsof_reports_installed_binary_file_and_alias_matches(
    tmp_path: Path, lsof_tmpfs_fixture: Path,
) -> None:
    selected = lsof_tmpfs_fixture / "selected"
    outside = lsof_tmpfs_fixture / "outside"
    selected.mkdir()
    outside.mkdir()
    for name in ("open", "mapped", "hardlink", "symlink"):
        (selected / name).write_bytes(b"x" * 4096)
    os.link(selected / "hardlink", outside / "hardlink")
    (outside / "symlink").symlink_to(selected / "symlink")
    holder = textwrap.dedent(r'''
        import ctypes, json, os, socket, sys
        from pathlib import Path
        selected, outside = map(Path, sys.argv[1:])
        os.chdir(selected)
        descriptors = {
            "open": os.open(selected / "open", os.O_RDONLY),
            "hardlink": os.open(outside / "hardlink", os.O_RDONLY),
            "symlink": os.open(outside / "symlink", os.O_RDONLY),
        }
        # Use libc directly so no duplicate descriptor keeps the mapping alive.
        libc = ctypes.CDLL(None, use_errno=True)
        libc.mmap.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int,
                              ctypes.c_int, ctypes.c_int, ctypes.c_long]
        libc.mmap.restype = ctypes.c_void_p
        mapped_fd = os.open(selected / "mapped", os.O_RDONLY)
        address = libc.mmap(None, 4096, 1, 2, mapped_fd, 0)
        assert address != ctypes.c_void_p(-1).value
        os.close(mapped_fd)
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        sock.bind(str(selected / "socket"))
        descriptors["socket"] = sock.fileno()
        print(json.dumps(descriptors), flush=True)
        assert sys.stdin.read(1) == "q"
    ''')
    process = subprocess.Popen(
        [sys.executable, "-c", holder, str(selected), str(outside)],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        text=True, cwd="/",
    )
    try:
        assert process.stdout is not None
        assert select.select([process.stdout], [], [], 5)[0], "holder did not become ready"
        readiness = process.stdout.readline()
        assert readiness, "holder exited before opening the selected files"
        descriptors = json.loads(readiness)
        assert isinstance(descriptors, dict)
        assert all(isinstance(value, int) for value in descriptors.values())
        expected_pid = str(process.pid)
        proc = Path("/proc") / expected_pid
        visible_maps = [line for line in (proc / "maps").read_text().splitlines() if str(selected) in line]
        fixture_evidence = {
            "pid": process.pid,
            "status": [line for line in (proc / "status").read_text().splitlines()
                       if line.startswith(("Uid:", "Gid:", "NSpid:", "TracerPid:", "NoNewPrivs:"))],
            "maps": visible_maps,
            "cwd": os.readlink(proc / "cwd"),
            "fds": {name: os.readlink(proc / "fd" / str(fd)) for name, fd in descriptors.items()},
            "namespaces": {name: os.readlink(proc / "ns" / name) for name in ("user", "pid", "mnt")},
        }
        (tmp_path / "fixture.json").write_text(json.dumps(fixture_evidence, indent=2))
        mapped_rows = [line for line in visible_maps if line.endswith(str(selected / "mapped"))]
        assert len(mapped_rows) == 1
        mapped_fields = mapped_rows[0].split()
        mapped_stat = (selected / "mapped").stat()
        assert tuple(int(part, 16) for part in mapped_fields[3].split(":")) == (
            os.major(mapped_stat.st_dev), os.minor(mapped_stat.st_dev)
        )
        assert int(mapped_fields[4]) == mapped_stat.st_ino
        # Assert the owned holder's population with the historical lsof flags.
        # Production's separate argv assertion retains the full-process census.
        arguments = ["-nP", "-Fpcfn", "-a", "-p", expected_pid]
        arguments.extend(("+D", str(selected)))
        budget = wrkslots._ReadOnlyCommandBudget.start(
            timeout_seconds=22, stdout_limit=16 * 1024 * 1024,
            stderr_limit=64 * 1024,
        )
        rc, stdout, stderr = wrkslots._run_same_uid_command(
            Path("/usr/bin/lsof"), arguments, budget=budget,
        )
        (tmp_path / "lsof.stdout").write_bytes(stdout)
        (tmp_path / "lsof.stderr").write_bytes(stderr)
        (tmp_path / "lsof.returncode").write_text(str(rc))
        assert rc == 0, (stdout.decode(errors="replace"), stderr.decode(errors="replace"))
        assert wrkslots._unrelated_lsof_warnings(
            stderr.decode(errors="surrogateescape"), selected
        )
        rows: set[tuple[str, str, str]] = set()
        pid = descriptor = ""
        for line in stdout.decode(errors="surrogateescape").splitlines():
            if line.startswith("p"):
                pid = line[1:]
            elif line.startswith("f"):
                descriptor = line[1:]
            elif line.startswith("n"):
                rows.add((pid, descriptor, line[1:]))
        assert {pid for pid, _fd, _name in rows} == {expected_pid}
        assert (expected_pid, "cwd", str(selected)) in rows
        assert (expected_pid, "mem", str(selected / "mapped")) in rows
        for name in ("open", "hardlink", "symlink"):
            assert (expected_pid, str(descriptors[name]), str(selected / name)) in rows
        matching = [row for row in rows if row[:2] == (expected_pid, str(descriptors["socket"]))]
        assert len(matching) == 1
        assert matching[0][2].startswith(str(selected / "socket"))
        assert len(rows) == 6
        _stdout, stderr_text = process.communicate("q", timeout=5)
        assert process.returncode == 0, stderr_text
    finally:
        terminate_process(process)


@pytest.mark.ordinary_environment
@pytest.mark.parametrize("kind", ["cwd", "open", "hardlink", "symlink", "mapped", "socket", "socket-at", "socket-space", "socket-abstract", "socket-orphan", "eventfd"])
def test_batch_census_isolated_real_file_alias_mapping_and_socket(
    tmp_path: Path, lsof_tmpfs_fixture: Path, monkeypatch: pytest.MonkeyPatch, kind: str,
) -> None:
    selected = lsof_tmpfs_fixture / "selected"
    outside = lsof_tmpfs_fixture / "outside"
    unrelated = lsof_tmpfs_fixture / "unrelated"
    for path in (selected, outside, unrelated):
        path.mkdir()
    (selected / "file").write_bytes(b"x" * 4096)
    os.link(selected / "file", outside / "alias")
    (outside / "symlink").symlink_to(selected / "file")
    holder = textwrap.dedent(r'''
        import ctypes, json, os, socket, stat, sys
        from pathlib import Path
        selected, outside, kind = Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3]
        if kind == "cwd":
            os.chdir(selected)
        elif kind == "eventfd":
            fd = os.eventfd(0)
            assert stat.S_IFMT(os.fstat(fd).st_mode) == 0
        elif kind in ("socket", "socket-at", "socket-space", "socket-abstract", "socket-orphan"):
            if kind == "socket-orphan":
                libc = ctypes.CDLL(None, use_errno=True)
                original = os.open("/proc/self/ns/net", os.O_RDONLY)
                before = os.readlink("/proc/self/ns/net")
                assert libc.unshare(0x40000000) == 0, ctypes.get_errno()
                bound = os.readlink("/proc/self/ns/net")
            sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            if kind in ("socket-at", "socket-space"):
                os.chdir(selected)
                sock.bind("@socket" if kind == "socket-at" else " ")
                os.chdir("/")
            elif kind == "socket-abstract":
                address = b"\0" + os.fsencode(str(selected / "socket"))
                sock.bind(address)
                assert sock.getsockname() == address
            else:
                sock.bind(str(selected / "socket"))
            if kind == "socket-orphan":
                evidence = dict(before=before, bound=bound, inode=os.fstat(sock.fileno()).st_ino,
                                table=Path("/proc/self/net/unix").read_text())
                assert libc.setns(original, 0x40000000) == 0, ctypes.get_errno()
                os.close(original)
                assert os.readlink("/proc/self/ns/net") == before != bound
                assert libc.prctl(4, 1, 0, 0, 0) == 0
                (outside / "orphan.json").write_text(json.dumps(evidence))
        else:
            path = outside / "alias" if kind in ("hardlink", "mapped") else (
                outside / "symlink" if kind == "symlink" else selected / "file")
            fd = os.open(path, os.O_RDONLY)
            if kind == "mapped":
                libc = ctypes.CDLL(None, use_errno=True)
                libc.mmap.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int,
                                     ctypes.c_int, ctypes.c_int, ctypes.c_long]
                libc.mmap.restype = ctypes.c_void_p
                address = libc.mmap(None, 4096, 1, 2, fd, 0)
                assert address != ctypes.c_void_p(-1).value
                os.close(fd)
        print("ready", flush=True)
        assert sys.stdin.read(1) == "q"
    ''')
    command = [sys.executable, "-B", "-c", holder, str(selected), str(outside), kind]
    if kind == "socket-orphan":
        command = ["/usr/bin/unshare", "--user", "--map-root-user", "--net", "--", *command]
    process = subprocess.Popen(
        command,
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        text=True, cwd="/",
    )
    try:
        assert process.stdout is not None
        assert select.select([process.stdout], [], [], 5)[0], "holder did not become ready"
        assert process.stdout.readline() == "ready\n"
        proc = Path(f"/proc/{process.pid}")
        generation = wrkslots._process_start_ticks(proc)
        namespace = wrkslots._mount_namespace(proc)
        assert generation is not None and namespace is not None
        observation = wrkslots._AbsentProcessObservation(process.pid, generation, "", namespace)
        # Only the fixture population is selected for this semantic control.
        # The actual classifier separately exercises the normal full-host scan.
        monkeypatch.setattr(wrkslots, "_absent_validate_process_snapshot", lambda **_kw: (observation,))
        cwd = os.readlink(proc / "cwd")
        if kind != "cwd":
            assert cwd == "/", "cwd must not mask an omitted file/socket match"
        if kind == "mapped":
            row = next(line for line in (proc / "maps").read_text().splitlines()
                       if line.endswith(str(outside / "alias")))
            fields = row.split()
            identity = (selected / "file").stat()
            evidence = {"device": identity.st_dev, "inode": identity.st_ino, "maps": row}
            map_path = proc / "map_files" / fields[0]
            check_budget = wrkslots._ReadOnlyCommandBudget.start(timeout_seconds=22, stdout_limit=4096, stderr_limit=4096)
            rc, raw, err = wrkslots._run_root_owned_command(
                Path("/usr/bin/find"), ("-L", str(map_path), "-maxdepth", "0", "-printf", "%D\\0%i\\0%y\\0"), budget=check_budget,
            )
            evidence.update(map_files_stdout=raw.decode(), map_files_stderr=err.decode(), map_files_rc=rc)
            (tmp_path / "mapping-identity.json").write_text(json.dumps(evidence))
            assert rc == 0 and not err, evidence
            # The kernel-held mapping must identify exactly the selected file;
            # raw maps device text is retained, never equated across filesystems.
            assert raw == f"{identity.st_dev}\0{identity.st_ino}\0f\0".encode(), evidence
            assert int(fields[4]) == identity.st_ino
        if kind in ("socket-at", "socket-space", "socket-abstract", "socket-orphan"):
            (tmp_path / "unix.raw").write_bytes((proc / "net/unix").read_bytes())
        if kind == "socket-orphan":
            evidence = json.loads((outside / "orphan.json").read_bytes())
            (tmp_path / "orphan.json").write_text(json.dumps(evidence))
            assert (selected / "socket").is_socket()
            for row in (proc / "net/unix").read_text().splitlines()[1:]:
                assert int(row.split(maxsplit=7)[6]) != evidence["inode"]
        if kind in ("socket-at", "socket-space"):
            assert (selected / ("@socket" if kind == "socket-at" else " ")).is_socket()
            reference_budget = wrkslots._ReadOnlyCommandBudget.start(
                timeout_seconds=22, stdout_limit=16 * 1024 * 1024, stderr_limit=64 * 1024,
            )
            rc, out, err = wrkslots._run_same_uid_command(
                Path("/usr/bin/lsof"), ("-nP", "-Fpcfn", "-a", "-p", str(process.pid), "+D", str(selected)), budget=reference_budget,
            )
            (tmp_path / "reference-lsof.stdout").write_bytes(out)
            (tmp_path / "reference-lsof.stderr").write_bytes(err)
            # The historical oracle itself misses this relative filesystem
            # binding. Its exact result is retained; it cannot authorize clear.
            (tmp_path / "reference-lsof.json").write_text(json.dumps({"rc": rc, "stdout": out.decode(), "stderr": err.decode()}))
            assert rc in (0, 1) and wrkslots._unrelated_lsof_warnings(
                err.decode(errors="surrogateescape"), selected
            ), (out, err)
        budget = wrkslots._ReadOnlyCommandBudget.start(
            timeout_seconds=22, stdout_limit=16 * 1024 * 1024,
            stderr_limit=64 * 1024, input_limit=wrkslots._MOUNTINFO_CENSUS_BYTES_LIMIT,
        )
        if kind in ("socket", "socket-at", "socket-space", "socket-orphan"):
            with pytest.raises(wrkslots.Refusal, match="filesystem socket node; socket-object liveness is unproven"):
                held = wrkslots._capture_lsof_process_path_census((selected, unrelated), budget=budget)
                held.assert_slot_unused(selected, None)
            # A selected tree without socket nodes remains a qualifying negative.
            census = wrkslots._capture_lsof_process_path_census((unrelated,), budget=budget)
            census.assert_slot_unused(unrelated, None)
            stdout_text, stderr_text = process.communicate("q", timeout=5)
            assert process.returncode == 0, (stdout_text, stderr_text)
            return
        census = wrkslots._capture_lsof_process_path_census((selected, unrelated), budget=budget)
        (tmp_path / "census.json").write_text(json.dumps({
            "kind": kind, "pid": process.pid, "start": generation, "cwd": cwd,
            "matches": census.matches, "remaining_seconds": budget.remaining_seconds(),
        }, indent=2))
        census.assert_slot_unused(unrelated, None)
        if kind in ("socket-abstract", "eventfd"):
            assert not (selected / "socket").exists()
            census.assert_slot_unused(selected, None)
            stdout_text, stderr_text = process.communicate("q", timeout=5)
            assert process.returncode == 0, (stdout_text, stderr_text)
            return
        with pytest.raises(wrkslots.Refusal, match=f"live process {process.pid}"):
            census.assert_slot_unused(selected, None)
        kinds = {row[2] for row in census.matches if row[0] == process.pid and row[1] == str(selected)}
        if kind == "hardlink":
            assert "inode" in kinds and "link" not in kinds
        elif kind == "mapped":
            assert "map" in kinds and "link" not in kinds and "inode" not in kinds
        stdout_text, stderr_text = process.communicate("q", timeout=5)
        assert process.returncode == 0, (stdout_text, stderr_text)
    finally:
        terminate_process(process)


@pytest.mark.ordinary_environment
def test_batch_census_validation_filesystem_outside_hardlink_mapping(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    # Exercise the kernel-held map_files identity on the normal validation
    # evidence filesystem too; its raw maps device may differ from stat.
    files = tmp_path / "files"
    files.mkdir()
    test_batch_census_isolated_real_file_alias_mapping_and_socket(
        tmp_path, files, monkeypatch, "mapped",
    )


def test_batch_inode_and_maps_census_refuse_incomplete_evidence(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    process = wrkslots._AbsentProcessObservation(4242, 17, "", "mnt:[1]")
    def budget() -> wrkslots._ReadOnlyCommandBudget:
        return wrkslots._ReadOnlyCommandBudget.start(
            timeout_seconds=22, stdout_limit=4096, stderr_limit=4096,
        )
    monkeypatch.setattr(wrkslots, "_process_generation_is_current", lambda _p: True)
    for output, reason in (
        (b"/proc/4242/fd/3\x005\x0019\x00", "truncated record"),
        (b"/proc/9999/fd/3\x005\x0019\x00f\x00", "unexpected PID"),
        (b"/proc/4242/fd/3\x00wrong\x0019\x00f\x00", "invalid inode identity"),
        (b"/proc/4242/fd/3\x005\x0019\x00\x00", "invalid inode identity"),
    ):
        monkeypatch.setattr(wrkslots, "_run_root_owned_command", lambda *_a, data=output, **_k: (0, data, b""))
        with pytest.raises(wrkslots.Refusal, match=reason):
            wrkslots._batch_inode_matches((process,), {}, budget())
    monkeypatch.setattr(wrkslots, "_run_root_owned_command", lambda *_a, **_k: (1, b"", b"/usr/bin/find: '/proc/4242/fd': Permission denied\n"))
    with pytest.raises(wrkslots.Refusal, match="produced diagnostics"):
        wrkslots._batch_inode_matches((process,), {}, budget())
    inodes = {(48, 19): ((str(tmp_path), "selected file"),)}
    def no_fallback(*_args: object, **_kwargs: object) -> tuple[int, bytes, bytes]:
        raise AssertionError("missing descriptor evidence must not use negative lsof")
    monkeypatch.setattr(wrkslots, "_run_same_uid_command", no_fallback)
    for path, error in (("fd", "Permission denied"), ("fd/3", "Transport endpoint is not connected"), ("exe", "Permission denied"), ("cwd", "Permission denied")):
        diagnostic = f"/usr/bin/find: '/proc/4242/{path}': {error}\n".encode()
        monkeypatch.setattr(wrkslots, "_run_root_owned_command", lambda *_a, data=diagnostic, **_k: (1, b"", data))
        with pytest.raises(wrkslots.Refusal, match="unattributed diagnostics"):
            wrkslots._batch_inode_matches((process,), inodes, budget())
    # A disconnected cwd must not discard that same process's independent FD
    # rows: a socket inode is still needed for namespace attribution.
    def disconnected_cwd(_program: Path, arguments: Sequence[str], **_kwargs: object) -> tuple[int, bytes, bytes]:
        if "-mindepth" in arguments:
            return 0, b"/proc/4242/fd/3\x0010\x00123\x00s\x00", b""
        return 1, b"/proc/4242/cwd\x006\x001\x00l\x00", b"/usr/bin/find: '/proc/4242/cwd': Transport endpoint is not connected\n"
    monkeypatch.setattr(wrkslots, "_run_root_owned_command", disconnected_cwd)
    monkeypatch.setattr(wrkslots, "_run_same_uid_command", lambda *_a, **_k: (1, b"", b""))
    matches, sockets = wrkslots._batch_inode_matches((process,), inodes, budget())
    assert not matches and sockets == {123: {4242}}
    # U is a valid anonymous descriptor type, not permission to drop identity.
    monkeypatch.setattr(wrkslots, "_run_root_owned_command", lambda *_a, **_k: (0, b"/proc/4242/fd/4\x0048\x0019\x00U\x00", b""))
    matches, sockets = wrkslots._batch_inode_matches((process,), inodes, budget())
    assert matches and all(row[0] == 4242 and row[2] == "inode" for row in matches)
    assert not sockets
    monkeypatch.setattr(wrkslots, "_run_root_owned_command", lambda *_a, **_k: (0, b"/proc/4242/maps:1-2 r--p 0 broken 19 /outside/file\n", b""))
    with pytest.raises(wrkslots.Refusal, match="invalid identity"):
        wrkslots._batch_maps_inode_matches((process,), {tmp_path: str(tmp_path)}, {}, budget())
    monkeypatch.setattr(wrkslots, "_run_same_uid_command", lambda *_a, **_k: (0, b"48\x0019\x00\x00", b""))
    with pytest.raises(wrkslots.Refusal, match="invalid file type"):
        wrkslots._selected_tree_inodes({tmp_path: str(tmp_path)}, budget())
    rows = {"/proc/4242/map_files/1-2": b"/proc/4242/maps:1-2 r--p 0 00:2f 19 /outside/file"}
    inodes = {(48, 19): ((str(tmp_path), "selected file"),)}
    monkeypatch.setattr(wrkslots, "_run_root_owned_command", lambda *_a, **_k: (1, b"", b"unreadable"))
    with pytest.raises(wrkslots.Refusal, match="map_files inode identity is unreadable"):
        wrkslots._batch_map_file_matches({4242: process}, rows, inodes, budget())
    def changed_mapping(program: Path, *_args: object, **_kwargs: object) -> tuple[int, bytes, bytes]:
        if program.name == "find":
            return 0, b"/proc/4242/map_files/1-2\x0048\x0019\x00f\x00", b""
        return 0, b"/proc/4242/maps:1-2 r--p 0 00:2f 20 /other/file\n", b""
    monkeypatch.setattr(wrkslots, "_run_root_owned_command", changed_mapping)
    with pytest.raises(wrkslots._ProcessEvidenceChanged, match="mappings changed"):
        wrkslots._batch_map_file_matches({4242: process}, rows, inodes, budget())



def test_batch_unix_census_preserves_uncertainty_and_generation_guards(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    process = wrkslots._AbsentProcessObservation(4242, 17, "", "mnt:[1]")
    header = b"Num RefCount Protocol Flags Type St Inode Path\n"
    monkeypatch.setattr(wrkslots, "_batch_network_namespaces", lambda *_args: {4242: "net:[1]"})
    monkeypatch.setattr(wrkslots, "_process_generation_is_current", lambda _p: True)
    monkeypatch.setattr(wrkslots, "_run_same_uid_command", lambda *_a, **_k: (127, b"", b"unavailable"))
    for contents, reason, holders in (
        (b"bad\n", "malformed header", {}),
        (header + b"bad\n", "malformed record", {}),
        (header + b"0: 2 0 0 1 1 123 relative-socket\n", "relative bound pathname", {123: {4242}}),
        (header + b"0: 2 0 0 1 1 123  \n", "relative bound pathname", {123: {4242}}),
        (header, "held socket identities are absent", {123: {4242}}),
        (header + f"0: 2 0 0 1 1 123 {tmp_path}/socket\n".encode(), "without a stable observed holder", {}),
    ):
        monkeypatch.setattr(wrkslots, "_run_root_owned_command", lambda *_a, data=contents, **_k: (0, data, b""))
        with pytest.raises(wrkslots.Refusal, match=reason):
            wrkslots._batch_unix_socket_matches((process,), {tmp_path: str(tmp_path)}, holders, wrkslots._ReadOnlyCommandBudget.start(timeout_seconds=22, stdout_limit=4096, stderr_limit=4096), selected_has_sockets=True)
    monkeypatch.setattr(wrkslots, "_run_root_owned_command", lambda *_a, **_k: (0, header, b""))
    monkeypatch.setattr(wrkslots, "_process_generation_is_current", lambda _p: False)
    with pytest.raises(wrkslots._ProcessEvidenceChanged, match="representatives changed"):
        wrkslots._batch_unix_socket_matches((process,), {tmp_path: str(tmp_path)}, {}, wrkslots._ReadOnlyCommandBudget.start(timeout_seconds=22, stdout_limit=4096, stderr_limit=4096))


@pytest.mark.ordinary_environment
def test_batch_maps_filter_preserves_large_inode_strings_and_read_errors(tmp_path: Path) -> None:
    path = tmp_path / "maps"
    first = "1-2 r--p 0 00:30 9007199254740992 /outside/one"
    second = "2-3 r--p 0 00:30 9007199254740993 /outside/two"
    path.write_text(first + "\n" + second + "\n")
    def budget() -> wrkslots._ReadOnlyCommandBudget:
        return wrkslots._ReadOnlyCommandBudget.start(timeout_seconds=22, stdout_limit=4096, stderr_limit=4096)
    for inode, expected in (("9007199254740992", first), ("9007199254740993", second)):
        rc, out, err = wrkslots._run_root_owned_command(
            Path("/usr/bin/gawk"), ("--", wrkslots._BATCH_MAPS_AWK, str(path)),
            input_data=f"i{inode}\n".encode(), budget=budget(),
        )
        assert rc == 0 and not err, (out, err)
        assert out == f"{path}:{expected}\n".encode()
    rc, out, err = wrkslots._run_root_owned_command(
        Path("/usr/bin/gawk"), ("--", wrkslots._BATCH_MAPS_AWK, str(path / "missing")),
        input_data=b"i9007199254740992\n", budget=budget(),
    )
    assert rc == 1 and not out and str(path / "missing").encode() in err


def test_batch_network_namespace_links_require_complete_generation_bound_records(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    process = wrkslots._AbsentProcessObservation(4242, 17, "", "mnt:[1]")
    monkeypatch.setattr(wrkslots, "_process_generation_is_current", lambda _p: True)
    for rc, out, err, reason in (
        (0, b"", b"", "omitted PID"),
        (0, b"/proc/4242/ns/net\0\0", b"", "unreadable or malformed"),
        (0, b"/proc/9999/ns/net\0net:[1]\0", b"", "unexpected path"),
        (1, b"", b"/usr/bin/find: '/proc/4242/ns/net': Permission denied\n", "produced diagnostics"),
    ):
        monkeypatch.setattr(wrkslots, "_run_root_owned_command", lambda *_a, value=(rc, out, err), **_k: value)
        with pytest.raises(wrkslots.Refusal, match=reason):
            wrkslots._batch_network_namespaces((process,), wrkslots._ReadOnlyCommandBudget.start(timeout_seconds=22, stdout_limit=4096, stderr_limit=4096))


@pytest.mark.ordinary_environment
@pytest.mark.parametrize("kind", ["relative-with-other-fd", "absolute-hardlink", "deleted-original", "rebound-original"])
def test_batch_census_two_selected_trees_cannot_mask_socket_holder(
    tmp_path: Path, lsof_tmpfs_fixture: Path, monkeypatch: pytest.MonkeyPatch, kind: str,
) -> None:
    first, second, unrelated = (lsof_tmpfs_fixture / name for name in ("first", "second", "unrelated"))
    for path in (first, second, unrelated):
        path.mkdir()
    (first / "open-file").write_bytes(b"a separate selected-tree descriptor")
    code = textwrap.dedent(r'''
        import os, socket, sys
        from pathlib import Path
        first, second, kind = Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3]
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        if kind == "relative-with-other-fd":
            held_file = open(first / "open-file", "rb")
            os.chdir(second)
            sock.bind("@socket")
        else:
            sock.bind(str(first / "socket"))
            os.link(first / "socket", second / "hardlink")
            if kind in ("deleted-original", "rebound-original"):
                (first / "socket").unlink()
            if kind == "rebound-original":
                replacement = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                replacement.bind(str(first / "socket"))
        os.chdir("/")
        print("ready", flush=True)
        assert sys.stdin.read(1) == "q"
    ''')
    process = subprocess.Popen(
        [sys.executable, "-B", "-c", code, str(first), str(second), kind],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        text=True, cwd="/",
    )
    try:
        assert process.stdout is not None
        assert select.select([process.stdout], [], [], 5)[0], "holder did not become ready"
        assert process.stdout.readline() == "ready\n"
        proc = Path(f"/proc/{process.pid}")
        generation = wrkslots._process_start_ticks(proc)
        namespace = wrkslots._mount_namespace(proc)
        assert generation is not None and namespace is not None
        assert os.readlink(proc / "cwd") == "/"
        bound = second / ("@socket" if kind == "relative-with-other-fd" else "hardlink")
        assert bound.is_socket()
        if kind == "absolute-hardlink":
            a, b = (first / "socket").stat(), bound.stat()
            assert (a.st_dev, a.st_ino) == (b.st_dev, b.st_ino)
        elif kind == "deleted-original":
            assert not (first / "socket").exists()
        elif kind == "rebound-original":
            assert (first / "socket").is_socket()
            a, b = (first / "socket").stat(), bound.stat()
            assert (a.st_dev, a.st_ino) != (b.st_dev, b.st_ino)
        observation = wrkslots._AbsentProcessObservation(process.pid, generation, "", namespace)
        monkeypatch.setattr(wrkslots, "_absent_validate_process_snapshot", lambda **_kw: (observation,))
        (tmp_path / "unix.raw").write_bytes((proc / "net/unix").read_bytes())
        budget = wrkslots._ReadOnlyCommandBudget.start(
            timeout_seconds=22, stdout_limit=16 * 1024 * 1024, stderr_limit=64 * 1024,
            input_limit=wrkslots._MOUNTINFO_CENSUS_BYTES_LIMIT,
        )
        with pytest.raises(wrkslots.Refusal, match="filesystem socket node; socket-object liveness is unproven") as refused:
            census = wrkslots._capture_lsof_process_path_census((first, second), budget=budget)
            # A's known match cannot make B's actual held socket safe to remove.
            census.assert_slot_unused(second, None)
        (tmp_path / "REFUSAL.txt").write_text(str(refused.value) + "\n")
        clear = wrkslots._capture_lsof_process_path_census((unrelated,), budget=budget)
        clear.assert_slot_unused(unrelated, None)
        stdout_text, stderr_text = process.communicate("q", timeout=5)
        assert process.returncode == 0, (stdout_text, stderr_text)
    finally:
        terminate_process(process)


def test_ownerless_validate_batch_retains_shared_census_use_but_removes_other(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    held, held_record = prepare_legacy_validate_checkout(
        project, repository, name="held"
    )
    unused, unused_record = prepare_legacy_validate_checkout(
        project, repository, name="unused"
    )
    held.chmod(0o700)
    unused.chmod(0o700)
    monkeypatch.setattr(
        wrkslots,
        "_capture_lsof_process_path_census",
        lambda _paths, **_kwargs: wrkslots._ProcessPathCensus(
            (), ((os.getpid(), str(held), "fd", str(held)),), owner_cgroup_complete=False
        ),
    )
    monkeypatch.setattr(
        wrkslots,
        "_capture_same_uid_process_path_census",
        lambda _paths, **_kwargs: wrkslots._ProcessPathCensus(
            (), (), owner_cgroup_complete=False
        ),
    )

    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "recover-ownerless-validate-batch",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--checkout",
            held.relative_to(project).as_posix(),
            "--checkout",
            unused.relative_to(project).as_posix(),
            "--completed-record",
            held_record.relative_to(project).as_posix(),
            "--completed-record",
            unused_record.relative_to(project).as_posix(),
            "--repository",
            repository.relative_to(project).as_posix(),
            "--repository",
            repository.relative_to(project).as_posix(),
            "--format",
            "json",
        ]
    )

    assert rc == 0
    report = json.loads(capsys.readouterr().out)
    assert report["removed"] == [
        {"checkout": unused.relative_to(project).as_posix()}
    ]
    assert len(report["retained"]) == 1
    assert report["retained"][0]["checkout"] == held.relative_to(project).as_posix()
    assert f"live process {os.getpid()}" in report["retained"][0]["reason"]
    assert held.is_dir()
    assert not unused.exists()


def test_ownerless_validate_batch_fresh_census_catches_late_same_uid_use(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout, record = prepare_legacy_validate_checkout(
        project, repository, name="late-use"
    )
    checkout.chmod(0o700)
    monkeypatch.setattr(
        wrkslots,
        "_capture_lsof_process_path_census",
        lambda _paths, **_kwargs: wrkslots._ProcessPathCensus(
            (), (), owner_cgroup_complete=False
        ),
    )
    monkeypatch.setattr(
        wrkslots,
        "_capture_same_uid_process_path_census",
        lambda paths, **_kwargs: wrkslots._ProcessPathCensus(
            (), ((4343, str(paths[0]), "cwd", str(paths[0])),), owner_cgroup_complete=False
        ),
    )

    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "recover-ownerless-validate-batch",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--checkout",
            checkout.relative_to(project).as_posix(),
            "--completed-record",
            record.relative_to(project).as_posix(),
            "--repository",
            repository.relative_to(project).as_posix(),
            "--format",
            "json",
        ]
    )

    assert rc == 0
    report = json.loads(capsys.readouterr().out)
    assert report["removed"] == []
    assert len(report["retained"]) == 1
    assert "live process 4343" in report["retained"][0]["reason"]
    assert report["same_uid_process_censuses"] == 1
    assert checkout.is_dir()


def test_ownerless_validate_batch_refuses_invoking_process_working_directory(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout, record = prepare_legacy_validate_checkout(
        project, repository, name="caller-cwd"
    )
    checkout.chmod(0o700)
    monkeypatch.setattr(
        wrkslots,
        "_capture_lsof_process_path_census",
        lambda _paths, **_kwargs: wrkslots._ProcessPathCensus(
            (), (), owner_cgroup_complete=False
        ),
    )
    monkeypatch.setattr(
        wrkslots,
        "_capture_same_uid_process_path_census",
        lambda _paths, **_kwargs: wrkslots._ProcessPathCensus(
            (), (), owner_cgroup_complete=False
        ),
    )
    previous = Path.cwd()
    try:
        os.chdir(checkout)
        rc = wrkslots.main(
            [
                "--project-root",
                str(project),
                "recover-ownerless-validate-batch",
                "--coordinator-authorized",
                "--coordinator-pid",
                str(os.getpid()),
                "--checkout",
                checkout.relative_to(project).as_posix(),
                "--completed-record",
                record.relative_to(project).as_posix(),
                "--repository",
                repository.relative_to(project).as_posix(),
                "--format",
                "json",
            ]
        )
    finally:
        os.chdir(previous)

    assert rc == 0
    report = json.loads(capsys.readouterr().out)
    assert report["removed"] == []
    assert len(report["retained"]) == 1
    assert "wrkslots process is running from inside slot" in report["retained"][0][
        "reason"
    ]
    assert report["same_uid_process_censuses"] == 0
    assert checkout.is_dir()


def test_recover_refuses_legacy_validate_checkout_without_completed_result(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout_path, record_path = prepare_legacy_validate_checkout(
        project, repository, state="running"
    )

    refused = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--legacy-validate-checkout",
        checkout_path.relative_to(project).as_posix(),
        "--completed-record",
        record_path.relative_to(project).as_posix(),
        "--repository",
        repository.relative_to(project).as_posix(),
    )

    assert refused.returncode == 3
    assert "does not contain an evidenced terminal result" in refused.stderr
    assert "no path was removed" in refused.stderr
    assert "Let validate-run finish recording the result" in refused.stderr
    assert checkout_path.is_dir()
    assert not (control_directory(project) / "ACTIVE.testhost.journal").exists()


def test_recover_resumes_legacy_validate_removal_after_checkout_disappears(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout_path, record_path = prepare_legacy_validate_checkout(project, repository)
    interrupted = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--legacy-validate-checkout",
        checkout_path.relative_to(project).as_posix(),
        "--completed-record",
        record_path.relative_to(project).as_posix(),
        "--repository",
        repository.relative_to(project).as_posix(),
        env={"WRKSLOTS_TEST_INTERRUPT": "after-ownerless-validate-remove"},
    )
    assert interrupted.returncode == 86
    assert not checkout_path.exists()
    journal = control_directory(project) / "ACTIVE.testhost.journal"
    assert journal.is_file()

    recovered = raw_command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
    )

    assert recovered.returncode == 0, recovered.stderr
    assert not journal.exists()
    assert active_slots(project) == []


def test_recover_refuses_literal_preupgrade_legacy_journal_without_proof(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout_path, record_path = prepare_legacy_validate_checkout(project, repository)
    record = json.loads(record_path.read_text())
    record["service_result_schema"] = 1
    record_path.write_text(json.dumps(record), encoding="utf-8")
    config = wrkslots._load_config(str(project), "testhost")
    actor = wrkslots._capture_caller_process(os.getpid(), "test coordinator")
    old_journal = {
        "schema": wrkslots.SCHEMA,
        "kind": "legacy-validate-remove",
        "machine": "testhost",
        "slot": checkout_path.name,
        "phase": "prepared",
        "checkout": checkout_path.relative_to(project).as_posix(),
        "repository": repository.relative_to(project).as_posix(),
        "completed_record": record_path.relative_to(project).as_posix(),
        "completed_record_sha256": hashlib.sha256(record_path.read_bytes()).hexdigest(),
        "head": git(checkout_path, "rev-parse", "HEAD").stdout.strip(),
        "actor": wrkslots._identity_to_obj(actor),
        "coordinator_authorized": False,
    }
    wrkslots._write_journal(config, old_journal)

    refused = raw_command(
        project, "recover", "--coordinator-pid", str(os.getpid())
    )

    assert refused.returncode == 3
    assert "has no bounded validation removal proof" in refused.stderr
    assert checkout_path.is_dir()
    assert checkout_path.absolute() in wrkslots._GitVcs().listed_worktrees(repository)
    assert (control_directory(project) / "ACTIVE.testhost.journal").is_file()


def test_preupgrade_legacy_journal_rechecks_handoff_before_removal(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    checkout_path, record_path = prepare_legacy_validate_checkout(project, repository)
    config = wrkslots._load_config(str(project), "testhost")
    actor = wrkslots._capture_caller_process(os.getpid(), "test coordinator")
    old_journal = {
        "schema": wrkslots.SCHEMA,
        "kind": "legacy-validate-remove",
        "machine": "testhost",
        "slot": checkout_path.name,
        "phase": "prepared",
        "checkout": checkout_path.relative_to(project).as_posix(),
        "repository": repository.relative_to(project).as_posix(),
        "completed_record": record_path.relative_to(project).as_posix(),
        "completed_record_sha256": hashlib.sha256(record_path.read_bytes()).hexdigest(),
        "head": git(checkout_path, "rev-parse", "HEAD").stdout.strip(),
        "actor": wrkslots._identity_to_obj(actor),
        "coordinator_authorized": False,
    }
    wrkslots._write_journal(config, old_journal)
    (checkout_path / "HANDOFF.md").write_text("preserve this\n", encoding="utf-8")

    refused = raw_command(project, "recover", "--coordinator-pid", str(os.getpid()))

    assert refused.returncode == 3
    assert "gained HANDOFF.md" in refused.stderr
    assert checkout_path.is_dir()


def test_recover_ownerless_validate_checkout_requires_explicit_authority(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    target = project / "worktrees" / "validate" / "review-checkout"
    target.parent.mkdir(parents=True)
    git(repository, "worktree", "add", "--detach", str(target), "origin/main")
    record = prepare_terminal_validation_record(project, target, field="checkout")

    refused = raw_command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-checkout",
        target.relative_to(project).as_posix(),
        "--completed-record",
        record.relative_to(project).as_posix(),
        "--repository",
        repository.relative_to(project).as_posix(),
    )

    assert refused.returncode == 3
    assert "coordinator-authorized" in refused.stderr
    assert target.is_dir()
    assert not (control_directory(project) / "ACTIVE.testhost.journal").exists()


def test_recover_ownerless_validate_checkout_removes_clean_terminal_worktree(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    target = project / "worktrees" / "validate" / "review-checkout"
    target.parent.mkdir(parents=True)
    git(repository, "worktree", "add", "--detach", str(target), "origin/main")
    record = prepare_terminal_validation_record(project, target, field="checkout")

    recovered = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-checkout",
        target.relative_to(project).as_posix(),
        "--completed-record",
        record.relative_to(project).as_posix(),
        "--repository",
        repository.relative_to(project).as_posix(),
    )

    assert recovered.returncode == 0, recovered.stderr
    assert not target.exists()
    assert target.absolute() not in wrkslots._GitVcs().listed_worktrees(repository)
    assert active_slots(project) == []


@pytest.mark.parametrize("schema_version", (2, 4))
def test_legacy_validate_checkout_accepts_exact_validation_removal_proof(
    tmp_path: Path, schema_version: int
) -> None:
    project, repository, _remote = make_project(tmp_path)
    commit_validation_removal_schema(repository, schema_version=schema_version)
    checkout_path, record_path = prepare_legacy_validate_checkout(
        project, repository, name="proof"
    )
    manifest, _artifacts = prepare_validation_removal_proof(
        project,
        checkout_path.name,
        checkout_path=checkout_path,
        run_record=record_path,
    )

    recovered = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--legacy-validate-checkout",
        checkout_path.relative_to(project).as_posix(),
        "--completed-record",
        record_path.relative_to(project).as_posix(),
        "--repository",
        repository.relative_to(project).as_posix(),
        "--validation-proof-manifest",
        manifest.relative_to(project).as_posix(),
    )

    assert recovered.returncode == 0, recovered.stderr
    assert not checkout_path.exists()
    assert checkout_path.absolute() not in wrkslots._GitVcs().listed_worktrees(
        repository
    )


def test_ownerless_validation_rechecks_proof_after_private_seal(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    commit_validation_removal_schema(repository)
    target = project / "worktrees" / "validate" / "review-checkout"
    target.parent.mkdir(parents=True)
    git(repository, "worktree", "add", "--detach", str(target), "origin/main")
    record = prepare_terminal_validation_record(project, target, field="checkout")
    manifest, artifacts = prepare_validation_removal_proof(
        project,
        target.name,
        checkout_path=target,
        run_record=record,
    )
    original_recheck = wrkslots._recheck_validation_removal_proof

    def change_result_before_recheck(
        config: wrkslots.Config,
        proof: wrkslots._ValidationRemovalProof,
        *,
        expected_slot: str,
        expected_generation: int | None,
        canonical_slot: Path,
        active_slot: Path,
        canonical_checkout: Path,
        expected_head: str,
        target_kind: str = "checkout",
        repository: Path | None = None,
    ) -> None:
        contents = artifacts["service-result"].read_bytes()
        artifacts["service-result"].write_bytes(b"X" + contents[1:])
        original_recheck(
            config,
            proof,
            expected_slot=expected_slot,
            expected_generation=expected_generation,
            canonical_slot=canonical_slot,
            active_slot=active_slot,
            canonical_checkout=canonical_checkout,
            expected_head=expected_head,
            target_kind=target_kind,
            repository=repository,
        )

    monkeypatch.setattr(
        wrkslots, "_recheck_validation_removal_proof", change_result_before_recheck
    )

    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "recover",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--ownerless-validate-checkout",
            target.relative_to(project).as_posix(),
            "--completed-record",
            record.relative_to(project).as_posix(),
            "--repository",
            repository.relative_to(project).as_posix(),
            "--validation-proof-manifest",
            manifest.relative_to(project).as_posix(),
        ]
    )

    assert rc == 3
    assert "service-result changed bytes" in capsys.readouterr().err
    assert target.is_dir()
    assert stat.S_IMODE(target.stat().st_mode) != 0o700
    assert target.absolute() in wrkslots._GitVcs().listed_worktrees(repository)
    assert not (control_directory(project) / "ACTIVE.testhost.journal").exists()


def test_ownerless_validation_proof_seal_recovers_after_interruption(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    commit_validation_removal_schema(repository)
    target = project / "worktrees" / "validate" / "review-checkout"
    target.parent.mkdir(parents=True)
    git(repository, "worktree", "add", "--detach", str(target), "origin/main")
    record = prepare_terminal_validation_record(project, target, field="checkout")
    manifest, _artifacts = prepare_validation_removal_proof(
        project,
        target.name,
        checkout_path=target,
        run_record=record,
    )
    arguments = (
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-checkout",
        target.relative_to(project).as_posix(),
        "--completed-record",
        record.relative_to(project).as_posix(),
        "--repository",
        repository.relative_to(project).as_posix(),
        "--validation-proof-manifest",
        manifest.relative_to(project).as_posix(),
    )

    interrupted = command(
        project,
        *arguments,
        env={
            "WRKSLOTS_TEST_INTERRUPT": "after-ownerless-validation-proof-seal"
        },
    )
    assert interrupted.returncode == 86
    assert target.is_dir()
    assert stat.S_IMODE(target.stat().st_mode) == 0o700
    assert (control_directory(project) / "ACTIVE.testhost.journal").is_file()

    recovered = raw_command(
        project, "recover", "--coordinator-pid", str(os.getpid())
    )

    assert recovered.returncode == 0, recovered.stderr
    assert not target.exists()
    assert not (control_directory(project) / "ACTIVE.testhost.journal").exists()


def test_ownerless_validation_blocking_status_ignores_only_explicit_safe_roots() -> None:
    status = (
        "! ignored/\n"
        "! ignored/run/output.log\n"
        "! target/\n"
        "! target/debug/program\n"
    )

    assert wrkslots._ownerless_validation_blocking_status(
        status, ("ignored", "target")
    ) == ""


@pytest.mark.parametrize(
    "record",
    (
        "1 tracked-change",
        "2 renamed-tracked-path",
        "u conflicted-tracked-path",
        "? ordinary-untracked-path",
        "? ignored/file",
        "! scratch/",
        "! ignored-other/",
        "! target-other/",
        "malformed-or-unknown-record",
    ),
)
def test_ownerless_validation_blocking_status_preserves_every_other_record(
    record: str,
) -> None:
    assert wrkslots._ownerless_validation_blocking_status(
        f"{record}\n", ("target",)
    ) == record


def test_recover_ownerless_validate_checkout_allows_validation_owned_ignored_and_cache(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
        cache_globs=("target",),
    )
    (repository / ".gitignore").write_text("/ignored/\n/target/\n", encoding="utf-8")
    git(repository, "add", ".gitignore")
    git(repository, "commit", "-m", "ignore validation artifacts")
    git(repository, "push", "origin", "main")
    target = project / "worktrees" / "validate" / "review-checkout"
    target.parent.mkdir(parents=True)
    git(repository, "worktree", "add", "--detach", str(target), "origin/main")
    (target / "ignored" / "run").mkdir(parents=True)
    (target / "ignored" / "run" / "output.log").write_text(
        "disposable\n", encoding="utf-8"
    )
    (target / "target" / "debug").mkdir(parents=True)
    (target / "target" / "debug" / "program").write_text(
        "disposable\n", encoding="utf-8"
    )
    status = wrkslots._GitVcs().status(target, ("target",))
    assert set(status.splitlines()) == {"! ignored/", "! target/"}
    record = prepare_terminal_validation_record(project, target, field="checkout")

    recovered = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-checkout",
        target.relative_to(project).as_posix(),
        "--completed-record",
        record.relative_to(project).as_posix(),
        "--repository",
        repository.relative_to(project).as_posix(),
    )

    assert recovered.returncode == 0, recovered.stderr
    assert not target.exists()


def test_recover_ownerless_validate_checkout_refuses_tracked_change(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
        cache_globs=("target",),
    )
    target = project / "worktrees" / "validate" / "review-checkout"
    target.parent.mkdir(parents=True)
    git(repository, "worktree", "add", "--detach", str(target), "origin/main")
    (target / "seed.txt").write_text("changed\n", encoding="utf-8")

    refused = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-checkout",
        target.relative_to(project).as_posix(),
        "--repository",
        repository.relative_to(project).as_posix(),
        "--recovery-note",
        "no retained run handle",
    )

    assert refused.returncode == 3
    assert "validation checkout is dirty (1 " in refused.stderr
    assert target.is_dir()


def test_recover_ownerless_validate_checkout_refuses_handoff_before_cache_status(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
        cache_globs=("target",),
    )
    target = project / "worktrees" / "validate" / "review-checkout"
    target.parent.mkdir(parents=True)
    git(repository, "worktree", "add", "--detach", str(target), "origin/main")
    (target / "target").mkdir()
    (target / "target" / "cache").write_text("disposable\n", encoding="utf-8")
    (target / "HANDOFF.md").write_text("preserve this\n", encoding="utf-8")

    refused = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-checkout",
        target.relative_to(project).as_posix(),
        "--repository",
        repository.relative_to(project).as_posix(),
        "--recovery-note",
        "no retained run handle",
    )

    assert refused.returncode == 3
    assert "validation checkout contains HANDOFF.md" in refused.stderr
    assert "validation checkout is dirty" not in refused.stderr
    assert target.is_dir()


def test_ownerless_checkout_refuses_remote_change_during_fetch(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    target = project / "worktrees" / "validate" / "review-checkout"
    target.parent.mkdir(parents=True)
    git(repository, "worktree", "add", "--detach", str(target), "origin/main")
    record = prepare_terminal_validation_record(project, target, field="checkout")
    alternate = tmp_path / "alternate.git"
    subprocess.run(
        ["git", "init", "--bare", "--initial-branch=main", str(alternate)],
        check=True,
        capture_output=True,
        text=True,
    )
    original_fetch = wrkslots._GitVcs.fetch_remote

    def mutate_remote_after_fetch(
        vcs: wrkslots._GitVcs,
        checkout: Path,
        remote: str,
        landed_ref: str,
    ) -> None:
        original_fetch(vcs, checkout, remote, landed_ref)
        git(checkout, "remote", "set-url", remote, str(alternate))

    monkeypatch.setattr(wrkslots._GitVcs, "fetch_remote", mutate_remote_after_fetch)

    returncode = wrkslots.main(
        [
            "--project-root",
            str(project),
            "recover",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--ownerless-validate-checkout",
            target.relative_to(project).as_posix(),
            "--completed-record",
            record.relative_to(project).as_posix(),
            "--repository",
            repository.relative_to(project).as_posix(),
        ]
    )

    assert returncode == 3
    assert (
        "validation checkout remote changed during recovery inspection"
        in capsys.readouterr().err
    )
    assert target.is_dir()
    assert target.absolute() in wrkslots._GitVcs().listed_worktrees(repository)
    assert not (control_directory(project) / "ACTIVE.testhost.journal").exists()


def test_ownerless_recovery_accepts_nested_source_without_relaxing_import(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    nested_source = project / "worktrees" / "slots" / "source-checkout"
    nested_source.parent.mkdir(parents=True, exist_ok=True)
    git(repository, "worktree", "add", "--detach", str(nested_source), "origin/main")
    target = project / "worktrees" / "validate" / "review-checkout"
    target.parent.mkdir(parents=True)
    git(nested_source, "worktree", "add", "--detach", str(target), "origin/main")
    record = prepare_terminal_validation_record(project, target, field="checkout")
    config = wrkslots._load_config(str(project), "testhost")
    with pytest.raises(wrkslots.Refusal, match="outside the managed worktrees"):
        wrkslots._repository_path(
            config, nested_source.relative_to(project).as_posix()
        )

    recovered = raw_command(
        project,
        "--allow-existing-unregistered-worktrees",
        "recover",
        "--coordinator-authorized",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-checkout",
        target.relative_to(project).as_posix(),
        "--completed-record",
        record.relative_to(project).as_posix(),
        "--repository",
        nested_source.relative_to(project).as_posix(),
    )

    assert recovered.returncode == 0, recovered.stderr
    assert not target.exists()
    assert nested_source.is_dir()
    assert active_slots(project) == []


def test_recover_ownerless_validate_checkout_refuses_live_or_authored_work(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    target = project / "worktrees" / "validate" / "review-checkout"
    target.parent.mkdir(parents=True)
    git(repository, "worktree", "add", "--detach", str(target), "origin/main")
    (target / "authored.txt").write_text("preserve me\n", encoding="utf-8")

    dirty = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-checkout",
        target.relative_to(project).as_posix(),
        "--repository",
        repository.relative_to(project).as_posix(),
        "--recovery-note",
        "no retained run handle",
    )

    assert dirty.returncode == 3
    assert "validation checkout is dirty" in dirty.stderr
    assert target.is_dir()
    (target / "authored.txt").unlink()

    owner = subprocess.Popen(["sleep", "60"], cwd=target, text=True)
    try:
        live = command(
            project,
            "recover",
            "--coordinator-pid",
            str(os.getpid()),
            "--ownerless-validate-checkout",
            target.relative_to(project).as_posix(),
            "--repository",
            repository.relative_to(project).as_posix(),
            "--recovery-note",
            "no retained run handle",
        )
        assert live.returncode == 3
        assert "uses slot" in live.stderr
        assert target.is_dir()
    finally:
        terminate_process(owner)


def test_ownerless_checkout_rejects_repository_inside_removal_target(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    target = project / "worktrees" / "validate" / "review-checkout"
    target.parent.mkdir(parents=True)
    git(repository, "worktree", "add", "--detach", str(target), "origin/main")
    record = prepare_terminal_validation_record(project, target, field="checkout")

    refused = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-checkout",
        target.relative_to(project).as_posix(),
        "--repository",
        target.relative_to(project).as_posix(),
        "--completed-record",
        record.relative_to(project).as_posix(),
    )

    assert refused.returncode == 3
    assert "repository must survive removal" in refused.stderr
    assert target.is_dir()


def test_ownerless_checkout_preserves_clean_unpublished_commit(tmp_path: Path) -> None:
    project, repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    target = project / "worktrees" / "validate" / "review-checkout"
    target.parent.mkdir(parents=True)
    git(repository, "worktree", "add", "-b", "local-only", str(target), "origin/main")
    (target / "local-only.txt").write_text("preserve me\n", encoding="utf-8")
    git(target, "add", "local-only.txt")
    git(target, "commit", "-m", "local-only fixture")

    refused = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-checkout",
        target.relative_to(project).as_posix(),
        "--repository",
        repository.relative_to(project).as_posix(),
        "--recovery-note",
        "the historical run handle is absent",
    )

    assert refused.returncode == 3
    assert "not contained by remote origin" in refused.stderr
    assert target.is_dir()
    assert git(target, "status", "--porcelain").stdout == ""


def test_recover_ownerless_validate_cargo_home_requires_terminal_or_manual_evidence(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    request: pytest.FixtureRequest,
) -> None:
    project, _repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    target = project / "worktrees" / "validate" / "validate-cargo-old"
    target.mkdir(parents=True)
    (target / "cache-entry").write_text("regenerable\n", encoding="utf-8")

    missing = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-cargo-home",
        target.relative_to(project).as_posix(),
    )
    assert missing.returncode == 3
    assert "--recovery-note" in missing.stderr
    assert target.is_dir()

    record = prepare_terminal_validation_record(project, target, field="cargo_home")
    recovered = in_process_validation_exclusion_command(
        project,
        monkeypatch,
        request,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-cargo-home",
        target.relative_to(project).as_posix(),
        "--completed-record",
        record.relative_to(project).as_posix(),
        add_coordinator_authorization=True,
    )
    assert recovered.args == [
        sys.executable,
        "-m",
        "wrkslots",
        "--project-root",
        str(project),
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-cargo-home",
        target.relative_to(project).as_posix(),
        "--completed-record",
        record.relative_to(project).as_posix(),
        "--coordinator-authorized",
    ]
    assert recovered.returncode == 0, recovered.stderr
    assert not target.exists()
    assert active_slots(project) == []


def test_ownerless_cargo_cleanup_ignores_an_unrelated_missing_slot(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    request: pytest.FixtureRequest,
) -> None:
    project, _repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    made = create(project, slot="unrelated", agent="other", branch="other/task")
    assert made.returncode == 0, made.stderr
    unrelated = checkout(project, "unrelated")
    shutil.rmtree(unrelated)

    target = project / "ignored" / "validate" / "cargo-homes" / "validate-cargo-old"
    target.mkdir(parents=True)
    record = prepare_terminal_validation_record(project, target, field="cargo_home")

    recovered = in_process_validation_exclusion_command(
        project,
        monkeypatch,
        request,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-cargo-home",
        target.relative_to(project).as_posix(),
        "--completed-record",
        record.relative_to(project).as_posix(),
        add_coordinator_authorization=True,
    )

    assert recovered.returncode == 0, recovered.stderr
    assert "unrelated" in recovered.stderr
    assert not target.exists()
    rows = active_slots(project)
    assert len(rows) == 1
    row = wrkslots._as_mapping(rows[0], "remaining active slot")
    assert row["slot"] == "unrelated"


def test_ownerless_cargo_home_allows_nested_git_cache_and_records_determination(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    request: pytest.FixtureRequest,
) -> None:
    project, _repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    target = project / "worktrees" / "validate" / "validate-cargo-nested"
    nested_git = target / "git" / "checkouts" / "dependency" / ".git"
    nested_git.mkdir(parents=True)
    (nested_git / "HEAD").write_text("ref: refs/heads/main\n", encoding="utf-8")
    records = project / "ignored" / "validate" / "runs"
    records.mkdir(parents=True)
    (records / ".historical.json.lock").write_text("not JSON\n", encoding="utf-8")

    recovered = in_process_validation_exclusion_command(
        project,
        monkeypatch,
        request,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-cargo-home",
        target.relative_to(project).as_posix(),
        "--recovery-note",
        "the historical run handle is absent",
        add_coordinator_authorization=True,
    )

    assert recovered.returncode == 0, recovered.stderr
    assert not target.exists()
    config = wrkslots._load_config(str(project), "testhost")
    event = next(
        item
        for item in reversed(wrkslots._load_events(config))
        if item["kind"] == "ownerless-validate-path-removed"
    )
    payload = wrkslots._as_mapping(event["payload"], "test removal event payload")
    authorization = wrkslots._as_mapping(
        payload["authorization"], "test removal authorization"
    )
    evidence = wrkslots._as_mapping(
        authorization["evidence"], "test recovery evidence"
    )
    assert evidence["kind"] == "coordinator-determination"
    assert evidence["note"] == "the historical run handle is absent"
    assert evidence["no_retained_record"] is True
    assert authorization["no_authored_work"] is True
    assert authorization["no_live_use"] is True
    assert authorization["path"] == target.relative_to(project).as_posix()
    target_identity = wrkslots._as_list(
        authorization["identity"], "test target identity"
    )
    actor = wrkslots._as_mapping(authorization["actor"], "test authorizing actor")
    assert len(target_identity) == 3
    assert actor["pid"] == os.getpid()


def test_recordless_ownerless_cleanup_refuses_malformed_retained_record(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    target = project / "worktrees" / "validate" / "validate-cargo-malformed"
    target.mkdir(parents=True)
    records = project / "ignored" / "validate" / "runs"
    records.mkdir(parents=True)
    (records / "unreadable-purpose.json").write_text("{not-json\n", encoding="utf-8")

    refused = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-cargo-home",
        target.relative_to(project).as_posix(),
        "--recovery-note",
        "the historical run handle is absent",
    )

    assert refused.returncode == 3
    assert "cannot prove retained record is unrelated" in refused.stderr
    assert target.is_dir()


def test_manual_ownerless_cleanup_cannot_override_matching_running_record(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    target = project / "worktrees" / "validate" / "validate-cargo-running"
    target.mkdir(parents=True)
    record = prepare_terminal_validation_record(
        project, target, field="cargo_home", state="running"
    )

    refused = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-cargo-home",
        target.relative_to(project).as_posix(),
        "--recovery-note",
        "ignore the retained row",
    )

    assert refused.returncode == 3
    assert str(record) in refused.stderr
    assert "--completed-record" in refused.stderr
    assert target.is_dir()


@pytest.mark.parametrize("state", ["killed", "not-run", "refused"])
def test_ownerless_cleanup_accepts_noncompleted_terminal_record(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    request: pytest.FixtureRequest,
    state: str,
) -> None:
    project, _repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    target = project / "worktrees" / "validate" / f"validate-cargo-{state}"
    target.mkdir(parents=True)
    record = prepare_terminal_validation_record(
        project, target, field="cargo_home", state=state, name=f"validate-{state}"
    )

    recovered = in_process_validation_exclusion_command(
        project,
        monkeypatch,
        request,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-cargo-home",
        target.relative_to(project).as_posix(),
        "--completed-record",
        record.relative_to(project).as_posix(),
        add_coordinator_authorization=True,
    )

    assert recovered.returncode == 0, recovered.stderr
    assert not target.exists()


def test_ownerless_cleanup_refuses_unknown_record(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    target = project / "worktrees" / "validate" / "validate-cargo-unknown"
    target.mkdir(parents=True)
    record = prepare_terminal_validation_record(
        project, target, field="cargo_home", state="unknown"
    )

    refused = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-cargo-home",
        target.relative_to(project).as_posix(),
        "--completed-record",
        record.relative_to(project).as_posix(),
    )

    assert refused.returncode == 3
    assert "does not contain an evidenced terminal result" in refused.stderr
    assert target.is_dir()


@pytest.mark.parametrize("admission_state", ("admitted", "refused"))
def test_ownerless_cleanup_accepts_unknown_record_after_exact_process_exits(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    request: pytest.FixtureRequest,
    admission_state: str,
) -> None:
    project, _repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    target = project / "worktrees" / "validate" / f"validate-cargo-{admission_state}"
    target.mkdir(parents=True)
    record = prepare_terminal_validation_record(
        project,
        target,
        field="cargo_home",
        state="unknown",
        name=f"validate-{admission_state}",
    )
    current = wrkslots._read_process_stat(Path("/proc") / str(os.getpid()))
    assert current is not None
    value = json.loads(record.read_text())
    value.update(
        {
            "kind": "validate",
            "producer": "ci-hub/validate/run_registry.py",
            "admission": "ci-hub validate-lock",
            "temporary_checkout": True,
            "result": "unknown",
            "detail": "the validation result was not recorded",
            "process_identity": {
                "pid": os.getpid(),
                "start_ticks": current.start_ticks + 1,
                "boot_id": wrkslots._boot_id(Path("/proc")),
            },
            "admission_result": (
                {
                    "state": "admitted",
                    "recorded_at": "2026-09-04T12:00:00+00:00",
                    "run_number": 7,
                }
                if admission_state == "admitted"
                else {
                    "state": "refused",
                    "recorded_at": "2026-09-04T12:00:00+00:00",
                    "reason": "stale-base",
                    "exit_code": 3,
                }
            ),
        }
    )
    record.write_text(json.dumps(value), encoding="utf-8")

    recovered = in_process_validation_exclusion_command(
        project,
        monkeypatch,
        request,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-cargo-home",
        target.relative_to(project).as_posix(),
        "--completed-record",
        record.relative_to(project).as_posix(),
        add_coordinator_authorization=True,
    )

    assert recovered.returncode == 0, recovered.stderr
    assert not target.exists()


def test_ownerless_cleanup_retains_unknown_record_while_exact_process_is_live(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    target = project / "worktrees" / "validate" / "validate-cargo-live"
    target.mkdir(parents=True)
    record = prepare_terminal_validation_record(
        project, target, field="cargo_home", state="unknown"
    )
    current = wrkslots._read_process_stat(Path("/proc") / str(os.getpid()))
    assert current is not None
    value = json.loads(record.read_text())
    value.update(
        {
            "kind": "validate",
            "producer": "ci-hub/validate/run_registry.py",
            "admission": "ci-hub validate-lock",
            "temporary_checkout": True,
            "result": "unknown",
            "detail": "the validation result was not recorded",
            "process_identity": {
                "pid": os.getpid(),
                "start_ticks": current.start_ticks,
                "boot_id": wrkslots._boot_id(Path("/proc")),
            },
            "admission_result": {
                "state": "admitted",
                "recorded_at": "2026-09-04T12:00:00+00:00",
                "run_number": 7,
            },
        }
    )
    record.write_text(json.dumps(value), encoding="utf-8")

    refused = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-cargo-home",
        target.relative_to(project).as_posix(),
        "--completed-record",
        record.relative_to(project).as_posix(),
    )

    assert refused.returncode == 3
    assert "does not contain an evidenced terminal result" in refused.stderr
    assert target.is_dir()


@pytest.mark.parametrize(
    ("pid", "start_ticks_delta", "boot_id"),
    (
        (2**31 - 1, 0, None),
        (None, 1, None),
        (None, 0, "different-boot"),
    ),
)
def test_unknown_validation_record_accepts_each_exact_dead_process_proof(
    pid: int | None,
    start_ticks_delta: int,
    boot_id: str | None,
) -> None:
    current = wrkslots._read_process_stat(Path("/proc") / str(os.getpid()))
    assert current is not None
    record = {
        "schema_version": 1,
        "kind": "validate",
        "producer": "ci-hub/validate/run_registry.py",
        "admission": "ci-hub validate-lock",
        "temporary_checkout": True,
        "state": "unknown",
        "result": "unknown",
        "detail": "the validation result was not recorded",
        "process_identity": {
            "pid": pid if pid is not None else os.getpid(),
            "start_ticks": current.start_ticks + start_ticks_delta,
            "boot_id": boot_id or wrkslots._boot_id(Path("/proc")),
        },
        "admission_result": {
            "state": "admitted",
            "recorded_at": "2026-09-04T12:00:00+00:00",
            "run_number": 7,
        },
    }

    assert wrkslots._validation_record_is_terminal(record, target_kind="cargo-home")
    assert not wrkslots._validation_record_is_terminal(
        record, target_kind="checkout"
    )


@pytest.mark.parametrize(
    "admission_result",
    (
        None,
        {"state": "admitted", "recorded_at": "not-a-timestamp"},
        {
            "state": "admitted",
            "recorded_at": "2026-09-04T12:00:00+00:00",
            "run_number": True,
        },
        {
            "state": "refused",
            "recorded_at": "2026-09-04T12:00:00+00:00",
            "reason": "unknown-reason",
            "exit_code": 3,
        },
        {
            "state": "refused",
            "recorded_at": "2026-09-04T12:00:00+00:00",
            "reason": ["stale-base"],
            "exit_code": 3,
        },
    ),
)
def test_unknown_validation_record_rejects_missing_or_malformed_admission(
    admission_result: object,
) -> None:
    record = {
        "schema_version": 1,
        "kind": "validate",
        "producer": "ci-hub/validate/run_registry.py",
        "admission": "ci-hub validate-lock",
        "temporary_checkout": True,
        "state": "unknown",
        "result": "unknown",
        "detail": "the validation result was not recorded",
        "process_identity": {
            "pid": 2**31 - 1,
            "start_ticks": 1,
            "boot_id": wrkslots._boot_id(Path("/proc")),
        },
        "admission_result": admission_result,
    }

    assert not wrkslots._validation_record_is_terminal(
        record, target_kind="cargo-home"
    )


@pytest.mark.parametrize(
    ("field", "value"),
    (
        ("schema_version", True),
        ("kind", ["validate"]),
        ("state", ["unknown"]),
    ),
)
def test_unknown_validation_record_rejects_malformed_discriminator_types(
    field: str, value: object
) -> None:
    record = {
        "schema_version": 1,
        "kind": "validate",
        "producer": "ci-hub/validate/run_registry.py",
        "admission": "ci-hub validate-lock",
        "temporary_checkout": True,
        "state": "unknown",
        "result": "unknown",
        "detail": "the validation result was not recorded",
        "process_identity": {
            "pid": 2**31 - 1,
            "start_ticks": 1,
            "boot_id": wrkslots._boot_id(Path("/proc")),
        },
        "admission_result": {
            "state": "admitted",
            "recorded_at": "2026-09-04T12:00:00+00:00",
            "run_number": 7,
        },
    }
    record[field] = value

    assert not wrkslots._validation_record_is_terminal(
        record, target_kind="cargo-home"
    )


def test_ownerless_cleanup_accepts_current_pass_with_failed_writeback(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    request: pytest.FixtureRequest,
) -> None:
    project, _repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    target = project / "worktrees" / "validate" / "validate-cargo-writeback"
    target.mkdir(parents=True)
    record = prepare_terminal_validation_record(project, target, field="cargo_home")
    value = json.loads(record.read_text())
    value.update(
        service_result_schema=3,
        exit_code=75,
        scorecard_writeback={"status": "failed", "error": "fixture refusal"},
    )
    record.write_text(json.dumps(value), encoding="utf-8")

    recovered = in_process_validation_exclusion_command(
        project,
        monkeypatch,
        request,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-cargo-home",
        target.relative_to(project).as_posix(),
        "--completed-record",
        record.relative_to(project).as_posix(),
        add_coordinator_authorization=True,
    )

    assert recovered.returncode == 0, recovered.stderr
    assert not target.exists()


def current_terminal_validation_record(
    *, schema: int = 4, status: str = "PASSED"
) -> dict[str, object]:
    outcomes = {
        "PASSED": ("success", 0, 7, 7),
        "FAILED": ("failure", 1, 7, 6),
        "COULD_NOT_RUN": ("no-result", 75, None, None),
    }
    result, exit_code, executed_tests, passed_tests = outcomes[status]
    record: dict[str, object] = {
        "state": "completed",
        "result": result,
        "final_validate_status": status,
        "exit_code": exit_code,
        "service_result_schema": schema,
        "result_source": "validation-service-result",
        "selection_mode": "full",
        "executed_nodes": 12,
        "executed_tests": executed_tests,
        "passed_tests": passed_tests,
        "scorecard_writeback": {"status": "completed"},
    }
    if schema == 5:
        record["detail"] = "guest execution failed" if status == "COULD_NOT_RUN" else None
    return record


def test_current_validation_record_requires_authoritative_counts() -> None:
    record = current_terminal_validation_record()
    assert wrkslots._validation_record_is_terminal(record, target_kind="cargo-home")

    mutations: tuple[tuple[str, object], ...] = (
        ("service_result_schema", 4.0),
        ("service_result_schema", True),
        ("service_result_schema", 6),
        ("exit_code", False),
        ("exit_code", 0.0),
        ("passed_tests", None),
        ("passed_tests", 6),
        ("passed_tests", True),
        ("executed_tests", 7.0),
        ("executed_nodes", -1),
        ("executed_nodes", True),
        ("selection_mode", ""),
        ("scorecard_writeback", {"status": "failed", "error": ""}),
        ("detail", None),
        ("result_source", "durable-log"),
        ("result", "failure"),
    )
    for field, value in mutations:
        changed = dict(record)
        changed[field] = value
        assert not wrkslots._validation_record_is_terminal(
            changed, target_kind="cargo-home"
        ), (field, value)

    for field in (
        "selection_mode",
        "result_source",
        "executed_nodes",
        "executed_tests",
        "passed_tests",
        "scorecard_writeback",
    ):
        changed = dict(record)
        del changed[field]
        assert not wrkslots._validation_record_is_terminal(
            changed, target_kind="cargo-home"
        ), field


def test_current_validation_record_preserves_failed_writeback_exit() -> None:
    record = current_terminal_validation_record()
    record["scorecard_writeback"] = {
        "status": "failed",
        "error": "could not publish scorecard",
    }
    record["exit_code"] = 75
    assert wrkslots._validation_record_is_terminal(record, target_kind="cargo-home")

    record["exit_code"] = 0
    assert not wrkslots._validation_record_is_terminal(
        record, target_kind="cargo-home"
    )

    failed = current_terminal_validation_record(status="FAILED")
    for exit_code in (True, 1.0):
        failed["exit_code"] = exit_code
        assert not wrkslots._validation_record_is_terminal(
            failed, target_kind="cargo-home"
        ), exit_code


def test_schema_five_terminal_record_requires_status_appropriate_detail() -> None:
    record = current_terminal_validation_record(schema=5, status="COULD_NOT_RUN")
    assert wrkslots._validation_record_is_terminal(record, target_kind="cargo-home")

    for detail in (None, "", [], ["guest execution failed"]):
        changed = dict(record)
        changed["detail"] = detail
        assert not wrkslots._validation_record_is_terminal(
            changed, target_kind="cargo-home"
        ), detail
    missing = dict(record)
    del missing["detail"]
    assert not wrkslots._validation_record_is_terminal(
        missing, target_kind="cargo-home"
    )

    passed = current_terminal_validation_record(schema=5)
    assert wrkslots._validation_record_is_terminal(passed, target_kind="cargo-home")
    passed["detail"] = "unexpected"
    assert not wrkslots._validation_record_is_terminal(
        passed, target_kind="cargo-home"
    )


@pytest.mark.parametrize(
    ("schema", "status", "missing"),
    (
        (4, "PASSED", "passed_tests"),
        (4, "PASSED", "executed_nodes"),
        (4, "PASSED", "result_source"),
        (5, "COULD_NOT_RUN", "detail"),
    ),
)
def test_ownerless_cleanup_retains_incomplete_current_validation_record(
    tmp_path: Path, schema: int, status: str, missing: str
) -> None:
    project, _repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    target = project / "worktrees" / "validate" / f"validate-cargo-schema-{schema}"
    target.mkdir(parents=True)
    record = prepare_terminal_validation_record(
        project,
        target,
        field="cargo_home",
        name=f"validate-schema-{schema}",
    )
    value = json.loads(record.read_text())
    value.update(current_terminal_validation_record(schema=schema, status=status))
    del value[missing]
    record.write_text(json.dumps(value), encoding="utf-8")

    refused = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-cargo-home",
        target.relative_to(project).as_posix(),
        "--completed-record",
        record.relative_to(project).as_posix(),
    )

    assert refused.returncode == 3
    assert "does not contain an evidenced terminal result" in refused.stderr
    assert target.is_dir()


@pytest.mark.parametrize(
    ("status", "exit_code"), (("PASSED", False), ("FAILED", True))
)
def test_ownerless_cleanup_retains_boolean_current_validation_exit(
    tmp_path: Path, status: str, exit_code: bool
) -> None:
    project, _repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    target = project / "worktrees" / "validate" / f"validate-cargo-{status.lower()}"
    target.mkdir(parents=True)
    record = prepare_terminal_validation_record(
        project,
        target,
        field="cargo_home",
        name=f"validate-{status.lower()}",
    )
    value = json.loads(record.read_text())
    value.update(current_terminal_validation_record(status=status))
    value["exit_code"] = exit_code
    record.write_text(json.dumps(value), encoding="utf-8")

    refused = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-cargo-home",
        target.relative_to(project).as_posix(),
        "--completed-record",
        record.relative_to(project).as_posix(),
    )

    assert refused.returncode == 3
    assert "does not contain an evidenced terminal result" in refused.stderr
    assert target.is_dir()


@pytest.mark.parametrize(
    ("schema_form", "status", "exit_code"),
    (
        ("omitted", "PASSED", 0),
        ("null", "PASSED", 0),
        ("omitted", "PASSED", False),
        ("null", "FAILED", True),
    ),
)
def test_ownerless_cleanup_retains_current_result_without_schema(
    tmp_path: Path, schema_form: str, status: str, exit_code: object
) -> None:
    project, _repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    target = project / "worktrees" / "validate" / "validate-cargo-current"
    target.mkdir(parents=True)
    record = prepare_terminal_validation_record(
        project,
        target,
        field="cargo_home",
        name=f"validate-current-{schema_form}",
    )
    value = json.loads(record.read_text())
    value.update(current_terminal_validation_record(status=status))
    value["exit_code"] = exit_code
    if schema_form == "omitted":
        del value["service_result_schema"]
    else:
        value["service_result_schema"] = None
    assert value["result_source"] == "validation-service-result"
    record.write_text(json.dumps(value), encoding="utf-8")

    refused = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-cargo-home",
        target.relative_to(project).as_posix(),
        "--completed-record",
        record.relative_to(project).as_posix(),
    )

    assert refused.returncode == 3
    assert "does not contain an evidenced terminal result" in refused.stderr
    assert target.is_dir()


def test_ownerless_cleanup_accepts_schema_less_durable_log_record(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    request: pytest.FixtureRequest,
) -> None:
    project, _repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    target = project / "worktrees" / "validate" / "validate-cargo-historical"
    target.mkdir(parents=True)
    record = prepare_terminal_validation_record(
        project,
        target,
        field="cargo_home",
        name="validate-historical-durable-log",
    )
    value = json.loads(record.read_text())
    value.update(
        result="success",
        result_source="durable-log",
        executed_nodes=None,
        executed_tests=None,
        passed_tests=None,
    )
    assert "service_result_schema" not in value
    record.write_text(json.dumps(value), encoding="utf-8")

    recovered = in_process_validation_exclusion_command(
        project,
        monkeypatch,
        request,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-cargo-home",
        target.relative_to(project).as_posix(),
        "--completed-record",
        record.relative_to(project).as_posix(),
        add_coordinator_authorization=True,
    )

    assert recovered.returncode == 0, recovered.stderr
    assert not target.exists()


def test_ownerless_cleanup_rejects_non_authoritative_service_result_schema(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    target = project / "worktrees" / "validate" / "validate-cargo-schema-one"
    target.mkdir(parents=True)
    record = prepare_terminal_validation_record(project, target, field="cargo_home")
    value = json.loads(record.read_text())
    value["service_result_schema"] = 1
    record.write_text(json.dumps(value), encoding="utf-8")

    refused = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-cargo-home",
        target.relative_to(project).as_posix(),
        "--completed-record",
        record.relative_to(project).as_posix(),
    )

    assert refused.returncode == 3
    assert "does not contain an evidenced terminal result" in refused.stderr
    assert target.is_dir()


def test_ownerless_cleanup_refuses_replacement_after_journal_creation(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    target = project / "worktrees" / "validate" / "validate-cargo-replaced"
    target.mkdir(parents=True)
    record = prepare_terminal_validation_record(project, target, field="cargo_home")
    interrupted = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-cargo-home",
        target.relative_to(project).as_posix(),
        "--completed-record",
        record.relative_to(project).as_posix(),
        env={"WRKSLOTS_TEST_INTERRUPT": "after-ownerless-validate-journal"},
    )
    assert interrupted.returncode == 86
    original = target.with_name("validate-cargo-replaced-original")
    target.rename(original)
    target.mkdir()

    refused = raw_command(project, "recover", "--coordinator-pid", str(os.getpid()))

    assert refused.returncode == 3
    assert "identity changed" in refused.stderr
    assert target.is_dir()
    assert original.is_dir()


def test_cache_removal_refuses_replacement_of_fenced_target(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    parent = project / "worktrees" / "validate"
    target = parent / ".validate-cargo-fenced.ownerless-validate.fixture"
    target.mkdir(parents=True)
    target_identity = wrkslots._open_directory_identity(target, "test target")
    parent_identity = wrkslots._open_directory_identity(parent, "test parent")
    original = parent / "preserved-original"
    target.rename(original)
    target.mkdir()
    (target / "replacement").write_text("preserve me\n", encoding="utf-8")

    with pytest.raises(wrkslots.Refusal, match="identity changed before cleanup"):
        wrkslots._remove_cache_directory(
            wrkslots._load_config(str(project), "testhost"),
            wrkslots.CacheDirectory(
                path=target,
                checkout_root=parent,
                checkout_device=parent_identity[0],
                checkout_inode=parent_identity[1],
                checkout_mount_id=parent_identity[2],
            ),
            allow_git_metadata=True,
            expected_identity=target_identity,
        )

    assert target.is_dir()
    assert (target / "replacement").is_file()
    assert original.is_dir()


def test_ownerless_cleanup_resumes_after_fence_before_journal_update(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    request: pytest.FixtureRequest,
) -> None:
    project, _repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    target = project / "worktrees" / "validate" / "validate-cargo-fence-crash"
    target.mkdir(parents=True)
    (target / "cache-entry").write_text("regenerable\n", encoding="utf-8")
    record = prepare_terminal_validation_record(project, target, field="cargo_home")
    interrupted = forked_validation_exclusion_command(
        project,
        monkeypatch,
        request,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-cargo-home",
        target.relative_to(project).as_posix(),
        "--completed-record",
        record.relative_to(project).as_posix(),
        interrupt="after-ownerless-validate-path-fence-before-journal",
        add_coordinator_authorization=True,
    )
    assert interrupted.args == [
        sys.executable,
        "-m",
        "wrkslots",
        "--project-root",
        str(project),
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-cargo-home",
        target.relative_to(project).as_posix(),
        "--completed-record",
        record.relative_to(project).as_posix(),
        "--coordinator-authorized",
    ]
    assert interrupted.returncode == 86
    journal = json.loads(
        (control_directory(project) / "ACTIVE.testhost.journal").read_text()
    )
    fenced = project / journal["fenced"]
    assert not target.exists()
    assert fenced.is_dir()

    resumed = in_process_validation_exclusion_command(
        project,
        monkeypatch,
        request,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        add_coordinator_authorization=False,
    )

    assert resumed.returncode == 0, resumed.stderr
    assert not target.exists()
    assert not fenced.exists()


@pytest.mark.parametrize("target_kind", ["checkout", "cargo-home"])
def test_ownerless_cleanup_rolls_back_when_process_enters_fenced_path(
    tmp_path: Path, target_kind: str
) -> None:
    project, repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    target = project / "worktrees" / "validate" / (
        "validate-fresh-race" if target_kind == "checkout" else "validate-cargo-race"
    )
    target.parent.mkdir(parents=True)
    field = "checkout" if target_kind == "checkout" else "cargo_home"
    args = [
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        (
            "--ownerless-validate-checkout"
            if target_kind == "checkout"
            else "--ownerless-validate-cargo-home"
        ),
        target.relative_to(project).as_posix(),
    ]
    if target_kind == "checkout":
        git(repository, "worktree", "add", "--detach", str(target), "origin/main")
        args.extend(("--repository", repository.relative_to(project).as_posix()))
    else:
        target.mkdir()
    record = prepare_terminal_validation_record(project, target, field=field)
    args.extend(("--completed-record", record.relative_to(project).as_posix()))
    interrupted = command(
        project,
        *args,
        env={"WRKSLOTS_TEST_INTERRUPT": "after-ownerless-validate-path-fence"},
    )
    assert interrupted.returncode == 86
    journal_path = control_directory(project) / "ACTIVE.testhost.journal"
    journal = json.loads(journal_path.read_text())
    fenced = project / journal["fenced"]
    holder = subprocess.Popen(["sleep", "60"], cwd=fenced, text=True)
    try:
        refused = raw_command(
            project, "recover", "--coordinator-pid", str(os.getpid())
        )
        assert refused.returncode == 3
        assert "uses slot" in refused.stderr
        assert target.is_dir()
        assert not fenced.exists()
        assert not journal_path.exists()
    finally:
        terminate_process(holder)


def test_ownerless_validate_recovery_resumes_without_reauthorizing(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    request: pytest.FixtureRequest,
) -> None:
    project, _repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    target = project / "worktrees" / "validate" / "validate-cargo-old"
    target.mkdir(parents=True)
    (target / "cache-entry").write_text("regenerable\n", encoding="utf-8")
    record = prepare_terminal_validation_record(project, target, field="cargo_home")

    interrupted = forked_validation_exclusion_command(
        project,
        monkeypatch,
        request,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-cargo-home",
        target.relative_to(project).as_posix(),
        "--completed-record",
        record.relative_to(project).as_posix(),
        interrupt="after-ownerless-validate-remove",
        add_coordinator_authorization=True,
    )
    assert interrupted.returncode == 86
    assert not target.exists()
    journal = control_directory(project) / "ACTIVE.testhost.journal"
    assert journal.is_file()
    # The durable authorization already captured the record digest before the
    # path was deleted.  Bookkeeping recovery must not become impossible if a
    # later retention pass removes that external record.
    record.unlink()

    resumed = in_process_validation_exclusion_command(
        project,
        monkeypatch,
        request,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        add_coordinator_authorization=False,
    )
    assert resumed.args == [
        sys.executable,
        "-m",
        "wrkslots",
        "--project-root",
        str(project),
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
    ]
    assert resumed.returncode == 0, resumed.stderr
    assert not journal.exists()


def test_ownerless_recovery_keeps_unrelated_drift_visible_in_status(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    request: pytest.FixtureRequest,
) -> None:
    project, _repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    target = project / "worktrees" / "validate" / "validate-cargo-target"
    # The exact same basename under the agent root is unrelated.  The targeted
    # validation-path exception must include slot type rather than silently
    # suppressing both entries by name.
    other = project / "worktrees" / "slots" / "validate-cargo-target"
    target.mkdir(parents=True)
    other.mkdir()

    before = raw_command(project, "status")
    assert before.returncode == 0, before.stderr
    assert "registry_storage=INCONSISTENT" in before.stdout
    assert "target=agent/validate-cargo-target" in before.stdout
    recovered = in_process_validation_exclusion_command(
        project,
        monkeypatch,
        request,
        "--allow-existing-unregistered-worktrees",
        "recover",
        "--coordinator-authorized",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-cargo-home",
        target.relative_to(project).as_posix(),
        "--recovery-note",
        "no retained run handle",
        add_coordinator_authorization=False,
    )
    assert recovered.returncode == 0, recovered.stderr
    assert not target.exists()
    assert other.is_dir()
    after = raw_command(project, "status")
    assert after.returncode == 0, after.stderr
    assert "registry_storage=INCONSISTENT" in after.stdout
    assert "target=agent/validate-cargo-target" in after.stdout
    assert "target=validate/validate-cargo-target" not in after.stdout


def test_create_without_slot_type_names_both_choices_and_changes_nothing(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    events = project / "worktrees" / "EVENTS.testhost"
    before = sorted(path.read_bytes() for path in events.glob("*.json"))

    refused = raw_command(
        project,
        "create",
        "slot01",
        "--coordinator-authorized",
        "--agent",
        "codex-1",
        "--task",
        "task-slot01",
        "--purpose",
        "test slot01",
        "--owner-pid",
        str(os.getpid()),
        "--coordinator-pid",
        str(os.getpid()),
        "--repo",
        "product=repo",
        "--branch",
        "product=codex/task",
    )

    assert refused.returncode == 3
    assert "whether this is an agent or validate slot" in refused.stderr
    assert "--slot-type agent" in refused.stderr
    assert "--slot-type validate" in refused.stderr
    assert "no worktree or lifecycle record was changed" in refused.stderr
    assert refused.stdout == ""
    assert not checkout(project).exists()
    assert active_slots(project) == []
    assert sorted(path.read_bytes() for path in events.glob("*.json")) == before


def test_existing_unregistered_flag_retains_and_warns_without_treating_path_free(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    orphan = slots_directory(project) / "historical"
    orphan.mkdir()

    strict = raw_command(project, "status", "--format", "json")
    assert strict.returncode == 0, strict.stderr
    strict_payload = json.loads(strict.stdout)
    assert strict_payload["active"] == []
    assert strict_payload["registry_storage_state"] == "inconsistent"
    strict_findings = strict_payload["registry_storage_inconsistencies"]
    assert len(strict_findings) == 1
    assert strict_findings[0]["kind"] == "directory-without-row"
    assert strict_findings[0]["slot"] == "historical"
    assert orphan.is_dir()

    audited = raw_command(project, "audit", "--format", "json")
    assert audited.returncode == 0, audited.stderr
    audit_payload = json.loads(audited.stdout)
    assert audit_payload["attention_count"] == 1
    assert audit_payload["attention_slots"] == ["historical"]
    assert audit_payload["summary"] == "1 worktree slot(s) need coordinator attention"

    gated = raw_command(project, "audit", "--gate")
    assert gated.returncode == 1
    assert "state=actionable" in gated.stdout
    assert "summary=1 worktree slot(s) need coordinator attention" in gated.stdout
    assert "summary=1 worktree slot(s) need coordinator attention:" not in gated.stdout
    assert "ACTION:" in gated.stdout
    assert orphan.is_dir()

    reported = raw_command(
        project,
        "--allow-existing-unregistered-worktrees",
        "status",
        "--format",
        "json",
    )
    assert reported.returncode == 0, reported.stderr
    reported_payload = json.loads(reported.stdout)
    assert reported_payload == strict_payload
    assert orphan.is_dir()

    made = raw_command(
        project,
        "--allow-existing-unregistered-worktrees",
        "create",
        "slot01",
        "--slot-type",
        "agent",
        "--coordinator-authorized",
        "--agent",
        "codex-1",
        "--task",
        "task-slot01",
        "--purpose",
        "new registered work beside retained history",
        "--owner-pid",
        str(os.getpid()),
        "--coordinator-pid",
        str(os.getpid()),
        "--repo",
        "product=repo",
        "--branch",
        "product=codex/task",
    )
    assert made.returncode == 0, made.stderr
    assert orphan.is_dir()
    assert len(active_slots(project)) == 1


def test_event_history_is_append_only_and_hash_chained(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)
    config = wrkslots._load_config(str(project), "testhost")
    directory = project / "worktrees" / "EVENTS.testhost"
    original = directory / "00000000000000000001.json"
    original_bytes = original.read_bytes()

    made = create(project)
    assert made.returncode == 0, made.stderr
    renewed = command(
        project,
        "heartbeat",
        "slot01",
        "--agent",
        "codex-1",
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
    )
    assert renewed.returncode == 0, renewed.stderr

    events = wrkslots._load_events(config)
    assert len(events) >= 6
    assert original.read_bytes() == original_bytes
    assert [event["sequence"] for event in events] == list(range(1, len(events) + 1))
    assert events[0]["previous_sha256"] == "0" * 64
    for previous, current in zip(events, events[1:]):
        assert current["previous_sha256"] == previous["sha256"]
    active_events = [
        event for event in events if event["kind"] == "active-state-recorded"
    ]
    assert active_events
    for event in active_events:
        event_payload = wrkslots._as_mapping(
            event["payload"], "test active-state-recorded payload"
        )
        assert "active" not in event_payload
        assert event_payload["slot"] == "slot01"
        event_record = wrkslots._as_mapping(
            event_payload["record"], "test active-state-recorded record"
        )
        assert event_record["slot"] == "slot01"

    changed = json.loads(original.read_text(encoding="utf-8"))
    changed["kind"] = "tampered"
    original.write_text(json.dumps(changed, indent=2) + "\n", encoding="utf-8")
    refused = command(project, "status", "--all-machines", "--format", "json")
    assert refused.returncode == 3
    assert "digest does not match" in refused.stderr
    assert refused.stdout == ""

    create_refused = create(
        project,
        slot="slot02",
        agent="codex-2",
        branch="codex/after-tamper",
    )
    assert create_refused.returncode == 3
    assert "digest does not match" in create_refused.stderr
    assert not checkout(project, "slot02").exists()


def test_status_derives_state_when_compatibility_views_are_missing(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    (project / "worktrees" / "ACTIVE.testhost.json").unlink()
    (project / "worktrees" / "ARCHIVED.testhost.json").unlink()

    status = command(project, "status", "--all-machines", "--format", "json")

    assert status.returncode == 0, status.stderr
    payload = json.loads(status.stdout)
    assert len(payload["active"]) == 1
    assert payload["active"][0]["slot"] == "slot01"


def test_recovery_reconstructs_a_missing_journal_from_event_history(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    interrupted = create(
        project,
        env={"WRKSLOTS_TEST_INTERRUPT": "after-create-worktree"},
    )
    assert interrupted.returncode == 86
    journal = create_journal_path(project)
    assert journal.is_file()
    journal.unlink()

    recovered = raw_command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
    )

    assert recovered.returncode == 0, recovered.stderr
    assert len(active_slots(project)) == 1
    assert checkout(project).is_dir()
    assert not journal.exists()
    events = wrkslots._load_events(wrkslots._load_config(str(project), "testhost"))
    assert events[-1]["kind"] == "operation-completed"


def test_same_slot_race_has_one_winner(tmp_path: Path) -> None:
    project, repository, _remote = make_project(tmp_path)
    base = [
        sys.executable,
        "-m",
        "wrkslots",
        "--project-root",
        str(project),
        "--wait-lock",
        "2",
        "create",
        "slot01",
        "--slot-type",
        "agent",
        "--coordinator-authorized",
        "--task",
        "task-race",
        "--purpose",
        "race test",
        "--owner-pid",
        str(os.getpid()),
        "--coordinator-pid",
        str(os.getpid()),
        "--repo",
        "product=repo",
    ]
    first = subprocess.Popen(
        [*base, "--agent", "codex-1", "--branch", "product=codex/race-one"],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=source_environment(),
    )
    second = subprocess.Popen(
        [*base, "--agent", "codex-2", "--branch", "product=codex/race-two"],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=source_environment(),
    )
    first.communicate(timeout=20)
    second.communicate(timeout=20)

    assert sorted((first.returncode, second.returncode)) == [0, 3]
    assert len(active_slots(project)) == 1
    assert checkout(project).is_dir()


def test_create_preserves_every_unrelated_row_and_directory(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)
    first = create(project)
    assert first.returncode == 0, first.stderr
    before = active_slots(project)[0]
    first_head = git(checkout(project), "rev-parse", "HEAD").stdout.strip()

    second = create(
        project,
        slot="slot02",
        agent="codex-2",
        branch="codex/second",
    )

    assert second.returncode == 0, second.stderr
    rows = active_slots(project)
    assert before in rows
    assert checkout(project).is_dir()
    assert git(checkout(project), "rev-parse", "HEAD").stdout.strip() == first_head
    assert checkout(project, "slot02").is_dir()


@pytest.mark.ordinary_environment
def test_lock_conflict_refuses_without_state_change(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)
    before = (project / "worktrees" / "ACTIVE.testhost.json").read_bytes()
    code = "\n".join(
        (
            "import sys, time",
            "from pathlib import Path",
            f"sys.path.insert(0, {str(PY_ROOT)!r})",
            "from wrkslots import cli as wrkslots",
            f"subject = Path({str(project / 'worktrees' / 'ACTIVE')!r})",
            "with wrkslots._locked(subject, exclusive=True, wait_seconds=0):",
            "    print('locked', flush=True)",
            "    time.sleep(60)",
        )
    )
    holder = subprocess.Popen(
        [sys.executable, "-c", code],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    try:
        assert holder.stdout is not None
        assert holder.stdout.readline().strip() == "locked"
        refused = command(project, "status")
        assert refused.returncode == 3
        assert "state lock is busy" in refused.stderr
        assert "REMEDY:" in refused.stderr
        assert "wrkslots status --help" in refused.stderr
        assert (project / "worktrees" / "ACTIVE.testhost.json").read_bytes() == before
    finally:
        terminate_process(holder)
    repaired = command(project, "status")
    assert repaired.returncode == 0, repaired.stderr


def test_different_machines_use_different_shards(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path, machine="machine-a")
    first = create(
        project,
        slot="slot01",
        agent="codex-1",
        branch="codex/machine-a",
        machine="machine-a",
    )
    second = create(
        project,
        slot="slot02",
        agent="codex-2",
        branch="codex/machine-b",
        machine="machine-b",
    )
    assert first.returncode == 0, first.stderr
    assert second.returncode == 0, second.stderr
    assert len(active_slots(project, "machine-a")) == 1
    assert len(active_slots(project, "machine-b")) == 1
    assert (project / "worktrees" / "ARCHIVED.machine-b.json").is_file()


def test_default_doctor_classifies_storage_against_all_machine_rows(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path, machine="testhost")
    first = create(
        project,
        slot="slot01",
        agent="codex-1",
        branch="codex/machine-a",
        machine="testhost",
    )
    second = create(
        project,
        slot="slot02",
        agent="codex-2",
        branch="codex/machine-b",
        machine="machine-b",
    )
    assert first.returncode == 0, first.stderr
    assert second.returncode == 0, second.stderr

    local = command(project, "doctor", "--format", "json")
    global_view = command(
        project, "doctor", "--all-machines", "--format", "json"
    )

    assert local.returncode == 0, local.stderr
    assert global_view.returncode == 0, global_view.stderr
    local_payload = json.loads(local.stdout)
    global_payload = json.loads(global_view.stdout)
    assert [row["slot"] for row in local_payload["active"]] == ["slot01"]
    assert local_payload["findings"] == []
    assert {row["slot"] for row in global_payload["active"]} == {
        "slot01",
        "slot02",
    }
    assert global_payload["findings"] == []


def test_doctor_replays_hold_events_once_per_machine(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, _repository, _remote = make_project(tmp_path, machine="testhost")
    first = create(
        project,
        slot="slot01",
        agent="codex-1",
        branch="codex/one",
    )
    second = create(
        project,
        slot="slot02",
        agent="codex-2",
        branch="codex/two",
    )
    assert first.returncode == 0, first.stderr
    assert second.returncode == 0, second.stderr
    held_then_released = command(project, "hold", "slot01", "--reason", "inspect")
    assert held_then_released.returncode == 0, held_then_released.stderr
    released = command(project, "unhold", "slot01")
    assert released.returncode == 0, released.stderr
    held = command(project, "hold", "slot02", "--reason", "retain")
    assert held.returncode == 0, held.stderr

    calls: list[tuple[str, frozenset[str]]] = []
    original = wrkslots._holds_from_events

    def counted(
        events: Sequence[Mapping[str, object]],
        machine: str,
        slots: AbstractSet[str],
    ) -> dict[str, dict[str, object]]:
        calls.append((machine, frozenset(slots)))
        return original(events, machine, slots)

    monkeypatch.setattr(wrkslots, "_holds_from_events", counted)
    rc = wrkslots.main(
        ["--project-root", str(project), "doctor", "--format", "json"]
    )

    assert rc == 0
    payload = json.loads(capsys.readouterr().out)
    rows = {row["slot"]: row for row in payload["active"]}
    assert rows["slot01"]["held"] is False
    assert rows["slot01"]["hold_reason"] is None
    assert rows["slot02"]["held"] is True
    assert rows["slot02"]["hold_reason"] == "retain"
    assert calls == [("testhost", frozenset({"slot01", "slot02"}))]


def test_interrupted_create_requires_and_supports_recovery(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)
    interrupted = create(
        project,
        env={"WRKSLOTS_TEST_INTERRUPT": "after-create-worktree"},
    )
    assert interrupted.returncode == 86
    assert checkout(project).is_dir()
    assert not active_slots(project)
    journal = create_journal_path(project)
    assert journal.is_file()
    second = create(
        project,
        slot="slot02",
        agent="codex-2",
        branch="codex/other",
    )
    assert second.returncode == 0, second.stderr
    assert journal.is_file()

    recovered = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--slot",
        "slot01",
    )

    assert recovered.returncode == 0, recovered.stderr
    assert len(active_slots(project)) == 2
    assert not journal.exists()


def test_live_dirty_legacy_create_stays_exact_while_unrelated_creates_succeed(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    interrupted = create(
        project, env={"WRKSLOTS_TEST_INTERRUPT": "after-create-worktree"}
    )
    assert interrupted.returncode == 86
    legacy = control_directory(project) / "ACTIVE.testhost.journal"
    os.replace(create_journal_path(project), legacy)
    shutil.rmtree(control_directory(project) / "EVENTS.testhost")
    dirty = checkout(project) / "unfinished.txt"
    dirty.write_text("authored work\n", encoding="utf-8")
    before = legacy.read_bytes()
    before_stat = legacy.stat(follow_symlinks=False)

    def forbidden(*_args: object, **_kwargs: object) -> object:
        raise AssertionError("read-only classification attempted a mutation")

    monkeypatch.setattr(wrkslots, "_mutation_locks", forbidden)
    for name in (
        "_atomic_write_json",
        "_clear_journal",
        "_remove_control_file",
        "_write_active_state",
        "_write_archive_state",
        "_write_event_file",
        "_write_journal",
    ):
        monkeypatch.setattr(wrkslots, name, forbidden)
    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "classify-create-journals",
            "slot02",
            "--slot-type",
            "agent",
            "--agent",
            "codex-2",
            "--format",
            "json",
        ]
    )
    assert rc == 0
    payload = json.loads(capsys.readouterr().out)
    assert payload["journals"][0]["dirty_checkouts"] == ["product"]
    assert legacy.read_bytes() == before
    assert legacy.stat(follow_symlinks=False).st_ino == before_stat.st_ino
    config = wrkslots._load_config(str(project), "testhost")
    assert wrkslots._journal_cache_slot(config, legacy).slot == "slot01"
    status = command(project, "status", "--all-machines", "--format", "json")
    assert status.returncode == 0, status.stderr
    assert not any(
        item["kind"] == "journal-unreadable"
        for item in json.loads(status.stdout)["registry_storage_inconsistencies"]
    )
    monkeypatch.undo()

    agent_create = create(
        project,
        slot="slot02",
        agent="codex-2",
        branch="codex/two",
        env={"WRKSLOTS_TEST_INTERRUPT": "after-create-worktree"},
    )
    validate_create = create(
        project,
        slot="validate02",
        agent="validate-2",
        branch=None,
        slot_type="validate",
        env={"WRKSLOTS_TEST_INTERRUPT": "after-create-worktree"},
    )
    assert agent_create.returncode == 86, agent_create.stderr
    assert validate_create.returncode == 86, validate_create.stderr
    assert create_journal_path(project, "slot02").is_file()
    assert create_journal_path(project, "validate02").is_file()
    for slot in ("slot02", "validate02"):
        recovered = command(
            project,
            "recover",
            "--coordinator-pid",
            str(os.getpid()),
            "--slot",
            slot,
        )
        assert recovered.returncode == 0, recovered.stderr
    assert legacy.read_bytes() == before
    assert legacy.stat(follow_symlinks=False).st_ino == before_stat.st_ino
    assert dirty.read_text(encoding="utf-8") == "authored work\n"


def test_dead_dirty_create_is_classified_without_mutation(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    interrupted = create(
        project, env={"WRKSLOTS_TEST_INTERRUPT": "after-create-worktree"}
    )
    assert interrupted.returncode == 86
    journal = create_journal_path(project)
    advanced = checkout(project) / "advanced.txt"
    advanced.write_text("new committed work\n", encoding="utf-8")
    git(checkout(project), "add", "advanced.txt")
    git(checkout(project), "commit", "-m", "advance interrupted slot")
    observed_head = git(checkout(project), "rev-parse", "HEAD").stdout.strip()
    dirty = checkout(project) / "unfinished.txt"
    dirty.write_text("authored work\n", encoding="utf-8")

    before = {
        path.relative_to(project).as_posix(): (
            path.read_bytes(),
            path.stat(follow_symlinks=False).st_ino,
            path.stat(follow_symlinks=False).st_mtime_ns,
        )
        for path in project.rglob("*")
        if path.is_file() and not path.is_symlink()
    }

    real_process_state = wrkslots._process_state

    def dead_process_state(
        identity: wrkslots.ProcessIdentity | None,
    ) -> tuple[str, str]:
        if identity is not None and identity.pid == os.getpid():
            return "dead", f"PID {identity.pid} generation exited"
        return real_process_state(identity)

    def forbidden(*_args: object, **_kwargs: object) -> object:
        raise AssertionError("read-only classification attempted a mutation")

    monkeypatch.setattr(wrkslots, "_process_state", dead_process_state)
    monkeypatch.setattr(wrkslots, "_mutation_locks", forbidden)
    for name in (
        "_atomic_write_json",
        "_clear_journal",
        "_remove_control_file",
        "_write_active_state",
        "_write_archive_state",
        "_write_event_file",
        "_write_journal",
    ):
        monkeypatch.setattr(wrkslots, name, forbidden)

    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "classify-create-journals",
            "slot02",
            "--slot-type",
            "validate",
            "--agent",
            "validate-slot02",
            "--format",
            "json",
        ]
    )

    assert rc == 0
    payload = json.loads(capsys.readouterr().out)
    assert payload["schema"] == 2
    assert payload["requested"] == {
        "agent": "validate-slot02",
        "slot": "slot02",
        "slot_path": str(slots_directory(project) / "validate" / "slot02"),
        "slot_type": "validate",
    }
    row = payload["journals"][0]
    assert row["state"] == "dead-incomplete-create"
    assert row["owner_state"] == "dead"
    assert row["coordinator_state"] == "dead"
    assert row["slot_path"] == str(slots_directory(project) / "slot01")
    assert row["dirty_checkouts"] == ["product"]
    assert row["checkouts"] == [
        {
            "branch": "codex/task",
            "dirty": True,
            "head": observed_head,
            "name": "product",
            "path": "worktrees/slot01/product",
        }
    ]

    after = {
        path.relative_to(project).as_posix(): (
            path.read_bytes(),
            path.stat(follow_symlinks=False).st_ino,
            path.stat(follow_symlinks=False).st_mtime_ns,
        )
        for path in project.rglob("*")
        if path.is_file() and not path.is_symlink()
    }
    assert after == before
    assert journal.is_file()
    assert dirty.read_text(encoding="utf-8") == "authored work\n"


def test_indeterminate_create_process_state_refuses_classification(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    interrupted = create(
        project, env={"WRKSLOTS_TEST_INTERRUPT": "after-create-worktree"}
    )
    assert interrupted.returncode == 86
    journal = create_journal_path(project)
    before = (
        journal.read_bytes(),
        journal.stat(follow_symlinks=False).st_ino,
        journal.stat(follow_symlinks=False).st_mtime_ns,
    )

    monkeypatch.setattr(
        wrkslots,
        "_process_state",
        lambda _identity: ("indeterminate", "process table is unreadable"),
    )
    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "classify-create-journals",
            "slot02",
            "--slot-type",
            "validate",
            "--agent",
            "validate-slot02",
            "--format",
            "json",
        ]
    )

    assert rc == 3
    assert "process state is indeterminate" in capsys.readouterr().err
    assert before == (
        journal.read_bytes(),
        journal.stat(follow_symlinks=False).st_ino,
        journal.stat(follow_symlinks=False).st_mtime_ns,
    )


def test_matching_legacy_create_event_is_trusted_by_every_consumer(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    interrupted = create(
        project, env={"WRKSLOTS_TEST_INTERRUPT": "after-create-worktree"}
    )
    assert interrupted.returncode == 86, interrupted.stderr
    legacy = install_legacy_create_journal_with_matching_event(project)

    classified = command(
        project,
        "classify-create-journals",
        "slot02",
        "--slot-type",
        "agent",
        "--agent",
        "codex-2",
    )
    assert classified.returncode == 0, classified.stderr
    assert (
        "journals=1 live-incomplete-create=1 completed-create=0"
        in classified.stdout
    )
    assert "JOURNAL: state=live-incomplete-create" in classified.stdout

    config = wrkslots._load_config(str(project), "testhost")
    assert wrkslots._journal_cache_slot(config, legacy).slot == "slot01"
    status = command(project, "status", "--all-machines", "--format", "json")
    assert status.returncode == 0, status.stderr
    assert not any(
        item["kind"] == "journal-unreadable"
        for item in json.loads(status.stdout)["registry_storage_inconsistencies"]
    )
    audit = command(project, "audit", "--format", "json")
    assert audit.returncode == 0, audit.stderr

    recovered = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--slot",
        "slot01",
    )
    assert recovered.returncode == 0, recovered.stderr
    assert not legacy.exists()
    assert len(active_slots(project)) == 1


def test_changed_legacy_create_journal_refuses_every_consumer(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    interrupted = create(
        project, env={"WRKSLOTS_TEST_INTERRUPT": "after-create-worktree"}
    )
    assert interrupted.returncode == 86, interrupted.stderr
    legacy = install_legacy_create_journal_with_matching_event(project)
    other_create = create(
        project,
        slot="slot02",
        agent="codex-2",
        branch="codex/two",
        env={"WRKSLOTS_TEST_INTERRUPT": "after-create-worktree"},
    )
    assert other_create.returncode == 86, other_create.stderr
    other = create_journal_path(project, "slot02")

    raw: object = json.loads(legacy.read_text(encoding="utf-8"))
    assert isinstance(raw, dict)
    raw["purpose"] = "altered after append-only publication"
    legacy.write_text(json.dumps(raw, indent=2, sort_keys=True) + "\n")
    expected = "standalone recovery journal differs from append-only operation evidence"
    legacy_bytes = legacy.read_bytes()
    legacy_inode = legacy.stat(follow_symlinks=False).st_ino
    other_bytes = other.read_bytes()
    other_inode = other.stat(follow_symlinks=False).st_ino

    classified = command(
        project,
        "classify-create-journals",
        "slot03",
        "--slot-type",
        "agent",
        "--agent",
        "codex-3",
    )
    assert classified.returncode == 3
    assert expected in classified.stderr

    config = wrkslots._load_config(str(project), "testhost")
    with pytest.raises(wrkslots.StateError, match=expected):
        wrkslots._journal_cache_slot(config, legacy)

    status = command(project, "status", "--all-machines", "--format", "json")
    assert status.returncode == 0, status.stderr
    assert any(
        item["kind"] == "journal-unreadable" and expected in item["detail"]
        for item in json.loads(status.stdout)["registry_storage_inconsistencies"]
    )
    audit = command(project, "audit", "--format", "json")
    assert audit.returncode == 3
    assert expected in audit.stderr
    recovered = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--slot",
        "slot01",
    )
    assert recovered.returncode == 3
    assert expected in recovered.stderr
    assert legacy.read_bytes() == legacy_bytes
    assert legacy.stat(follow_symlinks=False).st_ino == legacy_inode
    assert other.read_bytes() == other_bytes
    assert other.stat(follow_symlinks=False).st_ino == other_inode


def test_create_refuses_same_target_and_duplicate_journals_without_overwrite(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    interrupted = create(
        project, env={"WRKSLOTS_TEST_INTERRUPT": "after-create-worktree"}
    )
    assert interrupted.returncode == 86
    journal = create_journal_path(project)
    before = journal.read_bytes()
    same_target = create(
        project, slot="slot01", agent="codex-2", branch="codex/two"
    )
    assert same_target.returncode == 3
    assert "already targets slot" in same_target.stderr
    legacy = control_directory(project) / "ACTIVE.testhost.journal"
    legacy.write_bytes(before)
    duplicate = create(
        project, slot="slot02", agent="codex-2", branch="codex/two"
    )
    assert duplicate.returncode == 3
    assert "multiple interrupted creates claim" in duplicate.stderr
    assert journal.read_bytes() == legacy.read_bytes() == before


def test_scoped_create_journal_names_encode_machine_and_slot_boundaries(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    config = wrkslots._load_config(str(project), "testhost")

    first = wrkslots._create_journal_path(config, "b", "a.1")
    second = wrkslots._create_journal_path(config, "1.b", "a")

    assert first != second
    assert first.name == "CREATE.3.a.1.1.b.journal"
    assert second.name == "CREATE.1.a.3.1.b.journal"
    finish_first = wrkslots._finish_journal_path(config, "b", "a.1")
    finish_second = wrkslots._finish_journal_path(config, "1.b", "a")
    assert finish_first != finish_second
    assert finish_first.name == "FINISH.3.a.1.1.b.journal"
    assert finish_second.name == "FINISH.1.a.3.1.b.journal"


@pytest.mark.parametrize("damage", ("missing", "malformed", "cross-target"))
def test_read_only_create_classification_refuses_untrusted_journal_before_mutation(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    damage: str,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    interrupted = create(
        project, env={"WRKSLOTS_TEST_INTERRUPT": "after-create-worktree"}
    )
    assert interrupted.returncode == 86
    journal = create_journal_path(project)
    if damage == "missing":
        journal.unlink()
    elif damage == "malformed":
        journal.write_text("{broken\n", encoding="utf-8")
    else:
        legacy = control_directory(project) / "ACTIVE.testhost.journal"
        os.replace(journal, legacy)
        journal = legacy
        shutil.rmtree(control_directory(project) / "EVENTS.testhost")
        value = json.loads(journal.read_text(encoding="utf-8"))
        value["planned"][0]["destination"] = "worktrees/slot02/product"
        journal.write_text(json.dumps(value) + "\n", encoding="utf-8")

    def forbidden(*_args: object, **_kwargs: object) -> object:
        raise AssertionError("classification attempted a mutation")

    monkeypatch.setattr(wrkslots, "_mutation_locks", forbidden)
    monkeypatch.setattr(wrkslots, "_write_journal", forbidden)
    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "classify-create-journals",
            "slot02",
            "--slot-type",
            "agent",
            "--agent",
            "codex-2",
        ]
    )
    assert rc == 3
    assert {
        "missing": "missing while append-only history still names it",
        "malformed": "malformed JSON",
        "cross-target": "does not match slot",
    }[damage] in capsys.readouterr().err


def test_two_scoped_create_journals_recover_only_by_exact_slot(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    for slot, agent, branch in (
        ("slot01", "codex-1", "codex/one"),
        ("slot02", "codex-2", "codex/two"),
    ):
        interrupted = create(
            project,
            slot=slot,
            agent=agent,
            branch=branch,
            env={"WRKSLOTS_TEST_INTERRUPT": "after-create-worktree"},
        )
        assert interrupted.returncode == 86, interrupted.stderr
    first = create_journal_path(project, "slot01")
    second = create_journal_path(project, "slot02")
    audit = command(project, "audit", "--format", "json")
    assert audit.returncode == 0, audit.stderr
    audit_rows = json.loads(audit.stdout)["slots"]
    assert {row["slot"] for row in audit_rows} == {"slot01", "slot02"}
    assert {row["owner_state"] for row in audit_rows} == {"create-journal"}
    status = command(project, "status", "--all-machines", "--format", "json")
    assert status.returncode == 0, status.stderr
    assert not any(
        item["kind"] == "journal-unreadable"
        for item in json.loads(status.stdout)["registry_storage_inconsistencies"]
    )
    ambiguous = command(
        project, "recover", "--coordinator-pid", str(os.getpid())
    )
    assert ambiguous.returncode == 3
    assert "select one exact" in ambiguous.stderr
    for slot, selected, retained in (
        ("slot01", first, second),
        ("slot02", second, None),
    ):
        recovered = command(
            project,
            "recover",
            "--coordinator-pid",
            str(os.getpid()),
            "--slot",
            slot,
        )
        assert recovered.returncode == 0, recovered.stderr
        assert not selected.exists()
        if retained is not None:
            assert retained.is_file()
    rows = active_slots(project)
    assert all(isinstance(row, dict) for row in rows)
    assert {row["slot"] for row in rows if isinstance(row, dict)} == {
        "slot01",
        "slot02",
    }


def test_exact_slot_recovery_selects_legacy_create_beside_scoped_create(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    first = create(
        project, env={"WRKSLOTS_TEST_INTERRUPT": "after-create-worktree"}
    )
    assert first.returncode == 86
    legacy = control_directory(project) / "ACTIVE.testhost.journal"
    os.replace(create_journal_path(project), legacy)
    shutil.rmtree(control_directory(project) / "EVENTS.testhost")
    second = create(
        project,
        slot="slot02",
        agent="codex-2",
        branch="codex/two",
        env={"WRKSLOTS_TEST_INTERRUPT": "after-create-worktree"},
    )
    assert second.returncode == 86, second.stderr
    second_journal = create_journal_path(project, "slot02")

    recovered = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--slot",
        "slot01",
    )

    assert recovered.returncode == 0, recovered.stderr
    assert not legacy.exists()
    assert second_journal.is_file()


@pytest.mark.parametrize(
    ("damage", "expected"),
    (
        ("missing-events", "no matching append-only progress evidence"),
        ("tampered-journal", "differs from append-only progress evidence"),
    ),
)
def test_untrusted_scoped_create_journal_refuses_every_reader(
    tmp_path: Path, damage: str, expected: str
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    for slot, agent, branch in (
        ("slot01", "codex-1", "codex/one"),
        ("slot02", "codex-2", "codex/two"),
    ):
        interrupted = create(
            project,
            slot=slot,
            agent=agent,
            branch=branch,
            env={"WRKSLOTS_TEST_INTERRUPT": "after-create-worktree"},
        )
        assert interrupted.returncode == 86, interrupted.stderr
    target = create_journal_path(project, "slot01")
    other = create_journal_path(project, "slot02")
    if damage == "missing-events":
        shutil.rmtree(control_directory(project) / "EVENTS.testhost")
    else:
        raw = json.loads(target.read_text(encoding="utf-8"))
        raw["purpose"] = "tampered after append-only publication"
        target.write_text(json.dumps(raw, indent=2, sort_keys=True) + "\n")
    target_bytes = target.read_bytes()
    target_inode = target.stat(follow_symlinks=False).st_ino
    other_bytes = other.read_bytes()
    other_inode = other.stat(follow_symlinks=False).st_ino

    classified = command(
        project,
        "classify-create-journals",
        "slot03",
        "--slot-type",
        "agent",
        "--agent",
        "codex-3",
    )
    assert classified.returncode == 3
    assert expected in classified.stderr

    config = wrkslots._load_config(str(project), "testhost")
    with pytest.raises((wrkslots.Refusal, wrkslots.StateError), match=expected):
        wrkslots._journal_cache_slot(config, target)

    status = command(project, "status", "--all-machines", "--format", "json")
    assert status.returncode == 0, status.stderr
    findings = json.loads(status.stdout)["registry_storage_inconsistencies"]
    assert any(
        item["kind"] == "journal-unreadable" and expected in item["detail"]
        for item in findings
    )

    audit = command(project, "audit", "--format", "json")
    assert audit.returncode == 3
    assert expected in audit.stderr
    recovered = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--slot",
        "slot01",
    )
    assert recovered.returncode == 3
    assert expected in recovered.stderr
    assert target.read_bytes() == target_bytes
    assert target.stat(follow_symlinks=False).st_ino == target_inode
    assert other.read_bytes() == other_bytes
    assert other.stat(follow_symlinks=False).st_ino == other_inode


def test_completed_scoped_create_journal_requires_exact_active_row(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    interrupted = create(
        project,
        env={"WRKSLOTS_TEST_INTERRUPT": "after-operation-completed"},
    )
    assert interrupted.returncode == 86, interrupted.stderr
    journal = create_journal_path(project)
    assert journal.is_file()
    assert len(active_slots(project)) == 1

    classified = command(
        project,
        "classify-create-journals",
        "slot02",
        "--slot-type",
        "agent",
        "--agent",
        "codex-2",
        "--format",
        "json",
    )
    assert classified.returncode == 0, classified.stderr
    report = json.loads(classified.stdout)
    assert report["journals"][0]["state"] == "completed-create"
    human = command(
        project,
        "classify-create-journals",
        "slot02",
        "--slot-type",
        "agent",
        "--agent",
        "codex-2",
    )
    assert human.returncode == 0, human.stderr
    assert (
        "journals=1 live-incomplete-create=0 completed-create=1" in human.stdout
    )
    assert "JOURNAL: state=completed-create" in human.stdout

    config = wrkslots._load_config(str(project), "testhost")
    cache_slot = wrkslots._journal_cache_slot(config, journal)
    assert cache_slot.slot == "slot01"
    status = command(project, "status", "--all-machines", "--format", "json")
    assert status.returncode == 0, status.stderr
    assert not any(
        item["kind"] == "journal-unreadable"
        for item in json.loads(status.stdout)["registry_storage_inconsistencies"]
    )
    audit = command(project, "audit", "--format", "json")
    assert audit.returncode == 0, audit.stderr

    raw = json.loads(journal.read_text(encoding="utf-8"))
    states = wrkslots._load_all_active(config, require_repository=False)
    altered = [
        replace(
            state,
            slots=tuple(
                replace(record, purpose="different")
                if record.slot == "slot01"
                else record
                for record in state.slots
            ),
        )
        for state in states
    ]
    with pytest.raises(
        wrkslots.StateError,
        match="no exact durable ACTIVE row",
    ):
        wrkslots._scoped_create_journal_completed(
            config, journal, raw, altered
        )

    recovered = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--slot",
        "slot01",
    )
    assert recovered.returncode == 0, recovered.stderr
    assert not journal.exists()
    assert len(active_slots(project)) == 1


def test_destructive_recover_still_refuses_nonancestor_coordinator(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    interrupted = create(
        project, env={"WRKSLOTS_TEST_INTERRUPT": "after-create-worktree"}
    )
    assert interrupted.returncode == 86
    journal = create_journal_path(project)
    before = journal.read_bytes()
    sibling = subprocess.Popen(["sleep", "60"], text=True)
    try:
        refused = command(
            project,
            "recover",
            "--coordinator-pid",
            str(sibling.pid),
            "--slot",
            "slot01",
        )
    finally:
        terminate_process(sibling)
    assert refused.returncode == 3
    assert "not in the invoking process ancestry" in refused.stderr
    assert journal.read_bytes() == before


def test_create_refuses_owner_generation_change_before_publication(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    original_read = wrkslots._read_process_identity

    def changed_after_checkout(pid: int) -> wrkslots.ProcessIdentity:
        identity = original_read(pid)
        if pid == os.getpid() and checkout(project).exists():
            return replace(identity, start_ticks=identity.start_ticks + 1)
        return identity

    monkeypatch.setattr(wrkslots, "_read_process_identity", changed_after_checkout)
    returncode = wrkslots.main(
        [
            "--project-root",
            str(project),
            "create",
            "slot01",
            "--slot-type",
            "agent",
            "--coordinator-authorized",
            "--agent",
            "codex-1",
            "--task",
            "task-slot01",
            "--purpose",
            "owner generation test",
            "--owner-pid",
            str(os.getpid()),
            "--coordinator-pid",
            str(os.getpid()),
            "--repo",
            "product=repo",
            "--branch",
            "product=codex/task",
        ]
    )
    captured = capsys.readouterr()

    assert returncode == 3
    assert "owner process generation changed during create" in captured.err
    assert checkout(project).is_dir()
    assert active_slots(project) == []
    assert create_journal_path(project).is_file()


def test_interrupted_finish_recovers_archive_and_removal(tmp_path: Path) -> None:
    project, repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    commit_task(repository, checkout(project), "codex/task")
    handed_off = finish(project)
    assert handed_off.returncode == 0, handed_off.stderr
    mark_owner_dead(project)
    set_liveness(project, "dead")
    interrupted = command(
        project,
        "remove",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        env={"WRKSLOTS_TEST_INTERRUPT": "after-remove-worktree"},
    )
    assert interrupted.returncode == 86
    assert not checkout(project).exists()
    assert len(active_slots(project)) == 1
    assert (project / "worktrees" / "ACTIVE.testhost.journal").is_file()

    recovered = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
    )

    assert recovered.returncode == 0, recovered.stderr
    assert active_slots(project) == []
    assert not (project / "worktrees" / "slot01").exists()
    archive = json.loads(
        (project / "worktrees" / "ARCHIVED.testhost.json").read_text(encoding="utf-8")
    )
    assert len(archive["records"]) == 1


def test_crash_after_git_removal_before_journal_update_recovers(tmp_path: Path) -> None:
    project, repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    commit_task(repository, checkout(project), "codex/task")
    handed_off = finish(project)
    assert handed_off.returncode == 0, handed_off.stderr
    mark_owner_dead(project)
    set_liveness(project, "dead")
    interrupted = command(
        project,
        "remove",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        env={"WRKSLOTS_TEST_INTERRUPT": "after-remove-before-journal"},
    )
    assert interrupted.returncode == 86
    assert not checkout(project).exists()
    journal_path = project / "worktrees" / "ACTIVE.testhost.journal"
    journal = json.loads(journal_path.read_text(encoding="utf-8"))
    assert journal["removed"] == []

    recovered = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
    )

    assert recovered.returncode == 0, recovered.stderr
    assert active_slots(project) == []
    assert not (project / "worktrees" / "slot01").exists()


@pytest.mark.ordinary_environment
def test_process_entering_after_final_scan_before_path_move_is_not_deleted(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    commit_task(repository, checkout(project), "codex/task")
    handed_off = finish(project)
    assert handed_off.returncode == 0, handed_off.stderr
    mark_owner_dead(project)
    set_liveness(project, "dead")
    original_assert = wrkslots._assert_slot_unused
    calls = 0
    entrant: subprocess.Popen[str] | None = None

    def enter_after_scan(
        slot_path: Path,
        record: wrkslots.ActiveRecord | None = None,
        *,
        use_lsof: bool = True,
        ignore_invoking_ancestry: bool = False,
        census: wrkslots._ProcessPathCensus | None = None,
        fallback_census: wrkslots._ProcessPathCensus | None = None,
    ) -> None:
        nonlocal calls, entrant
        original_assert(
            slot_path,
            record,
            use_lsof=use_lsof,
            ignore_invoking_ancestry=ignore_invoking_ancestry,
            census=census,
            fallback_census=fallback_census,
        )
        calls += 1
        if calls == 2:
            entrant = subprocess.Popen(
                [
                    sys.executable,
                    "-c",
                    "import os,sys,time; os.chdir(sys.argv[1]); "
                    "print(os.getcwd(), flush=True); time.sleep(60)",
                    str(checkout(project)),
                ],
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            assert entrant.stdout is not None
            assert entrant.stdout.readline().strip() == str(checkout(project))

    monkeypatch.setattr(wrkslots, "_assert_slot_unused", enter_after_scan)
    try:
        returncode = wrkslots.main(
            [
                "--project-root",
                str(project),
                "remove",
                "slot01",
                "--coordinator-authorized",
                "--coordinator-pid",
                str(os.getpid()),
                "--expected-generation",
                "1",
            ]
        )

        assert returncode == 3
        assert "live process" in capsys.readouterr().err
        assert entrant is not None
        assert checkout(project).is_dir()
        assert Path(os.readlink(f"/proc/{entrant.pid}/cwd")) == checkout(project)
        assert not list((project / "worktrees").glob(".slot01.fenced.*"))
        assert not (project / "worktrees" / "ACTIVE.testhost.journal").exists()
        assert len(active_slots(project)) == 1
    finally:
        if entrant is not None:
            terminate_process(entrant)


@pytest.mark.parametrize(
    ("layout", "worktrees_directory"),
    ((None, "worktrees"), ("flat", "worktrees/slots")),
)
def test_crash_after_path_fence_recovers_from_fenced_storage(
    tmp_path: Path, layout: str | None, worktrees_directory: str
) -> None:
    project, repository, _remote = make_project(
        tmp_path,
        layout=layout,
        worktrees_directory=worktrees_directory,
    )
    made = create(project)
    assert made.returncode == 0, made.stderr
    commit_task(repository, checkout(project), "codex/task")
    handed_off = finish(project)
    assert handed_off.returncode == 0, handed_off.stderr
    mark_owner_dead(project)
    set_liveness(project, "dead")

    interrupted = command(
        project,
        "remove",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        env={"WRKSLOTS_TEST_INTERRUPT": "after-path-fence-before-journal"},
    )

    assert interrupted.returncode == 86
    assert not checkout(project).exists()
    assert len(list(slots_directory(project).glob(".slot01.fenced.*"))) == 1
    recovered = command(project, "recover", "--coordinator-pid", str(os.getpid()))
    assert recovered.returncode == 0, recovered.stderr
    assert active_slots(project) == []
    assert not list(slots_directory(project).glob(".slot01.fenced.*"))


@pytest.mark.parametrize(
    ("layout", "worktrees_directory"),
    ((None, "worktrees"), ("flat", "worktrees/slots")),
)
def test_recovery_rolls_back_when_handoff_changes_after_path_fence(
    tmp_path: Path, layout: str | None, worktrees_directory: str
) -> None:
    project, _repository, _remote = make_project(
        tmp_path,
        layout=layout,
        worktrees_directory=worktrees_directory,
    )
    made = create(project)
    assert made.returncode == 0, made.stderr
    tree = checkout(project)
    handoff = tree / "HANDOFF.md" if layout == "flat" else tree.parent / "HANDOFF.md"
    handoff.write_text("read before reclaim\n", encoding="utf-8")
    read = raw_command(
        project,
        "read-handoff",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
    )
    assert read.returncode == 0, read.stderr
    mark_owner_dead(project)
    set_liveness(project, "dead")
    expire_heartbeat(project)

    interrupted = command(
        project,
        "remove",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        env={"WRKSLOTS_TEST_INTERRUPT": "after-path-fence-before-journal"},
    )

    assert interrupted.returncode == 86
    fenced = list(slots_directory(project).glob(".slot01.fenced.*"))
    assert len(fenced) == 1
    fenced_handoff = fenced[0] / "HANDOFF.md"
    fenced_handoff.write_text("changed while reclaim was interrupted\n", encoding="utf-8")

    refused = raw_command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
    )

    assert refused.returncode == 3
    assert "contains an unread HANDOFF.md" in refused.stderr
    assert "path-fence rollback failed" not in refused.stderr
    assert checkout(project).is_dir()
    assert not list(slots_directory(project).glob(".slot01.fenced.*"))
    assert not (control_directory(project) / "ACTIVE.testhost.journal").exists()

    reread = raw_command(
        project,
        "read-handoff",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
    )
    assert reread.returncode == 3
    assert "sidecar disagrees with the legacy file" in reread.stderr
    assert checkout(project).is_dir()
    config = wrkslots._load_config(str(project), "testhost")
    sidecar = wrkslots._handoff_sidecar_path(config, "slot01", 1)
    assert sidecar.is_file()
    assert handoff.is_file()

    adopted = raw_command(
        project,
        "read-handoff",
        "slot01",
        "--coordinator-authorized",
        "--coordinator-pid",
        str(os.getpid()),
        "--adopt-legacy-change",
        "--expected-generation",
        "1",
    )
    assert adopted.returncode == 3
    assert "sidecars are immutable" in adopted.stderr
    assert handoff.is_file()
    assert checkout(project).is_dir()
    artifact = wrkslots._load_handoff_sidecar(
        config, wrkslots._load_active(config).slots[0]
    )
    assert artifact is not None
    assert artifact.contents == b"read before reclaim\n"
    pending = wrkslots._retirement_candidates(
        config, wrkslots._load_active(config), wrkslots._load_events(config)
    )
    assert len(pending) == 1
    assert pending[0].blocked_reason is not None
    assert "sidecar disagrees with the legacy file" in pending[0].blocked_reason


def test_crash_after_archive_before_active_delete_recovers_exact_overlap(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    commit_task(repository, checkout(project), "codex/task")
    handed_off = finish(project)
    assert handed_off.returncode == 0, handed_off.stderr
    mark_owner_dead(project)
    set_liveness(project, "dead")

    interrupted = command(
        project,
        "remove",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        env={"WRKSLOTS_TEST_INTERRUPT": "after-archive-before-active"},
    )

    assert interrupted.returncode == 86
    assert len(active_slots(project)) == 1
    archive_path = project / "worktrees" / "ARCHIVED.testhost.json"
    archive = json.loads(archive_path.read_text(encoding="utf-8"))
    assert len(archive["records"]) == 1
    assert not (project / "worktrees" / "slot01").exists()
    assert (project / "worktrees" / "ACTIVE.testhost.journal").is_file()

    original_archive = archive_path.read_text(encoding="utf-8")
    archive["records"][0]["purpose"] = "mismatched archive"
    archive_path.write_text(json.dumps(archive, indent=2) + "\n", encoding="utf-8")
    recovered = command(project, "recover", "--coordinator-pid", str(os.getpid()))

    assert recovered.returncode == 0, recovered.stderr
    assert active_slots(project) == []
    assert archive_path.read_text(encoding="utf-8") == original_archive
    archive = json.loads(archive_path.read_text(encoding="utf-8"))
    assert len(archive["records"]) == 1
    assert not (project / "worktrees" / "ACTIVE.testhost.journal").exists()


def test_changed_finish_journal_cannot_redirect_deletion(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)
    first = create(project)
    second = create(
        project,
        slot="slot02",
        agent="codex-2",
        branch="codex/second",
    )
    assert first.returncode == 0, first.stderr
    assert second.returncode == 0, second.stderr
    journal_path = prepare_removal_journal(project)
    journal = json.loads(journal_path.read_text(encoding="utf-8"))
    journal["record"]["checkouts"][0]["path"] = "worktrees/slot02/product"
    journal_path.write_text(json.dumps(journal, indent=2) + "\n", encoding="utf-8")
    shutil.rmtree(control_directory(project) / "EVENTS.testhost")

    refused = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
    )

    assert refused.returncode == 3
    assert "does not exactly match ACTIVE" in refused.stderr
    assert checkout(project, "slot01").is_dir()
    assert checkout(project, "slot02").is_dir()
    assert len(active_slots(project)) == 2


def test_changed_finish_phase_cannot_deregister_present_slot(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    journal_path = prepare_removal_journal(project)
    journal = json.loads(journal_path.read_text(encoding="utf-8"))
    journal["phase"] = "removed"
    journal["removed"] = ["product"]
    journal_path.write_text(json.dumps(journal, indent=2) + "\n", encoding="utf-8")
    shutil.rmtree(control_directory(project) / "EVENTS.testhost")

    refused = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
    )

    assert refused.returncode == 3
    assert "physical slot still exists" in refused.stderr
    assert checkout(project).is_dir()
    assert len(active_slots(project)) == 1


def test_recovery_refuses_checkout_head_changed_after_finish_prepared(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    journal_path = prepare_removal_journal(project)
    commit_task(repository, checkout(project), "codex/task")

    refused = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
    )

    assert refused.returncode == 3
    assert "changed after salvage" in refused.stderr
    assert "rerun remove" in refused.stderr
    assert journal_path.is_file()
    assert checkout(project).is_dir()
    assert len(active_slots(project)) == 1


@pytest.mark.parametrize("change", ["tracked", "untracked"])
def test_finish_refuses_dirty_or_untracked_without_deleting(
    tmp_path: Path, change: str
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    tree = checkout(project)
    if change == "tracked":
        (tree / "seed.txt").write_text("changed\n", encoding="utf-8")
    else:
        (tree / "untracked.txt").write_text("intended\n", encoding="utf-8")

    refused = finish(project)

    assert refused.returncode == 3
    assert "dirty or has untracked/ignored files" in refused.stderr
    assert tree.is_dir()
    assert len(active_slots(project)) == 1
    assert not (project / "worktrees" / "ACTIVE.testhost.journal").exists()


def test_finish_refuses_local_only_commit_without_deleting(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    tree = checkout(project)
    git(tree, "config", "user.name", "Wrkslots Test")
    git(tree, "config", "user.email", "wrkslots@example.invalid")
    (tree / "local.txt").write_text("local only\n", encoding="utf-8")
    git(tree, "add", "local.txt")
    git(tree, "commit", "-m", "local only")

    refused = finish(project)

    assert refused.returncode == 3
    assert "not reachable from any refs/remotes/origin" in refused.stderr
    assert tree.is_dir()
    assert len(active_slots(project)) == 1


def test_forged_local_remote_tracking_refs_do_not_prove_publication(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    tree = checkout(project)
    git(tree, "config", "user.name", "Wrkslots Test")
    git(tree, "config", "user.email", "wrkslots@example.invalid")
    (tree / "forged.txt").write_text("not published\n", encoding="utf-8")
    git(tree, "add", "forged.txt")
    git(tree, "commit", "-m", "not published")
    head = git(tree, "rev-parse", "HEAD").stdout.strip()
    git(tree, "update-ref", "refs/remotes/origin/forged", head)
    git(tree, "update-ref", "refs/remotes/origin/main", head)

    refused = finish(project)

    assert refused.returncode == 3
    assert "not reachable from any refs/remotes/origin" in refused.stderr
    assert tree.is_dir()
    assert len(active_slots(project)) == 1


def test_pushed_but_unmerged_branch_is_not_landed(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    tree = checkout(project)
    git(tree, "config", "user.name", "Wrkslots Test")
    git(tree, "config", "user.email", "wrkslots@example.invalid")
    (tree / "pushed.txt").write_text("pushed only\n", encoding="utf-8")
    git(tree, "add", "pushed.txt")
    git(tree, "commit", "-m", "pushed only")
    git(tree, "push", "-u", "origin", "codex/task")

    refused = finish(project)

    assert refused.returncode == 3
    assert "not an ancestor of configured landed ref" in refused.stderr
    assert tree.is_dir()
    assert len(active_slots(project)) == 1


def test_active_state_cannot_redirect_landed_authority_to_task_branch(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    tree = checkout(project)
    git(tree, "config", "user.name", "Wrkslots Test")
    git(tree, "config", "user.email", "wrkslots@example.invalid")
    (tree / "pushed.txt").write_text("pushed only\n", encoding="utf-8")
    git(tree, "add", "pushed.txt")
    git(tree, "commit", "-m", "pushed only")
    git(tree, "push", "-u", "origin", "codex/task")
    active_path = project / "worktrees" / "ACTIVE.testhost.json"
    state = json.loads(active_path.read_text(encoding="utf-8"))
    state["slots"][0]["checkouts"][0]["landed_ref"] = (
        "refs/remotes/origin/codex/task"
    )
    active_path.write_text(json.dumps(state, indent=2) + "\n", encoding="utf-8")

    refused = finish(project)

    assert refused.returncode == 3
    assert "not an ancestor of configured landed ref refs/remotes/origin/main" in refused.stderr
    assert tree.is_dir()
    assert len(active_slots(project)) == 1


def test_changed_remote_url_cannot_redirect_landed_authority(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    tree = checkout(project)
    git(tree, "config", "user.name", "Wrkslots Test")
    git(tree, "config", "user.email", "wrkslots@example.invalid")
    (tree / "redirected.txt").write_text("alternate only\n", encoding="utf-8")
    git(tree, "add", "redirected.txt")
    git(tree, "commit", "-m", "alternate only")
    alternate = tmp_path / "alternate.git"
    subprocess.run(
        ["git", "init", "--bare", "--initial-branch=main", str(alternate)],
        check=True,
        capture_output=True,
        text=True,
    )
    git(tree, "push", str(alternate), "HEAD:main")
    git(tree, "remote", "set-url", "origin", str(alternate))

    refused = finish(project)

    assert refused.returncode == 3
    assert "remote origin URL changed" in refused.stderr
    assert tree.is_dir()
    assert len(active_slots(project)) == 1


def test_create_fetches_remote_before_selecting_default_start(tmp_path: Path) -> None:
    project, _repository, remote = make_project(tmp_path)
    updater = tmp_path / "updater"
    subprocess.run(
        ["git", "clone", str(remote), str(updater)],
        check=True,
        capture_output=True,
        text=True,
    )
    git(updater, "config", "user.name", "Wrkslots Test")
    git(updater, "config", "user.email", "wrkslots@example.invalid")
    (updater / "advanced.txt").write_text("advanced\n", encoding="utf-8")
    git(updater, "add", "advanced.txt")
    git(updater, "commit", "-m", "advance remote main")
    advanced = git(updater, "rev-parse", "HEAD").stdout.strip()
    git(updater, "push", "origin", "main")

    made = create(project)

    assert made.returncode == 0, made.stderr
    assert git(checkout(project), "rev-parse", "HEAD").stdout.strip() == advanced


def test_create_defaults_to_origin_and_accepts_a_configured_remote_name(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)

    defaulted = create(project, slot="slot01", branch="codex/default-origin")

    assert defaulted.returncode == 0, defaulted.stderr
    default_row = active_slots(project)[0]
    assert isinstance(default_row, dict)
    default_checkouts = default_row["checkouts"]
    assert isinstance(default_checkouts, list)
    default_checkout = default_checkouts[0]
    assert isinstance(default_checkout, dict)
    assert default_checkout["remote"] == "origin"

    second_root = tmp_path / "second"
    second_root.mkdir()
    second_project, second_repository, second_remote = make_project(second_root)
    git(second_repository, "remote", "add", "upstream", str(second_remote))
    selected = command(
        second_project,
        "create",
        "slot02",
        "--agent",
        "codex-2",
        "--task",
        "task-slot02",
        "--purpose",
        "configured remote selection",
        "--owner-pid",
        str(os.getpid()),
        "--coordinator-pid",
        str(os.getpid()),
        "--repo",
        "product=repo",
        "--remote",
        "product=upstream",
        "--branch",
        "product=codex/upstream",
    )

    assert selected.returncode == 0, selected.stderr
    selected_row = active_slots(second_project)[0]
    assert isinstance(selected_row, dict)
    selected_checkouts = selected_row["checkouts"]
    assert isinstance(selected_checkouts, list)
    checkout_row = selected_checkouts[0]
    assert isinstance(checkout_row, dict)
    assert checkout_row["remote"] == "upstream"
    assert checkout_row["landed_ref"] == "refs/remotes/upstream/main"


def test_create_can_verify_the_selected_configured_remote_url(tmp_path: Path) -> None:
    project, repository, remote = make_project(tmp_path)
    git(repository, "remote", "add", "upstream", str(remote))

    selected = command(
        project,
        "create",
        "slot01",
        "--agent",
        "codex-1",
        "--task",
        "task-slot01",
        "--purpose",
        "configured remote URL verification",
        "--owner-pid",
        str(os.getpid()),
        "--coordinator-pid",
        str(os.getpid()),
        "--repo",
        "product=repo",
        "--remote",
        "product=upstream",
        "--remote-url",
        f"product={remote}",
        "--branch",
        "product=codex/upstream",
    )

    assert selected.returncode == 0, selected.stderr
    row = active_slots(project)[0]
    assert isinstance(row, dict)
    checkouts = row["checkouts"]
    assert isinstance(checkouts, list)
    checkout_row = checkouts[0]
    assert isinstance(checkout_row, dict)
    assert checkout_row["remote"] == "upstream"
    assert checkout_row["remote_url_sha256"] == wrkslots._GitVcs().remote_url_sha256(
        checkout(project), "upstream"
    )

    mismatched = command(
        project,
        "create",
        "slot02",
        "--agent",
        "codex-2",
        "--task",
        "task-slot02",
        "--purpose",
        "mismatched remote URL",
        "--owner-pid",
        str(os.getpid()),
        "--coordinator-pid",
        str(os.getpid()),
        "--repo",
        "product=repo",
        "--remote",
        "product=upstream",
        "--remote-url",
        "product=file:///wrong",
        "--branch",
        "product=codex/wrong",
    )

    assert mismatched.returncode == 3
    assert "differs from trusted provisioning input" in mismatched.stderr
    assert len(active_slots(project)) == 1
    assert not checkout(project, "slot02").exists()


def test_ignored_file_is_preserved_by_finish_refusal(tmp_path: Path) -> None:
    project, repository, _remote = make_project(tmp_path)
    (repository / ".gitignore").write_text("ignored.bin\n", encoding="utf-8")
    git(repository, "add", ".gitignore")
    git(repository, "commit", "-m", "ignore test artifact")
    git(repository, "push", "origin", "main")
    made = create(project)
    assert made.returncode == 0, made.stderr
    tree = checkout(project)
    ignored = tree / "ignored.bin"
    ignored.write_text("possibly intended\n", encoding="utf-8")

    refused = finish(project)

    assert refused.returncode == 3
    assert "untracked/ignored" in refused.stderr
    assert ignored.read_text(encoding="utf-8") == "possibly intended\n"
    assert len(active_slots(project)) == 1


def test_finish_refuses_unfinished_git_operation_without_deleting(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    tree = checkout(project)
    merge_head = Path(git(tree, "rev-parse", "--git-path", "MERGE_HEAD").stdout.strip())
    if not merge_head.is_absolute():
        merge_head = tree / merge_head
    merge_head.write_text(git(tree, "rev-parse", "HEAD").stdout, encoding="ascii")

    refused = finish(project)

    assert refused.returncode == 3
    assert "unfinished Git operation" in refused.stderr
    assert tree.is_dir()
    assert len(active_slots(project)) == 1


def test_git_environment_cannot_redirect_finish_checks(tmp_path: Path) -> None:
    project, repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    commit_task(repository, checkout(project), "codex/task")

    completed = finish(
        project,
        env={
            "GIT_DIR": str(tmp_path / "redirected.git"),
            "GIT_WORK_TREE": str(tmp_path / "redirected-tree"),
            "GIT_INDEX_FILE": str(tmp_path / "redirected-index"),
        },
    )

    assert completed.returncode == 0, completed.stderr
    assert checkout(project).is_dir()


@pytest.mark.parametrize("flag", ["--assume-unchanged", "--skip-worktree"])
def test_finish_refuses_nonordinary_index_flags(tmp_path: Path, flag: str) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    tree = checkout(project)
    git(tree, "update-index", flag, "seed.txt")
    (tree / "seed.txt").write_text("hidden change\n", encoding="utf-8")

    refused = finish(project)

    assert refused.returncode == 3
    expected = "assume-unchanged" if flag == "--assume-unchanged" else "skip-worktree"
    assert expected in refused.stderr
    assert tree.is_dir()


@pytest.mark.parametrize("mechanism", ["replace", "grafts", "shallow"])
def test_finish_refuses_nonordinary_history(tmp_path: Path, mechanism: str) -> None:
    project, repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    tree = checkout(project)
    commit_task(repository, tree, "codex/task")
    common = Path(
        git(tree, "rev-parse", "--path-format=absolute", "--git-common-dir").stdout.strip()
    )
    if mechanism == "replace":
        head = git(tree, "rev-parse", "HEAD").stdout.strip()
        parent = git(tree, "rev-parse", "HEAD^").stdout.strip()
        git(tree, "replace", head, parent)
    elif mechanism == "grafts":
        grafts = common / "info" / "grafts"
        grafts.parent.mkdir(exist_ok=True)
        grafts.write_text(git(tree, "rev-parse", "HEAD").stdout, encoding="ascii")
    else:
        (common / "shallow").write_text(
            git(tree, "rev-parse", "HEAD").stdout, encoding="ascii"
        )

    refused = finish(project)

    assert refused.returncode == 3
    expected = "replacement refs" if mechanism == "replace" else mechanism
    assert expected in refused.stderr
    assert tree.is_dir()


def test_remote_and_landed_ancestry_records_handoff_then_allows_removal(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    tree = checkout(project)
    task_head = commit_task(repository, tree, "codex/task")

    completed = finish(project)

    assert completed.returncode == 0, completed.stderr
    assert (project / "worktrees" / "slot01").is_dir()
    slots = active_slots(project)
    assert len(slots) == 1
    row = slots[0]
    assert isinstance(row, dict)
    assert row["handoff"]["validation"] == ["pytest"]
    assert row["checkouts"][0]["head"] == task_head
    assert row["checkouts"][0]["containing_remote_refs"]

    mark_owner_dead(project)
    set_liveness(project, "dead")
    removed = remove(project)

    assert removed.returncode == 0, removed.stderr
    assert not (project / "worktrees" / "slot01").exists()
    assert active_slots(project) == []
    archive = json.loads(
        (project / "worktrees" / "ARCHIVED.testhost.json").read_text(encoding="utf-8")
    )
    assert archive["records"][0]["physical_storage"] == "removed"
    assert archive["records"][0]["checkouts"][0]["head"] == task_head
    assert archive["records"][0]["validation"] == ["pytest"]


def test_agent_reclaim_pushes_unpushed_commits_and_nonignored_files(
    tmp_path: Path,
) -> None:
    project, _repository, remote = make_project(tmp_path, cache_globs=("target",))
    made = create(project)
    assert made.returncode == 0, made.stderr
    tree = checkout(project)
    local_commit = commit_local(tree, "local.txt", "local commit\n", "local")
    (tree / "seed.txt").write_text("dirty tracked file\n", encoding="utf-8")
    (tree / "untracked.txt").write_text("untracked work\n", encoding="utf-8")
    (tree / ".gitignore").write_text("ignored.txt\ntarget/\n", encoding="utf-8")
    (tree / "ignored.txt").write_text("ignored work\n", encoding="utf-8")
    (tree / "target").mkdir()
    (tree / "target" / "build-output").write_text("regenerable\n", encoding="utf-8")
    mark_owner_dead(project)
    set_liveness(project, "dead")

    removed = raw_command(
        project,
        "remove",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
    )

    assert removed.returncode == 0, removed.stderr
    assert not tree.exists()
    archive = json.loads(
        (project / "worktrees" / "ARCHIVED.testhost.json").read_text(
            encoding="utf-8"
        )
    )
    records = archive["records"]
    assert isinstance(records, list)
    archived = records[0]
    assert isinstance(archived, dict)
    salvage = archived["salvage"]
    assert isinstance(salvage, list)
    receipt = salvage[0]
    assert isinstance(receipt, dict)
    assert receipt["source_head"] == local_commit
    assert receipt["disposition"] == "salvaged"
    salvage_commit = receipt["salvage_commit"]
    remote_ref = receipt["remote_ref"]
    assert git(remote, "rev-parse", remote_ref).stdout.strip() == salvage_commit
    names = set(git(remote, "ls-tree", "-r", "--name-only", salvage_commit).stdout.splitlines())
    assert {
        ".gitignore",
        "local.txt",
        "seed.txt",
        "untracked.txt",
    } <= names
    assert "ignored.txt" not in names
    assert "target/build-output" not in names
    assert git(remote, "show", f"{salvage_commit}:seed.txt").stdout == "dirty tracked file\n"
    events = wrkslots._load_events(wrkslots._load_config(str(project), "testhost"))
    started = [event for event in events if event["kind"] == "reclaim-started"]
    assert len(started) == 1
    payload = started[0]["payload"]
    assert isinstance(payload, dict)
    assert payload["coordinator_authorized"] is False


def test_agent_reclaim_never_uploads_oversized_ignored_file(
    tmp_path: Path,
) -> None:
    project, _repository, remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    tree = checkout(project)
    (tree / ".gitignore").write_text("oversized.bin\n", encoding="utf-8")
    git(tree, "add", ".gitignore")
    git(tree, "commit", "-m", "ignore generated artifact")
    git(tree, "push", "origin", "HEAD:main")
    oversized = tree / "oversized.bin"
    with oversized.open("wb") as output:
        output.truncate(100 * 1024**2 + 1)
    (tree / "seed.txt").write_text("authored tracked change\n", encoding="utf-8")
    (tree / "untracked.txt").write_text("authored untracked change\n", encoding="utf-8")
    mark_owner_dead(project)
    set_liveness(project, "dead")

    removed = raw_command(
        project,
        "remove",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
    )

    assert removed.returncode == 0, removed.stderr
    assert not tree.exists()
    archive = json.loads(
        (project / "worktrees" / "ARCHIVED.testhost.json").read_text(
            encoding="utf-8"
        )
    )
    receipt = archive["records"][0]["salvage"][0]
    assert receipt["disposition"] == "salvaged"
    salvage_commit = receipt["salvage_commit"]
    remote_ref = receipt["remote_ref"]
    assert git(remote, "rev-parse", remote_ref).stdout.strip() == salvage_commit
    names = set(git(remote, "ls-tree", "-r", "--name-only", salvage_commit).stdout.splitlines())
    assert "seed.txt" in names
    assert "untracked.txt" in names
    assert "oversized.bin" not in names
    assert git(remote, "show", f"{salvage_commit}:seed.txt").stdout == "authored tracked change\n"


def test_agent_reclaim_does_not_salvage_ignored_only_payload(
    tmp_path: Path,
) -> None:
    project, _repository, remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    tree = checkout(project)
    (tree / ".gitignore").write_text("generated/\n", encoding="utf-8")
    git(tree, "add", ".gitignore")
    git(tree, "commit", "-m", "ignore generated artifacts")
    git(tree, "push", "origin", "HEAD:main")
    generated = tree / "generated"
    generated.mkdir()
    for index in range(22):
        name = f"artifact-{index:02}.bin"
        with (generated / name).open("wb") as output:
            output.truncate(100 * 1024**2)
    mark_owner_dead(project)
    set_liveness(project, "dead")
    removed = raw_command(
        project,
        "remove",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
    )

    assert removed.returncode == 0, removed.stderr
    assert not tree.exists()
    assert git(
        remote,
        "for-each-ref",
        "--format=%(refname)",
        "refs/heads/salvage",
    ).stdout == ""
    archive = json.loads(
        (project / "worktrees" / "ARCHIVED.testhost.json").read_text(
            encoding="utf-8"
        )
    )
    receipt = archive["records"][0]["salvage"][0]
    assert receipt["disposition"] == "already-published"


def test_agent_reclaim_salvages_uncommitted_submodule_files_before_removal(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    submodule_remote = tmp_path / "submodule.git"
    submodule_source = tmp_path / "submodule-source"
    subprocess.run(
        ["git", "init", "--bare", "--initial-branch=main", str(submodule_remote)],
        check=True,
        capture_output=True,
        text=True,
    )
    subprocess.run(
        ["git", "clone", str(submodule_remote), str(submodule_source)],
        check=True,
        capture_output=True,
        text=True,
    )
    git(submodule_source, "config", "user.name", "Wrkslots Test")
    git(submodule_source, "config", "user.email", "wrkslots@example.invalid")
    (submodule_source / "base.txt").write_text("base\n", encoding="utf-8")
    git(submodule_source, "add", "base.txt")
    git(submodule_source, "commit", "-m", "submodule base")
    git(submodule_source, "push", "-u", "origin", "main")
    git(
        repository,
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        str(submodule_remote),
        "component",
    )
    git(repository, "commit", "-am", "add component")
    git(repository, "push", "origin", "main")
    update_configuration(
        project,
        post_provision_hooks=[
            "git -c protocol.file.allow=always submodule update --init --recursive"
        ],
    )

    made = create(project)
    assert made.returncode == 0, made.stderr
    tree = checkout(project)
    nested = tree / "component"
    assert nested.is_dir()
    (nested / "uncommitted.txt").write_text("must survive\n", encoding="utf-8")
    mark_owner_dead(project)
    set_liveness(project, "dead")

    removed = raw_command(
        project,
        "remove",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
    )

    assert removed.returncode == 0, removed.stderr
    assert not tree.exists()
    archive = json.loads(
        (project / "worktrees" / "ARCHIVED.testhost.json").read_text(
            encoding="utf-8"
        )
    )
    receipts = {
        receipt["checkout"]: receipt for receipt in archive["records"][0]["salvage"]
    }
    nested_receipt = receipts["product/component"]
    assert nested_receipt["disposition"] == "salvaged"
    assert (
        git(
            submodule_remote,
            "show",
            f"{nested_receipt['salvage_commit']}:uncommitted.txt",
        ).stdout
        == "must survive\n"
    )


def test_validate_slot_removes_dirty_checkout_without_salvage(tmp_path: Path) -> None:
    project, _repository, remote = make_project(tmp_path)
    made = create(project, slot_type="validate", branch=None)
    assert made.returncode == 0, made.stderr
    tree = checkout(project, slot_type="validate")
    assert tree == project / "worktrees" / "validate" / "slot01" / "product"
    (tree / "result.tmp").write_text("disposable result\n", encoding="utf-8")
    removed = command(
        project,
        "remove",
        "slot01",
        "--validate-complete",
        "--coordinator-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
    )

    assert removed.returncode == 0, removed.stderr
    assert not tree.exists()
    assert git(
        project / "repo",
        "show-ref",
        "--verify",
        "--quiet",
        "refs/heads/wrkslots/validate/testhost/slot01/product",
        check=False,
    ).returncode == 1
    assert git(remote, "for-each-ref", "--format=%(refname)", "refs/heads/salvage").stdout == ""
    archive = json.loads(
        (project / "worktrees" / "ARCHIVED.testhost.json").read_text(
            encoding="utf-8"
        )
    )
    row = archive["records"][0]
    assert row["slot_type"] == "validate"
    assert row["salvage"] == []
    assert row["validation"] == [
        "slot type validate: authored work is excluded by construction, so salvage was not run"
    ]


def test_validate_complete_removes_dead_owner_checkout_despite_shared_cgroup(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project, slot_type="validate", branch=None)
    assert made.returncode == 0, made.stderr
    tree = checkout(project, slot_type="validate")
    mark_owner_dead_in_current_cgroup(project)
    set_liveness(project, "dead")

    removed = command(
        project,
        "remove",
        "slot01",
        "--validate-complete",
        "--coordinator-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
    )

    assert removed.returncode == 0, removed.stderr
    assert not tree.exists()
    assert active_slots(project) == []


def test_validate_complete_removes_nested_git_metadata_in_configured_cache(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path, cache_globs=("target",))
    made = create(project, slot_type="validate", branch=None)
    assert made.returncode == 0, made.stderr
    tree = checkout(project, slot_type="validate")
    nested_git = tree / "target" / "install-build" / "dependency" / ".git"
    nested_git.mkdir(parents=True)
    (nested_git / "HEAD").write_text("ref: refs/heads/main\n", encoding="utf-8")
    mark_owner_dead_in_current_cgroup(project)
    set_liveness(project, "dead")

    removed = command(
        project,
        "remove",
        "slot01",
        "--validate-complete",
        "--coordinator-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
    )

    assert removed.returncode == 0, removed.stderr
    assert not tree.exists()
    assert active_slots(project) == []


def test_validate_removal_without_complete_refuses_nested_git_metadata(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path, cache_globs=("target",))
    made = create(project, slot_type="validate", branch=None)
    assert made.returncode == 0, made.stderr
    tree = checkout(project, slot_type="validate")
    nested_git = tree / "target" / "install-build" / "dependency" / ".git"
    nested_git.mkdir(parents=True)
    sentinel = nested_git / "HEAD"
    sentinel.write_text("ref: refs/heads/main\n", encoding="utf-8")
    mark_owner_dead(project)
    set_liveness(project, "dead")

    removed = remove(project)

    assert removed.returncode == 3
    assert "nested Git metadata" in removed.stderr
    assert sentinel.is_file()
    assert tree.is_dir()
    assert active_slots(project)


def test_validate_complete_still_refuses_live_owner_from_shared_cgroup(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project, slot_type="validate", branch=None)
    assert made.returncode == 0, made.stderr
    tree = checkout(project, slot_type="validate")
    owner = subprocess.Popen(["sleep", "60"], cwd=tree, text=True)
    try:
        replace_owner(project, wrkslots._read_process_identity(owner.pid))
        set_liveness(project, "dead")

        refused = command(
            project,
            "remove",
            "slot01",
            "--validate-complete",
            "--coordinator-pid",
            str(os.getpid()),
            "--expected-generation",
            "1",
        )

        assert refused.returncode == 3
        assert "requires a proven-dead recorded owner; owner is live" in refused.stderr
        assert tree.is_dir()
        assert active_slots(project)
    finally:
        terminate_process(owner)


def test_validate_complete_still_refuses_live_path_use_after_owner_dies(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project, slot_type="validate", branch=None)
    assert made.returncode == 0, made.stderr
    tree = checkout(project, slot_type="validate")
    mark_owner_dead_in_current_cgroup(project)
    set_liveness(project, "dead")
    user = subprocess.Popen(["sleep", "60"], cwd=tree, text=True)
    try:
        refused = command(
            project,
            "remove",
            "slot01",
            "--validate-complete",
            "--coordinator-pid",
            str(os.getpid()),
            "--expected-generation",
            "1",
        )

        assert refused.returncode == 3
        assert "live process" in refused.stderr
        assert "uses slot" in refused.stderr
        assert tree.is_dir()
        assert active_slots(project)
    finally:
        terminate_process(user)


def test_validate_remove_accepts_exact_validation_removal_proof(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    commit_validation_removal_schema(repository)
    slot_path = prepare_dead_validate_slots(project, ("slot01",))["slot01"]
    manifest, artifacts = prepare_validation_removal_proof(project, "slot01")

    stub_validate_batch_censuses(monkeypatch)
    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "remove",
            "slot01",
            "--coordinator-pid",
            str(os.getpid()),
            "--expected-generation",
            "1",
            "--validation-proof-manifest",
            manifest.relative_to(project).as_posix(),
            "--completed-record",
            artifacts["run-record"].relative_to(project).as_posix(),
        ]
    )

    assert rc == 0, capsys.readouterr().err
    assert not slot_path.exists()
    assert artifacts["scorecard-handoff"].is_file()
    assert (
        artifacts["scorecard-handoff"].parent / "SCORECARD.md"
    ).is_file()
    assert (
        artifacts["scorecard-handoff"].parent
        / "ci"
        / "compat-envelope"
        / "cells.json"
    ).is_file()
    assert active_slots(project) == []


def test_validate_remove_accepts_schema_five_no_result_detail_projection(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    commit_validation_removal_schema(repository, schema_version=5)
    slot_path = prepare_dead_validate_slots(project, ("slot01",))["slot01"]
    manifest, artifacts = prepare_validation_removal_proof(project, "slot01")

    record_value: object = json.loads(
        artifacts["run-record"].read_text(encoding="utf-8")
    )
    result_value: object = json.loads(
        artifacts["service-result"].read_text(encoding="utf-8")
    )
    assert isinstance(record_value, dict)
    assert isinstance(result_value, dict)
    detail = ["validation host unavailable", "retry on another host"]
    record_value.update(
        {
            "result": "no-result",
            "final_validate_status": "COULD_NOT_RUN",
            "detail": "\n".join(detail),
            "exit_code": 75,
            "executed_tests": None,
            "passed_tests": None,
        }
    )
    result_value.update(
        {
            "final_validate_status": "COULD_NOT_RUN",
            "detail": detail,
            "exit_code": 75,
            "executed_tests": None,
            "passed_tests": None,
        }
    )
    artifacts["run-record"].write_text(
        json.dumps(record_value, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    artifacts["service-result"].write_text(
        json.dumps(result_value, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    rebind_validation_removal_artifact(
        project, manifest, "run-record", artifacts["run-record"]
    )
    rebind_validation_removal_artifact(
        project, manifest, "service-result", artifacts["service-result"]
    )

    stub_validate_batch_censuses(monkeypatch)
    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "remove",
            "slot01",
            "--coordinator-pid",
            str(os.getpid()),
            "--expected-generation",
            "1",
            "--validation-proof-manifest",
            manifest.relative_to(project).as_posix(),
            "--completed-record",
            artifacts["run-record"].relative_to(project).as_posix(),
        ]
    )

    assert rc == 0, capsys.readouterr().err
    assert not slot_path.exists()
    assert active_slots(project) == []


@pytest.mark.parametrize("schema_version", (1, 2, 3))
def test_validate_remove_accepts_historical_service_result_projection(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    schema_version: int,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    commit_validation_removal_schema(repository, schema_version=schema_version)
    slot_path = prepare_dead_validate_slots(project, ("slot01",))["slot01"]
    manifest, artifacts = prepare_validation_removal_proof(project, "slot01")
    record: object = json.loads(
        artifacts["run-record"].read_text(encoding="utf-8")
    )
    assert isinstance(record, dict)
    assert record["state"] == "unknown"
    assert record["result"] == "unknown"
    assert record["result_source"] == "historical-validation-service-result"
    assert record["detail"] == (
        "validation-service-result-schema_version: historical schema "
        f"{schema_version} is readable but has no current authority"
    )
    assert record["passed_tests"] is None
    assert ("selection_mode" in record) is (schema_version == 3)
    assert ("scorecard_writeback" in record) is (schema_version >= 2)

    stub_validate_batch_censuses(monkeypatch)
    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "remove",
            "slot01",
            "--coordinator-pid",
            str(os.getpid()),
            "--expected-generation",
            "1",
            "--validation-proof-manifest",
            manifest.relative_to(project).as_posix(),
            "--completed-record",
            artifacts["run-record"].relative_to(project).as_posix(),
        ]
    )

    assert rc == 0, capsys.readouterr().err
    assert not slot_path.exists()
    assert active_slots(project) == []


@pytest.mark.parametrize(
    ("field", "replacement"),
    (
        ("result_source", "validation-service-result"),
        ("state", "completed"),
        ("result", "success"),
        ("detail", "historical result"),
        ("passed_tests", 0),
    ),
)
def test_validate_remove_refuses_changed_historical_projection(
    tmp_path: Path,
    field: str,
    replacement: object,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    commit_validation_removal_schema(repository, schema_version=3)
    slot_path = prepare_dead_validate_slots(project, ("slot01",))["slot01"]
    manifest, artifacts = prepare_validation_removal_proof(project, "slot01")
    record: object = json.loads(
        artifacts["run-record"].read_text(encoding="utf-8")
    )
    assert isinstance(record, dict)
    record[field] = replacement
    artifacts["run-record"].write_text(
        json.dumps(record, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    rebind_validation_removal_artifact(
        project, manifest, "run-record", artifacts["run-record"]
    )

    refused = command(
        project,
        "remove",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        "--validation-proof-manifest",
        manifest.relative_to(project).as_posix(),
        "--completed-record",
        artifacts["run-record"].relative_to(project).as_posix(),
    )

    assert refused.returncode == 3
    assert "does not contain an evidenced terminal result" in refused.stderr
    assert slot_path.is_dir()
    assert active_slots(project)


@pytest.mark.parametrize(
    ("field", "replacement", "message"),
    (
        ("slot", "another-slot", "names slot"),
        ("generation", 2, "names generation"),
    ),
)
def test_validate_remove_refuses_proof_for_another_slot_generation(
    tmp_path: Path,
    field: str,
    replacement: object,
    message: str,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    commit_validation_removal_schema(repository)
    slot_path = prepare_dead_validate_slots(project, ("slot01",))["slot01"]
    manifest, artifacts = prepare_validation_removal_proof(project, "slot01")
    value = json.loads(manifest.read_text(encoding="utf-8"))
    value[field] = replacement
    manifest.write_text(json.dumps(value), encoding="utf-8")

    refused = command(
        project,
        "remove",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        "--validation-proof-manifest",
        manifest.relative_to(project).as_posix(),
        "--completed-record",
        artifacts["run-record"].relative_to(project).as_posix(),
    )

    assert refused.returncode == 3
    assert message in refused.stderr
    assert slot_path.is_dir()
    assert active_slots(project)


def test_validate_remove_refuses_malformed_proof_without_sealing(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    commit_validation_removal_schema(repository)
    slot_path = prepare_dead_validate_slots(project, ("slot01",))["slot01"]
    manifest, artifacts = prepare_validation_removal_proof(project, "slot01")
    contents = manifest.read_text(encoding="utf-8")
    manifest.write_text(
        contents.replace('"schema": 1,', '"schema": 1, "schema": 1,', 1),
        encoding="utf-8",
    )

    refused = command(
        project,
        "remove",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        "--validation-proof-manifest",
        manifest.relative_to(project).as_posix(),
        "--completed-record",
        artifacts["run-record"].relative_to(project).as_posix(),
    )

    assert refused.returncode == 3
    assert "malformed JSON" in refused.stderr
    assert "duplicate key" in refused.stderr
    assert slot_path.is_dir()
    assert stat.S_IMODE(slot_path.stat().st_mode) != 0o700


@pytest.mark.parametrize(
    ("mutation", "message"),
    (
        ("cross-slot-service", "service-result path does not match"),
        ("cross-slot-record", "differs from --completed-record"),
        ("record-generation", "exact slot generation"),
        ("path-swap", "scorecard-handoff"),
        ("hardlink", "share one regular-file identity"),
        ("noncanonical-schema", "producer-schema path does not match"),
        ("noncanonical-handoff", "scorecard-handoff path does not match"),
    ),
)
def test_validate_remove_refuses_noncanonical_proof_role_binding(
    tmp_path: Path,
    mutation: str,
    message: str,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    commit_validation_removal_schema(repository)
    slots = (
        ("slot01", "slot02")
        if mutation.startswith("cross-slot")
        else ("slot01",)
    )
    slot_path = prepare_dead_validate_slots(project, slots)["slot01"]
    manifest, artifacts = prepare_validation_removal_proof(project, "slot01")

    if mutation in {"cross-slot-service", "cross-slot-record"}:
        _other_manifest, other_artifacts = prepare_validation_removal_proof(
            project, "slot02"
        )
        rebind_validation_removal_artifact(
            project,
            manifest,
            (
                "service-result"
                if mutation == "cross-slot-service"
                else "run-record"
            ),
            other_artifacts[
                "service-result"
                if mutation == "cross-slot-service"
                else "run-record"
            ],
        )
    elif mutation == "record-generation":
        record_value: object = json.loads(
            artifacts["run-record"].read_text(encoding="utf-8")
        )
        assert isinstance(record_value, dict)
        record_value["wrkslots_generation"] = 2
        artifacts["run-record"].write_text(
            json.dumps(record_value, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        rebind_validation_removal_artifact(
            project, manifest, "run-record", artifacts["run-record"]
        )
    elif mutation == "path-swap":
        value: object = json.loads(manifest.read_text(encoding="utf-8"))
        assert isinstance(value, dict)
        rows = value["artifacts"]
        assert isinstance(rows, list)
        by_role = {
            str(row["role"]): row for row in rows if isinstance(row, dict)
        }
        service = by_role["service-result"]
        handoff = by_role["scorecard-handoff"]
        keys = ("path", "identity", "size", "sha256")
        service_values = {key: service[key] for key in keys}
        for key in keys:
            service[key] = handoff[key]
            handoff[key] = service_values[key]
        manifest.write_text(
            json.dumps(value, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
    elif mutation == "hardlink":
        service_result = artifacts["service-result"]
        service_result.unlink()
        os.link(artifacts["run-record"], service_result)
        rebind_validation_removal_artifact(
            project, manifest, "service-result", service_result
        )
    elif mutation == "noncanonical-schema":
        noncanonical = artifacts["producer-schema"].with_name(
            "other-validation-service-result-schema.json"
        )
        shutil.copyfile(artifacts["producer-schema"], noncanonical)
        rebind_validation_removal_artifact(
            project, manifest, "producer-schema", noncanonical
        )
    elif mutation == "noncanonical-handoff":
        noncanonical = (
            project
            / "ignored"
            / "validate"
            / "scorecard-writebacks"
            / "validate-other"
            / "handoff.json"
        )
        noncanonical.parent.mkdir(parents=True)
        shutil.copyfile(artifacts["scorecard-handoff"], noncanonical)
        rebind_validation_removal_artifact(
            project, manifest, "scorecard-handoff", noncanonical
        )
    else:
        raise AssertionError(mutation)

    refused = command(
        project,
        "remove",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        "--validation-proof-manifest",
        manifest.relative_to(project).as_posix(),
        "--completed-record",
        artifacts["run-record"].relative_to(project).as_posix(),
    )

    assert refused.returncode == 3
    assert message in refused.stderr
    assert slot_path.is_dir()
    assert stat.S_IMODE(slot_path.stat().st_mode) != 0o700
    assert active_slots(project)


@pytest.mark.parametrize(
    ("mutation", "message"),
    (
        ("missing-retained-file", "invalid file set"),
        ("handoff-record-mismatch", "differ from run-record"),
        ("extra-file", "invalid file set"),
        ("extra-symlink", "invalid file set"),
    ),
)
def test_validate_remove_refuses_incomplete_scorecard_evidence(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    mutation: str,
    message: str,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    commit_validation_removal_schema(repository)
    slot_path = prepare_dead_validate_slots(project, ("slot01",))["slot01"]
    manifest, artifacts = prepare_validation_removal_proof(project, "slot01")
    if mutation == "missing-retained-file":
        (artifacts["scorecard-handoff"].parent / "SCORECARD.md").unlink()
    elif mutation == "handoff-record-mismatch":
        record_value: object = json.loads(
            artifacts["run-record"].read_text(encoding="utf-8")
        )
        assert isinstance(record_value, dict)
        rows = record_value["scorecard_writeback_files"]
        assert isinstance(rows, list) and isinstance(rows[0], dict)
        rows[0]["sha256"] = "0" * 64
        artifacts["run-record"].write_text(
            json.dumps(record_value, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        rebind_validation_removal_artifact(
            project, manifest, "run-record", artifacts["run-record"]
        )
    elif mutation == "extra-file":
        (artifacts["scorecard-handoff"].parent / "unexpected.txt").write_text(
            "unexpected\n", encoding="utf-8"
        )
    elif mutation == "extra-symlink":
        (artifacts["scorecard-handoff"].parent / "unexpected-link").symlink_to(
            "SCORECARD.md"
        )
    else:
        raise AssertionError(mutation)
    stub_validate_batch_censuses(monkeypatch)

    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "remove",
            "slot01",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--expected-generation",
            "1",
            "--validation-proof-manifest",
            manifest.relative_to(project).as_posix(),
            "--completed-record",
            artifacts["run-record"].relative_to(project).as_posix(),
        ]
    )

    assert rc == 3
    assert message in capsys.readouterr().err
    assert slot_path.is_dir()
    assert active_slots(project)


@pytest.mark.parametrize(
    ("mutation", "message"),
    (
        ("record-generation", "exact slot generation"),
        ("record-projection", "does not contain an evidenced terminal result"),
        ("producer-passed-exit", "producer-schema does not match"),
        ("producer-failed-exit", "producer-schema does not match"),
        ("producer-nullable", "producer-schema does not match"),
        ("handoff-schema", "scorecard-handoff does not match"),
    ),
)
def test_validate_remove_refuses_boolean_aliases_in_typed_evidence(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    mutation: str,
    message: str,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    schema_version = 3 if mutation == "record-projection" else 4
    commit_validation_removal_schema(repository, schema_version=schema_version)
    slot_path = prepare_dead_validate_slots(project, ("slot01",))["slot01"]
    manifest, artifacts = prepare_validation_removal_proof(project, "slot01")

    if mutation in {"record-generation", "record-projection"}:
        record_value: object = json.loads(
            artifacts["run-record"].read_text(encoding="utf-8")
        )
        assert isinstance(record_value, dict)
        if mutation == "record-generation":
            record_value["wrkslots_generation"] = True
        else:
            record_value["exit_code"] = False
            record_value["executed_nodes"] = True
        artifacts["run-record"].write_text(
            json.dumps(record_value, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        rebind_validation_removal_artifact(
            project, manifest, "run-record", artifacts["run-record"]
        )
    elif mutation.startswith("producer-"):
        schema_value: object = json.loads(
            artifacts["producer-schema"].read_text(encoding="utf-8")
        )
        assert isinstance(schema_value, dict)
        outcomes = schema_value["outcomes"]
        writeback = schema_value["scorecard_writeback"]
        assert isinstance(outcomes, dict) and isinstance(outcomes["PASSED"], dict)
        assert isinstance(outcomes["FAILED"], dict)
        assert isinstance(writeback, dict)
        if mutation == "producer-passed-exit":
            outcomes["PASSED"]["validation_exit_code"] = False
        elif mutation == "producer-failed-exit":
            outcomes["FAILED"]["validation_exit_code"] = True
        elif mutation == "producer-nullable":
            writeback["nullable"] = 1
        else:
            raise AssertionError(mutation)
        artifacts["producer-schema"].write_text(
            json.dumps(schema_value, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        rebind_validation_removal_artifact(
            project, manifest, "producer-schema", artifacts["producer-schema"]
        )
    elif mutation == "handoff-schema":
        handoff_value: object = json.loads(
            artifacts["scorecard-handoff"].read_text(encoding="utf-8")
        )
        assert isinstance(handoff_value, dict)
        handoff_value["schema_version"] = True
        artifacts["scorecard-handoff"].write_text(
            json.dumps(handoff_value, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        rebind_validation_removal_artifact(
            project,
            manifest,
            "scorecard-handoff",
            artifacts["scorecard-handoff"],
        )
    else:
        raise AssertionError(mutation)

    stub_validate_batch_censuses(monkeypatch)
    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "remove",
            "slot01",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--expected-generation",
            "1",
            "--validation-proof-manifest",
            manifest.relative_to(project).as_posix(),
            "--completed-record",
            artifacts["run-record"].relative_to(project).as_posix(),
        ]
    )

    assert rc == 3
    assert message in capsys.readouterr().err
    assert slot_path.is_dir()
    assert active_slots(project)


def test_validation_service_result_projection_refuses_passed_without_counts() -> None:
    payload = json.dumps(
        {
            "schema_version": 4,
            "commit": "a" * 40,
            "profile": "full",
            "selection_mode": "full",
            "final_validate_status": "PASSED",
            "exit_code": 0,
            "executed_nodes": 1,
            "executed_tests": None,
            "passed_tests": None,
            "scorecard_writeback": {"status": "completed"},
        }
    ).encode()

    with pytest.raises(wrkslots.Refusal, match="PASSED counts disagree"):
        wrkslots._validation_service_result_projection(
            payload,
            schema=4,
            schema_fields=wrkslots._VALIDATION_SERVICE_RESULT_FIELDS[4],
            expected_commit="a" * 40,
        )


@pytest.mark.parametrize("mutation", ("extra", "missing", "wrong-type"))
def test_validate_remove_refuses_service_result_field_drift(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    mutation: str,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    commit_validation_removal_schema(repository, schema_version=5)
    slot_path = prepare_dead_validate_slots(project, ("slot01",))["slot01"]
    manifest, artifacts = prepare_validation_removal_proof(project, "slot01")
    result_value: object = json.loads(
        artifacts["service-result"].read_text(encoding="utf-8")
    )
    assert isinstance(result_value, dict)
    if mutation == "extra":
        result_value["undeclared"] = "field"
    elif mutation == "missing":
        del result_value["detail"]
    elif mutation == "wrong-type":
        result_value["schema_version"] = 5.0
    else:
        raise AssertionError(mutation)
    artifacts["service-result"].write_text(
        json.dumps(result_value, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    rebind_validation_removal_artifact(
        project, manifest, "service-result", artifacts["service-result"]
    )
    stub_validate_batch_censuses(monkeypatch)

    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "remove",
            "slot01",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--expected-generation",
            "1",
            "--validation-proof-manifest",
            manifest.relative_to(project).as_posix(),
            "--completed-record",
            artifacts["run-record"].relative_to(project).as_posix(),
        ]
    )

    assert rc == 3
    error = capsys.readouterr().err
    assert (
        "service-result fields" in error
        if mutation != "wrong-type"
        else "service-result schema differs" in error
    )
    assert slot_path.is_dir()
    assert active_slots(project)


@pytest.mark.parametrize("disagreement", ("count", "duplicate"))
def test_validate_batch_refuses_proof_list_disagreement_before_sealing(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    disagreement: str,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    commit_validation_removal_schema(repository)
    slot_paths = prepare_dead_validate_slots(project, ("slot01", "slot02"))
    prepared = [
        prepare_validation_removal_proof(project, slot)
        for slot in ("slot01", "slot02")
    ]
    manifests = [item[0] for item in prepared]
    records = [item[1]["run-record"] for item in prepared]
    supplied = [manifests[0]] if disagreement == "count" else [manifests[0]] * 2
    monkeypatch.setattr(
        wrkslots,
        "_capture_process_path_census",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(
            AssertionError("batch disagreement must refuse before census")
        ),
    )

    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "remove-validate-batch",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--slot",
            "slot01=1",
            "--slot",
            "slot02=1",
            *[
                value
                for manifest in supplied
                for value in (
                    "--validation-proof-manifest",
                    manifest.relative_to(project).as_posix(),
                )
            ],
            *[
                value
                for record in records
                for value in (
                    "--completed-record",
                    record.relative_to(project).as_posix(),
                )
            ],
            "--format",
            "json",
        ]
    )

    assert rc == 3
    assert disagreement in (
        "count" if "requires one" in capsys.readouterr().err else "duplicate"
    )
    assert all(path.is_dir() for path in slot_paths.values())
    assert all(stat.S_IMODE(path.stat().st_mode) != 0o700 for path in slot_paths.values())


def test_validate_removal_proof_survives_registered_finish_recovery(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    commit_validation_removal_schema(repository)
    slot_path = prepare_dead_validate_slots(project, ("slot01",))["slot01"]
    manifest, artifacts = prepare_validation_removal_proof(project, "slot01")
    arguments = (
        "remove",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        "--validation-proof-manifest",
        manifest.relative_to(project).as_posix(),
        "--completed-record",
        artifacts["run-record"].relative_to(project).as_posix(),
    )

    interrupted = raw_command_with_census_authority_stub(
        project,
        *arguments,
        env={"WRKSLOTS_TEST_INTERRUPT": "after-validation-removal-proof-recheck"},
    )
    assert interrupted.returncode == 86
    assert (control_directory(project) / "ACTIVE.testhost.journal").is_file()

    recovered = raw_command_with_census_authority_stub(
        project, "recover", "--coordinator-pid", str(os.getpid())
    )

    assert recovered.returncode == 0, recovered.stderr
    assert not slot_path.exists()
    assert active_slots(project) == []


@pytest.mark.parametrize(
    ("mutation", "message"),
    (
        ("changed-bytes", "changed bytes"),
        ("changed-path", "regular-file identity changed"),
        ("missing", "cannot read"),
        pytest.param(
            "unreadable",
            "cannot read",
            marks=pytest.mark.ordinary_environment,
        ),
        ("symlink", "crosses a symlink"),
        ("nonregular", "not a regular file"),
        ("stale-manifest", "regular-file identity changed"),
        ("retained-file-bytes", "retained scorecard file changed bytes"),
        ("retained-extra-file", "invalid file set"),
        ("retained-extra-symlink", "invalid file set"),
    ),
)
def test_validate_batch_rechecks_external_proof_after_private_seal(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    mutation: str,
    message: str,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    commit_validation_removal_schema(repository)
    slot_path = prepare_dead_validate_slots(project, ("slot01",))["slot01"]
    manifest, artifacts = prepare_validation_removal_proof(project, "slot01")

    def replace_with_same_bytes_and_distinct_identity(path: Path) -> None:
        contents = path.read_bytes()
        original = path.stat(follow_symlinks=False)
        replacement = path.with_name(f".{path.name}.replacement")
        assert not replacement.exists()
        replacement.write_bytes(contents)
        replacement_identity = replacement.stat(follow_symlinks=False)
        assert (replacement_identity.st_dev, replacement_identity.st_ino) != (
            original.st_dev,
            original.st_ino,
        )
        os.replace(replacement, path)
        rebound = path.stat(follow_symlinks=False)
        assert (rebound.st_dev, rebound.st_ino) == (
            replacement_identity.st_dev,
            replacement_identity.st_ino,
        )
        assert path.read_bytes() == contents

    def shared(
        _paths: Sequence[Path],
        *,
        budget: wrkslots._ReadOnlyCommandBudget,
        include_owner_cgroups: bool = True,
    ) -> wrkslots._ProcessPathCensus:
        del budget, include_owner_cgroups
        return wrkslots._ProcessPathCensus((), ())

    def mutate_at_final_boundary(
        _paths: Sequence[Path], *, budget: wrkslots._ReadOnlyCommandBudget
    ) -> wrkslots._ProcessPathCensus:
        del budget
        run_record = artifacts["run-record"]
        if mutation == "changed-bytes":
            service_result = artifacts["service-result"]
            contents = service_result.read_bytes()
            assert b'"profile": "full"' in contents
            service_result.write_bytes(
                contents.replace(b'"profile": "full"', b'"profile": "evil"', 1)
            )
        elif mutation == "changed-path":
            replace_with_same_bytes_and_distinct_identity(run_record)
        elif mutation == "missing":
            run_record.unlink()
        elif mutation == "unreadable":
            run_record.chmod(0)
        elif mutation == "symlink":
            replacement = run_record.with_name("replacement-run-record.json")
            replacement.write_bytes(run_record.read_bytes())
            run_record.unlink()
            run_record.symlink_to(replacement)
        elif mutation == "nonregular":
            run_record.unlink()
            run_record.mkdir()
        elif mutation == "stale-manifest":
            replace_with_same_bytes_and_distinct_identity(manifest)
        elif mutation == "retained-file-bytes":
            scorecard = artifacts["scorecard-handoff"].parent / "SCORECARD.md"
            contents = scorecard.read_bytes()
            scorecard.write_bytes(b"!" + contents[1:])
        elif mutation == "retained-extra-file":
            (artifacts["scorecard-handoff"].parent / "unexpected.txt").write_text(
                "unexpected\n", encoding="utf-8"
            )
        elif mutation == "retained-extra-symlink":
            (artifacts["scorecard-handoff"].parent / "unexpected-link").symlink_to(
                "SCORECARD.md"
            )
        else:
            raise AssertionError(mutation)
        return wrkslots._ProcessPathCensus((), ())

    monkeypatch.setattr(wrkslots, "_capture_process_path_census", shared)
    monkeypatch.setattr(
        wrkslots, "_capture_same_uid_process_path_census", mutate_at_final_boundary
    )
    args = argparse.Namespace(
        project_root=str(project),
        machine=None,
        wait_lock=0,
        command="remove-validate-batch",
        coordinator_authorized=True,
        coordinator_pid=os.getpid(),
        slots=["slot01=1"],
        validation_proof_manifests=[manifest.relative_to(project).as_posix()],
        completed_records=[
            artifacts["run-record"].relative_to(project).as_posix()
        ],
        format="json",
    )

    assert wrkslots._cmd_remove_validate_batch(args) == 0
    report = json.loads(capsys.readouterr().out)
    assert report["removed"] == []
    assert len(report["retained"]) == 1
    assert message in report["retained"][0]["reason"]
    assert slot_path.is_dir()
    assert not (control_directory(project) / "ACTIVE.testhost.journal").exists()
    assert active_slots(project)


def test_validate_batch_shares_one_census_and_retains_each_refusal(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    commit_validation_removal_schema(repository)
    slot_paths = prepare_dead_validate_slots(project, ("unused", "in-use"))
    proofs = {
        slot: prepare_validation_removal_proof(project, slot)
        for slot in ("unused", "in-use")
    }
    in_use = slot_paths["in-use"]
    calls: list[tuple[Path, ...]] = []
    census_order: list[str] = []

    def census(
        paths: Sequence[Path],
        *,
        budget: wrkslots._ReadOnlyCommandBudget | None = None,
        include_owner_cgroups: bool = True,
    ) -> wrkslots._ProcessPathCensus:
        assert budget is not None
        assert include_owner_cgroups is False
        census_order.append("privileged")
        calls.append(tuple(paths))
        assert set(paths) == {
            slot_paths["unused"],
            in_use,
        }
        assert all(stat.S_IMODE(path.stat().st_mode) == 0o700 for path in paths)
        return wrkslots._ProcessPathCensus(
            (), ((12345, str(in_use), "link", str(in_use)),)
        )

    monkeypatch.setattr(wrkslots, "_capture_process_path_census", census)
    monkeypatch.setattr(
        wrkslots,
        "_capture_same_uid_process_path_census",
        lambda _paths, **_kwargs: wrkslots._ProcessPathCensus((), ()),
    )

    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "remove-validate-batch",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--slot",
            "in-use=1",
            "--slot",
            "unused=1",
            "--validation-proof-manifest",
            proofs["in-use"][0].relative_to(project).as_posix(),
            "--validation-proof-manifest",
            proofs["unused"][0].relative_to(project).as_posix(),
            "--completed-record",
            proofs["in-use"][1]["run-record"].relative_to(project).as_posix(),
            "--completed-record",
            proofs["unused"][1]["run-record"].relative_to(project).as_posix(),
            "--format",
            "json",
        ]
    )

    assert rc == 0
    report = json.loads(capsys.readouterr().out)
    assert report["process_censuses"] == 1
    assert report["shared_process_censuses"] == 1
    assert report["same_uid_process_censuses"] == 1
    assert report["removed"] == [{"generation": 1, "slot": "unused"}]
    assert len(report["retained"]) == 1
    assert report["retained"][0]["slot"] == "in-use"
    assert "live process 12345" in report["retained"][0]["reason"]
    assert len(calls) == 1
    assert census_order == ["privileged"]
    assert not slot_paths["unused"].exists()
    assert in_use.is_dir()


def test_validate_batch_removes_nested_git_metadata_in_configured_cache(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, _repository, _remote = make_project(tmp_path, cache_globs=("target",))
    prepare_dead_validate_slots(project, ("nested-cache",))
    tree = checkout(project, slot="nested-cache", slot_type="validate")
    nested_git = tree / "target" / "install-build" / "dependency" / ".git"
    nested_git.mkdir(parents=True)
    (nested_git / "HEAD").write_text("ref: refs/heads/main\n", encoding="utf-8")
    stub_validate_batch_censuses(monkeypatch)

    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "remove-validate-batch",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--slot",
            "nested-cache=1",
            "--format",
            "json",
        ]
    )

    assert rc == 0
    report = json.loads(capsys.readouterr().out)
    assert report == {
        "batch_limit": wrkslots.VALIDATE_REMOVE_BATCH_LIMIT,
        "process_censuses": 1,
        "removed": [{"generation": 1, "slot": "nested-cache"}],
        "requested": 1,
        "retained": [],
        "same_uid_process_censuses": 1,
        "schema": 1,
        "shared_process_censuses": 1,
    }
    assert not tree.exists()
    assert active_slots(project) == []


def test_validate_batch_is_bounded_before_process_census(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    called = False

    def census(_paths: list[Path]) -> wrkslots._ProcessPathCensus:
        nonlocal called
        called = True
        return wrkslots._ProcessPathCensus((), ())

    monkeypatch.setattr(wrkslots, "_capture_process_path_census", census)
    args = [
        "--project-root",
        str(project),
        "remove-validate-batch",
        "--coordinator-pid",
        str(os.getpid()),
    ]
    for index in range(wrkslots.VALIDATE_REMOVE_BATCH_LIMIT + 1):
        args.extend(("--slot", f"slot-{index}=1"))

    assert wrkslots.main(args) == 3
    assert "accepts at most" in capsys.readouterr().err
    assert called is False


def test_validate_batch_all_ineligible_reports_zero_censuses(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    monkeypatch.setattr(
        wrkslots,
        "_capture_process_path_census",
        lambda _paths, **_kwargs: pytest.fail(
            "all-ineligible batch ran a privileged census"
        ),
    )
    assert (
        wrkslots.main(
            [
                "--project-root",
                str(project),
                "remove-validate-batch",
                "--coordinator-pid",
                str(os.getpid()),
                "--slot",
                "missing=1",
                "--format",
                "json",
            ]
        )
        == 0
    )

    report = json.loads(capsys.readouterr().out)
    assert report["process_censuses"] == 0
    assert report["shared_process_censuses"] == 0
    assert report["same_uid_process_censuses"] == 0
    assert report["removed"] == []
    assert len(report["retained"]) == 1
    config = wrkslots._load_config(str(project), "testhost")
    assert not wrkslots._validate_batch_seal_journal_path(config).exists()


@pytest.mark.parametrize("expiry_point", ("shared", "fresh"))
def test_validate_batch_census_deadline_restores_seals_before_outer_bound(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    expiry_point: str,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    slots = tuple(
        f"slot{index:02d}" for index in range(1, wrkslots.VALIDATE_REMOVE_BATCH_LIMIT + 1)
    )
    slot_paths = prepare_dead_validate_slots(project, slots)
    original_modes = {
        slot: stat.S_IMODE(path.stat().st_mode) for slot, path in slot_paths.items()
    }
    monotonic_now = 100.0

    def monotonic() -> float:
        return monotonic_now

    def exhaust_after_shared_census(
        _paths: Sequence[Path],
        *,
        budget: wrkslots._ReadOnlyCommandBudget | None = None,
        include_owner_cgroups: bool = True,
    ) -> wrkslots._ProcessPathCensus:
        nonlocal monotonic_now
        assert budget is not None
        assert include_owner_cgroups is False
        assert budget.deadline == pytest.approx(
            100.0 + wrkslots._VALIDATE_REMOVE_BATCH_CENSUS_SECONDS
        )
        if expiry_point == "shared":
            monotonic_now = budget.deadline + 1.0
        return wrkslots._ProcessPathCensus((), ())

    def exhaust_during_fresh_census(
        _paths: Sequence[Path], *, budget: wrkslots._ReadOnlyCommandBudget
    ) -> wrkslots._ProcessPathCensus:
        nonlocal monotonic_now
        monotonic_now = budget.deadline + 1.0
        budget.remaining_seconds()
        raise AssertionError("unreachable")

    monkeypatch.setattr(time, "monotonic", monotonic)
    monkeypatch.setattr(
        wrkslots, "_capture_process_path_census", exhaust_after_shared_census
    )
    monkeypatch.setattr(
        wrkslots,
        "_capture_same_uid_process_path_census",
        exhaust_during_fresh_census,
    )
    arguments = [
        "--project-root",
        str(project),
        "remove-validate-batch",
        "--coordinator-pid",
        str(os.getpid()),
    ]
    for slot in slots:
        arguments.extend(("--slot", f"{slot}=1"))
    arguments.extend(("--format", "json"))
    started = time.perf_counter()
    assert wrkslots.main(arguments) == 0
    rollback_seconds = time.perf_counter() - started

    report = json.loads(capsys.readouterr().out)
    assert report["removed"] == []
    assert report["same_uid_process_censuses"] == int(expiry_point == "fresh")
    assert len(report["retained"]) == wrkslots.VALIDATE_REMOVE_BATCH_LIMIT
    assert all(
        "operation-wide time bound" in row["reason"] for row in report["retained"]
    )
    rollback_reserve = 30.0 - wrkslots._VALIDATE_REMOVE_BATCH_CENSUS_SECONDS
    assert rollback_reserve == 8.0
    assert rollback_seconds < rollback_reserve
    assert all(path.is_dir() for path in slot_paths.values())
    assert {
        slot: stat.S_IMODE(path.stat().st_mode) for slot, path in slot_paths.items()
    } == original_modes
    config = wrkslots._load_config(str(project), "testhost")
    assert not wrkslots._journal_path(config).exists()
    assert not wrkslots._validate_batch_seal_journal_path(config).exists()
    assert not any(
        list(path.parent.glob(f".{slot}.fenced.*"))
        for slot, path in slot_paths.items()
    )


def test_validate_batch_deadline_between_targets_never_enters_later_removals(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    slots = tuple(
        f"slot{index:02d}" for index in range(1, wrkslots.VALIDATE_REMOVE_BATCH_LIMIT + 1)
    )
    slot_paths = prepare_dead_validate_slots(project, slots)
    original_modes = {
        slot: stat.S_IMODE(path.stat().st_mode) for slot, path in slot_paths.items()
    }
    monotonic_now = 100.0
    entered: list[str] = []
    fresh_scans: list[tuple[Path, ...]] = []
    fenced: list[str] = []
    original_remove = wrkslots._cmd_remove

    monkeypatch.setattr(time, "monotonic", lambda: monotonic_now)
    monkeypatch.setattr(
        wrkslots,
        "_capture_process_path_census",
        lambda _paths, **_kwargs: wrkslots._ProcessPathCensus((), ()),
    )

    def fresh_census(
        paths: Sequence[Path], *, budget: wrkslots._ReadOnlyCommandBudget
    ) -> wrkslots._ProcessPathCensus:
        del budget
        fresh_scans.append(tuple(paths))
        return wrkslots._ProcessPathCensus((), ())

    def remove_then_expire(
        args: argparse.Namespace,
        *,
        private_cleanup: wrkslots._PrivateCleanupContext | None = None,
        validation_removal_proof: wrkslots._ValidationRemovalProof | None = None,
        emit: bool = True,
    ) -> int:
        nonlocal monotonic_now
        assert validation_removal_proof is None
        entered.append(args.slot)
        result = original_remove(
            args,
            private_cleanup=private_cleanup,
            validation_removal_proof=validation_removal_proof,
            emit=emit,
        )
        assert private_cleanup is not None
        monotonic_now = private_cleanup.census_budget.deadline + 1.0
        return result

    def record_fence(point: str) -> None:
        if point == "after-path-fence":
            fenced.append(entered[-1])

    monkeypatch.setattr(wrkslots, "_capture_same_uid_process_path_census", fresh_census)
    monkeypatch.setattr(wrkslots, "_cmd_remove", remove_then_expire)
    monkeypatch.setattr(wrkslots, "_interrupt_for_test", record_fence)
    arguments = [
        "--project-root",
        str(project),
        "remove-validate-batch",
        "--coordinator-pid",
        str(os.getpid()),
    ]
    for slot in slots:
        arguments.extend(("--slot", f"{slot}=1"))
    arguments.extend(("--format", "json"))

    assert wrkslots.main(arguments) == 0

    report = json.loads(capsys.readouterr().out)
    assert entered == [slots[0]]
    assert fenced == [slots[0]]
    assert len(fresh_scans) == 1
    assert report["removed"] == [{"generation": 1, "slot": slots[0]}]
    assert {row["slot"] for row in report["retained"]} == set(slots[1:])
    assert all(
        "operation-wide time bound" in row["reason"] for row in report["retained"]
    )
    assert report["same_uid_process_censuses"] == 1
    assert not slot_paths[slots[0]].exists()
    assert all(slot_paths[slot].is_dir() for slot in slots[1:])
    assert {
        slot: stat.S_IMODE(slot_paths[slot].stat().st_mode) for slot in slots[1:]
    } == {slot: original_modes[slot] for slot in slots[1:]}
    config = wrkslots._load_config(str(project), "testhost")
    assert not wrkslots._journal_path(config).exists()
    assert not wrkslots._validate_batch_seal_journal_path(config).exists()
    assert not any(
        list(path.parent.glob(f".{slot}.fenced.*"))
        for slot, path in slot_paths.items()
    )


def test_validate_batch_shared_census_explicitly_omits_owner_cgroups(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    prepare_dead_validate_slots(project, ("slot01",))
    observed_owner_cgroup_flags: list[bool] = []

    def shared_census(
        _paths: Sequence[Path],
        *,
        budget: wrkslots._ReadOnlyCommandBudget | None = None,
        include_owner_cgroups: bool = True,
    ) -> wrkslots._ProcessPathCensus:
        del budget
        observed_owner_cgroup_flags.append(include_owner_cgroups)
        return wrkslots._ProcessPathCensus(
            (), (), owner_cgroup_complete=include_owner_cgroups
        )

    monkeypatch.setattr(wrkslots, "_capture_process_path_census", shared_census)
    monkeypatch.setattr(
        wrkslots,
        "_capture_same_uid_process_path_census",
        lambda _paths, **_kwargs: wrkslots._ProcessPathCensus(
            (), (), owner_cgroup_complete=False
        ),
    )

    assert (
        wrkslots.main(
            [
                "--project-root",
                str(project),
                "remove-validate-batch",
                "--coordinator-pid",
                str(os.getpid()),
                "--slot",
                "slot01=1",
                "--format",
                "json",
            ]
        )
        == 0
    )

    report = json.loads(capsys.readouterr().out)
    assert observed_owner_cgroup_flags == [False]
    assert report["removed"] == [{"generation": 1, "slot": "slot01"}]


def test_incomplete_owner_cgroup_census_refuses_only_when_it_will_compare(
    tmp_path: Path,
) -> None:
    """Missing evidence is a reason to refuse only for evidence still consulted.

    ⚠️ THIS ASSERTION WAS NARROWED, and it used to be
    test_incomplete_owner_cgroup_census_refuses_owner_check, which refused for
    the dead-owner case below. That became wrong when the owner-cgroup
    comparison stopped being made for a proven-dead owner: the census was then
    refusing for the absence of evidence it was no longer going to look at.

    The live-owner half is the control, and it is the reason this is a
    narrowing rather than a deletion -- where the comparison will still be
    made, an incomplete census still refuses.
    """
    project, _repository, _remote = make_project(tmp_path)
    prepare_dead_validate_slots(project, ("slot01",))
    config = wrkslots._load_config(str(project), "testhost")
    record = wrkslots._find_record(wrkslots._load_active(config), "slot01")
    assert record.owner is not None
    census = wrkslots._ProcessPathCensus(
        (), (), owner_cgroup_complete=False
    )
    slot = wrkslots._slot_directory(config, "slot01", "validate")

    census.assert_slot_unused(slot, record)

    live = replace(record, owner=wrkslots._read_process_identity(os.getpid()))
    with pytest.raises(wrkslots.Refusal, match="omitted owner-cgroup evidence"):
        census.assert_slot_unused(slot, live)


def test_process_census_ignores_only_the_invoking_process_ancestry(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    target = tmp_path / "slot"
    current = os.getpid()
    parent = current + 100_000
    unrelated = current + 200_000
    monkeypatch.setattr(
        wrkslots,
        "_read_process_parent",
        lambda pid: parent if pid == current else None,
    )
    parent_census = wrkslots._ProcessPathCensus(
        (), ((parent, str(target), "cwd", str(target)),)
    )

    parent_census.assert_slot_unused(
        target, None, ignore_invoking_ancestry=True
    )
    with pytest.raises(wrkslots.Refusal, match=f"live process {parent}"):
        parent_census.assert_slot_unused(
            target, None, ignore_invoking_ancestry=False
        )

    unrelated_census = wrkslots._ProcessPathCensus(
        (), ((unrelated, str(target), "cwd", str(target)),)
    )
    with pytest.raises(wrkslots.Refusal, match=f"live process {unrelated}"):
        unrelated_census.assert_slot_unused(
            target, None, ignore_invoking_ancestry=True
        )


def test_validate_batch_rechecks_later_slot_use_after_shared_census(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    paths = prepare_dead_validate_slots(project, ("first", "late-use"))
    tree = paths["late-use"] / "product"
    monkeypatch.setattr(
        wrkslots,
        "_capture_process_path_census",
        lambda _paths, **_kwargs: wrkslots._ProcessPathCensus((), ()),
    )
    real_rename = os.rename
    late_user: subprocess.Popen[str] | None = None
    fenced_scans = 0

    def record_slot_scan(
        slot_paths: Sequence[Path], *, budget: wrkslots._ReadOnlyCommandBudget
    ) -> wrkslots._ProcessPathCensus:
        nonlocal fenced_scans
        del budget
        assert len(slot_paths) == 1
        slot_path = slot_paths[0]
        if slot_path.name.startswith(".") and ".fenced." in slot_path.name:
            fenced_scans += 1
        if late_user is not None:
            observed = Path(os.readlink(f"/proc/{late_user.pid}/cwd"))
            if observed == slot_path or slot_path in observed.parents:
                return wrkslots._ProcessPathCensus(
                    (), ((late_user.pid, str(slot_path), "cwd", str(observed)),)
                )
        return wrkslots._ProcessPathCensus((), ())

    def enter_at_final_path_fence(source: os.PathLike[str], target: os.PathLike[str]) -> None:
        nonlocal late_user
        if Path(source) == paths["late-use"]:
            late_user = subprocess.Popen(["sleep", "60"], cwd=tree, text=True)
        real_rename(source, target)

    monkeypatch.setattr(
        wrkslots, "_capture_same_uid_process_path_census", record_slot_scan
    )
    monkeypatch.setattr("wrkslots.cli.os.rename", enter_at_final_path_fence)
    try:
        rc = wrkslots.main(
            [
                "--project-root",
                str(project),
                "remove-validate-batch",
                "--coordinator-pid",
                str(os.getpid()),
                "--slot",
                "first=1",
                "--slot",
                "late-use=1",
                "--format",
                "json",
            ]
        )
    finally:
        if late_user is not None:
            terminate_process(late_user)

    assert rc == 0
    report = json.loads(capsys.readouterr().out)
    assert report["removed"] == [{"generation": 1, "slot": "first"}], json.dumps(
        report, sort_keys=True
    )
    assert len(report["retained"]) == 1
    assert "live process" in report["retained"][0]["reason"]
    assert fenced_scans == 2
    assert not paths["first"].exists()
    assert tree.is_dir()


@pytest.mark.parametrize("foreign_process_preexisted", (False, True))
def test_validate_batch_fresh_privileged_census_refuses_foreign_uid_late_use(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    foreign_process_preexisted: bool,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project, slot="late-foreign", slot_type="validate", branch=None)
    assert made.returncode == 0, made.stderr
    mark_owner_dead(project)
    set_liveness(project, "dead")
    tree = checkout(project, slot="late-foreign", slot_type="validate").parent
    original_mode = stat.S_IMODE(tree.stat().st_mode)
    foreign = wrkslots._AbsentProcessObservation(
        54321, 91, "/foreign", "mnt:[54321]"
    )
    calls: list[tuple[Path, ...]] = []
    foreign_exists = foreign_process_preexisted
    foreign_uses_target = False
    ordering: list[str] = ["foreign-exists"] if foreign_exists else []
    original_seal = wrkslots._seal_cleanup_path_private

    def seal(
        config: wrkslots.Config,
        path: Path,
        *,
        before_seal: Callable[[int, wrkslots._PrivateCleanupIdentity], None]
        | None = None,
    ) -> tuple[int, wrkslots._PrivateCleanupIdentity]:
        nonlocal foreign_exists, foreign_uses_target
        if foreign_process_preexisted:
            foreign_uses_target = True
            ordering.append("foreign-transitioned-before-seal")
        result = original_seal(config, path, before_seal=before_seal)
        ordering.append("sealed")
        if not foreign_process_preexisted:
            foreign_exists = True
            foreign_uses_target = True
            ordering.append("foreign-spawned-after-seal")
        return result

    def census(
        paths: Sequence[Path],
        *,
        budget: wrkslots._ReadOnlyCommandBudget | None = None,
        include_owner_cgroups: bool = True,
    ) -> wrkslots._ProcessPathCensus:
        assert budget is not None
        assert include_owner_cgroups is False
        calls.append(tuple(paths))
        assert foreign_exists
        assert foreign_uses_target
        ordering.append("privileged-census")
        assert paths == [tree]
        assert stat.S_IMODE(tree.stat().st_mode) == 0o700
        return wrkslots._ProcessPathCensus(
            (foreign,), ((foreign.pid, str(tree), "link", str(tree)),)
        )

    monkeypatch.setattr(wrkslots, "_seal_cleanup_path_private", seal)
    monkeypatch.setattr(wrkslots, "_capture_process_path_census", census)
    monkeypatch.setattr(
        wrkslots,
        "_capture_same_uid_process_path_census",
        lambda _paths, **_kwargs: wrkslots._ProcessPathCensus((), ()),
    )

    assert (
        wrkslots.main(
            [
                "--project-root",
                str(project),
                "remove-validate-batch",
                "--coordinator-pid",
                str(os.getpid()),
                "--slot",
                "late-foreign=1",
                "--format",
                "json",
            ]
        )
        == 0
    )

    report = json.loads(capsys.readouterr().out)
    assert report["removed"] == []
    assert report["process_censuses"] == 1
    assert len(report["retained"]) == 1
    assert "live process 54321" in report["retained"][0]["reason"]
    assert tree.is_dir()
    assert stat.S_IMODE(tree.stat().st_mode) == original_mode
    assert not list(tree.parent.glob(".late-foreign.fenced.*"))
    assert any(
        isinstance(row, dict) and row.get("slot") == "late-foreign"
        for row in active_slots(project)
    )
    expected_prefix = (
        ["foreign-exists", "foreign-transitioned-before-seal", "sealed"]
        if foreign_process_preexisted
        else ["sealed", "foreign-spawned-after-seal"]
    )
    assert ordering == [*expected_prefix, "privileged-census"]


def test_validate_batch_refuses_untrusted_cleanup_fence_root(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project, slot="untrusted-root", slot_type="validate", branch=None)
    assert made.returncode == 0, made.stderr
    mark_owner_dead(project)
    set_liveness(project, "dead")
    tree = checkout(project, slot="untrusted-root", slot_type="validate").parent
    tree.parent.chmod(0o777)
    monkeypatch.setattr(
        wrkslots,
        "_capture_process_path_census",
        lambda _paths, **_kwargs: wrkslots._ProcessPathCensus((), ()),
    )

    assert (
        wrkslots.main(
            [
                "--project-root",
                str(project),
                "remove-validate-batch",
                "--coordinator-pid",
                str(os.getpid()),
                "--slot",
                "untrusted-root=1",
                "--format",
                "json",
            ]
        )
        == 0
    )

    report = json.loads(capsys.readouterr().out)
    assert report["removed"] == []
    assert "not an owner-controlled directory" in report["retained"][0]["reason"]
    assert tree.is_dir()
    assert not list(tree.parent.glob(".untrusted-root.fenced.*"))


def test_validate_batch_seal_failure_restores_original_mode_through_fd(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project, slot="seal-failure", slot_type="validate", branch=None)
    assert made.returncode == 0, made.stderr
    config = wrkslots._load_config(str(project), "testhost")
    slot_path = checkout(project, slot="seal-failure", slot_type="validate").parent
    original_mode = stat.S_IMODE(slot_path.stat().st_mode)

    real_fstat = os.fstat
    fstat_calls = 0

    def fail_after_chmod(descriptor: int) -> os.stat_result:
        nonlocal fstat_calls
        fstat_calls += 1
        if fstat_calls == 2:
            raise OSError("injected post-fchmod failure")
        return real_fstat(descriptor)

    monkeypatch.setattr(os, "fstat", fail_after_chmod)

    with pytest.raises(wrkslots.Refusal, match="injected post-fchmod failure"):
        wrkslots._seal_cleanup_path_private(config, slot_path)

    assert stat.S_IMODE(slot_path.stat().st_mode) == original_mode


def test_validate_batch_unexpected_preflight_failure_restores_prior_targets(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    paths = prepare_dead_validate_slots(project, ("first", "second"))
    first_path = paths["first"]
    original_mode = stat.S_IMODE(first_path.stat().st_mode)
    real_seal = wrkslots._seal_cleanup_path_private
    calls = 0

    def fail_after_first_seal(
        batch_config: wrkslots.Config,
        path: Path,
        *,
        before_seal: Callable[[int, wrkslots._PrivateCleanupIdentity], None]
        | None = None,
    ) -> tuple[int, wrkslots._PrivateCleanupIdentity]:
        nonlocal calls
        calls += 1
        if calls == 2:
            raise RuntimeError("injected preflight failure")
        return real_seal(batch_config, path, before_seal=before_seal)

    monkeypatch.setattr(wrkslots, "_seal_cleanup_path_private", fail_after_first_seal)

    with pytest.raises(RuntimeError, match="injected preflight failure"):
        wrkslots.main(
            [
                "--project-root",
                str(project),
                "remove-validate-batch",
                "--coordinator-pid",
                str(os.getpid()),
                "--slot",
                "first=1",
                "--slot",
                "second=1",
            ]
        )

    assert stat.S_IMODE(first_path.stat().st_mode) == original_mode


def test_private_cleanup_identity_is_stable_across_mount_namespace_ids(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    config = wrkslots._load_config(str(project), "testhost")
    target = project / "target"
    target.mkdir()
    target.chmod(0o700)
    mount_ids = iter((865, 3503))
    monkeypatch.setattr(wrkslots, "_fd_mount_id", lambda _fd, _label: next(mount_ids))

    mount_scoped_before = wrkslots._open_directory_identity(target, "target")
    stable_before = wrkslots._private_cleanup_fence_identity(config, target)
    mount_scoped_after = wrkslots._open_directory_identity(target, "target")
    stable_after = wrkslots._private_cleanup_fence_identity(config, target)

    assert mount_scoped_before[:2] == mount_scoped_after[:2]
    assert mount_scoped_before[2:] == (865,)
    assert mount_scoped_after[2:] == (3503,)
    assert stable_before == stable_after


def test_private_cleanup_identity_refuses_changed_file_handle_with_same_inode(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    config = wrkslots._load_config(str(project), "testhost")
    target = project / "target"
    target.mkdir()
    target.chmod(0o700)
    expected = wrkslots._private_cleanup_fence_identity(config, target)
    handle_type, file_handle = expected[2:]
    replacement = "00" * (len(file_handle) // 2)
    if replacement == file_handle:
        replacement = "11" * (len(file_handle) // 2)
    monkeypatch.setattr(
        wrkslots,
        "_fd_file_handle",
        lambda _fd, _label: (handle_type, replacement),
    )

    with pytest.raises(wrkslots.Refusal, match="identity changed"):
        wrkslots._restore_private_cleanup_path(
            target, stat.S_IMODE(target.stat().st_mode), expected, allow_missing=False
        )


def test_finish_recovery_upgrades_legacy_mount_namespace_identity(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    slot_path = prepare_dead_validate_slots(project, ("slot01",))["slot01"]
    stub_validate_batch_censuses(monkeypatch)
    interrupt_validate_batch(project, monkeypatch, "after-finish-journal", ("slot01",))
    config = wrkslots._load_config(str(project), "testhost")
    finish_path = wrkslots._journal_path(config)
    seal_path = wrkslots._validate_batch_seal_journal_path(config)
    finish = json.loads(finish_path.read_text(encoding="utf-8"))
    seal = json.loads(seal_path.read_text(encoding="utf-8"))
    metadata = slot_path.stat()
    legacy_identity = [metadata.st_dev, metadata.st_ino, 865]
    finish["private_census_identity"] = legacy_identity
    seal["targets"][0]["identity"] = legacy_identity
    finish_path.write_text(json.dumps(finish, indent=2) + "\n", encoding="utf-8")
    seal_path.write_text(json.dumps(seal, indent=2) + "\n", encoding="utf-8")
    shutil.rmtree(control_directory(project) / "EVENTS.testhost")
    monkeypatch.setattr(wrkslots, "_fd_mount_id", lambda _fd, _label: 3503)

    assert (
        wrkslots.main(
            [
                "--project-root",
                str(project),
                "recover",
                "--coordinator-pid",
                str(os.getpid()),
            ]
        )
        == 0
    )
    assert not slot_path.exists()
    assert not finish_path.exists()
    assert not seal_path.exists()


def test_destructive_legacy_identity_upgrade_refuses_dirty_checkout(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    slot_path = prepare_dead_validate_slots(project, ("slot01",))["slot01"]
    stub_validate_batch_censuses(monkeypatch)
    interrupt_validate_batch(project, monkeypatch, "after-finish-journal", ("slot01",))
    config = wrkslots._load_config(str(project), "testhost")
    finish_path = wrkslots._journal_path(config)
    seal_path = wrkslots._validate_batch_seal_journal_path(config)
    finish = json.loads(finish_path.read_text(encoding="utf-8"))
    seal = json.loads(seal_path.read_text(encoding="utf-8"))
    metadata = slot_path.stat()
    legacy_identity = [metadata.st_dev, metadata.st_ino, 865]
    finish["private_census_identity"] = legacy_identity
    seal["targets"][0]["identity"] = legacy_identity
    finish_path.write_text(json.dumps(finish, indent=2) + "\n", encoding="utf-8")
    seal_path.write_text(json.dumps(seal, indent=2) + "\n", encoding="utf-8")
    shutil.rmtree(control_directory(project) / "EVENTS.testhost")
    finish_before = finish_path.read_bytes()
    seal_before = seal_path.read_bytes()
    retained = slot_path / "product" / "seed.txt"
    retained.write_text("dirty retained output\n", encoding="utf-8")

    assert (
        wrkslots.main(
            [
                "--project-root",
                str(project),
                "recover",
                "--coordinator-pid",
                str(os.getpid()),
            ]
        )
        == 3
    )
    assert "dirty or has untracked/ignored files" in capsys.readouterr().err
    assert retained.read_text(encoding="utf-8") == "dirty retained output\n"
    assert finish_path.read_bytes() == finish_before
    assert seal_path.read_bytes() == seal_before
    assert slot_path.is_dir()


def test_legacy_identity_upgrade_refuses_changed_head_with_same_inode(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    slot_path = prepare_dead_validate_slots(project, ("slot01",))["slot01"]
    stub_validate_batch_censuses(monkeypatch)
    interrupt_validate_batch(project, monkeypatch, "after-finish-journal", ("slot01",))
    config = wrkslots._load_config(str(project), "testhost")
    finish_path = wrkslots._journal_path(config)
    seal_path = wrkslots._validate_batch_seal_journal_path(config)
    finish = json.loads(finish_path.read_text(encoding="utf-8"))
    seal = json.loads(seal_path.read_text(encoding="utf-8"))
    before = slot_path.stat()
    legacy_identity = [before.st_dev, before.st_ino, 865]
    finish["private_census_identity"] = legacy_identity
    seal["targets"][0]["identity"] = legacy_identity
    finish_path.write_text(json.dumps(finish, indent=2) + "\n", encoding="utf-8")
    seal_path.write_text(json.dumps(seal, indent=2) + "\n", encoding="utf-8")
    shutil.rmtree(control_directory(project) / "EVENTS.testhost")
    finish_before = finish_path.read_bytes()
    seal_before = seal_path.read_bytes()
    commit_local(slot_path / "product", "seed.txt", "different\n", "different head")
    after = slot_path.stat()
    assert (after.st_dev, after.st_ino) == (before.st_dev, before.st_ino)

    assert (
        wrkslots.main(
            [
                "--project-root",
                str(project),
                "recover",
                "--coordinator-pid",
                str(os.getpid()),
            ]
        )
        == 3
    )
    assert "not reachable from any refs/remotes/origin" in capsys.readouterr().err
    assert finish_path.read_bytes() == finish_before
    assert seal_path.read_bytes() == seal_before
    assert slot_path.is_dir()


@pytest.mark.parametrize("already_restored", (False, True))
def test_seal_recovery_upgrades_legacy_mount_namespace_identity(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, already_restored: bool
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    slot_path = prepare_dead_validate_slots(project, ("slot01",))["slot01"]
    original_mode = stat.S_IMODE(slot_path.stat().st_mode)
    interrupted = raw_command(
        project,
        "remove-validate-batch",
        "--coordinator-pid",
        str(os.getpid()),
        "--slot",
        "slot01=1",
        env={"WRKSLOTS_TEST_INTERRUPT": "after-validate-batch-seal-target"},
    )
    assert interrupted.returncode == 86, interrupted.stderr
    config = wrkslots._load_config(str(project), "testhost")
    seal_path = wrkslots._validate_batch_seal_journal_path(config)
    seal = json.loads(seal_path.read_text(encoding="utf-8"))
    recorded_stable_identity = seal["targets"][0]["identity"]
    assert len(recorded_stable_identity) == 4
    assert tuple(recorded_stable_identity) == wrkslots._private_cleanup_fence_identity(
        config, slot_path
    )
    metadata = slot_path.stat()
    seal["targets"][0]["identity"] = [metadata.st_dev, metadata.st_ino, 865]
    seal_path.write_text(json.dumps(seal, indent=2) + "\n", encoding="utf-8")
    if already_restored:
        slot_path.chmod(original_mode)
    monkeypatch.setattr(wrkslots, "_fd_mount_id", lambda _fd, _label: 3503)

    assert (
        wrkslots.main(
            [
                "--project-root",
                str(project),
                "recover",
                "--coordinator-pid",
                str(os.getpid()),
            ]
        )
        == 0
    )
    assert slot_path.is_dir()
    assert stat.S_IMODE(slot_path.stat().st_mode) == original_mode
    assert not seal_path.exists()


def test_seal_only_legacy_identity_upgrade_allows_dirty_retained_output(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    slot_path = prepare_dead_validate_slots(project, ("slot01",))["slot01"]
    original_mode = stat.S_IMODE(slot_path.stat().st_mode)
    interrupted = raw_command(
        project,
        "remove-validate-batch",
        "--coordinator-pid",
        str(os.getpid()),
        "--slot",
        "slot01=1",
        env={"WRKSLOTS_TEST_INTERRUPT": "after-validate-batch-seal-target"},
    )
    assert interrupted.returncode == 86, interrupted.stderr
    config = wrkslots._load_config(str(project), "testhost")
    seal_path = wrkslots._validate_batch_seal_journal_path(config)
    seal = json.loads(seal_path.read_text(encoding="utf-8"))
    metadata = slot_path.stat()
    seal["targets"][0]["identity"] = [metadata.st_dev, metadata.st_ino, 865]
    seal_path.write_text(json.dumps(seal, indent=2) + "\n", encoding="utf-8")
    retained = slot_path / "product" / "seed.txt"
    retained.write_text("dirty retained output\n", encoding="utf-8")
    monkeypatch.setattr(wrkslots, "_fd_mount_id", lambda _fd, _label: 3503)

    assert (
        wrkslots.main(
            [
                "--project-root",
                str(project),
                "recover",
                "--coordinator-pid",
                str(os.getpid()),
            ]
        )
        == 0
    )
    assert retained.read_text(encoding="utf-8") == "dirty retained output\n"
    assert stat.S_IMODE(slot_path.stat().st_mode) == original_mode
    assert not seal_path.exists()


@pytest.mark.parametrize(
    ("paired_finish", "recovery_kind", "message"),
    (
        (
            False,
            wrkslots._ValidateBatchSealRecoveryKind.PAIRED_FINISH,
            "requires one mutation journal",
        ),
        (
            True,
            wrkslots._ValidateBatchSealRecoveryKind.SEAL_ONLY,
            "cannot run with a paired mutation journal",
        ),
    ),
)
def test_validate_batch_seal_recovery_rejects_wrong_journal_kind(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    paired_finish: bool,
    recovery_kind: wrkslots._ValidateBatchSealRecoveryKind,
    message: str,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    slot_path = prepare_dead_validate_slots(project, ("slot01",))["slot01"]
    if paired_finish:
        stub_validate_batch_censuses(monkeypatch)
        interrupt_validate_batch(
            project, monkeypatch, "after-finish-journal", ("slot01",)
        )
    else:
        interrupted = raw_command(
            project,
            "remove-validate-batch",
            "--coordinator-pid",
            str(os.getpid()),
            "--slot",
            "slot01=1",
            env={"WRKSLOTS_TEST_INTERRUPT": "after-validate-batch-seal-target"},
        )
        assert interrupted.returncode == 86, interrupted.stderr
    config = wrkslots._load_config(str(project), "testhost")
    finish_path = wrkslots._journal_path(config)
    seal_path = wrkslots._validate_batch_seal_journal_path(config)
    seal_before = seal_path.read_bytes()

    with pytest.raises(wrkslots.StateError, match=message):
        wrkslots._recover_validate_batch_seal_journal(
            config, recovery_kind=recovery_kind
        )

    assert slot_path.is_dir()
    assert stat.S_IMODE(slot_path.stat().st_mode) == 0o700
    assert seal_path.read_bytes() == seal_before
    assert finish_path.exists() is paired_finish


@pytest.mark.parametrize(
    ("interrupt", "sealed"),
    (
        ("after-validate-batch-seal-journal", False),
        ("after-validate-batch-seal-target-planned", False),
        ("after-validate-batch-seal-target", True),
        ("after-validate-batch-shared-census", True),
    ),
)
def test_validate_batch_kill_boundaries_recover_without_deletion_or_mode_leak(
    tmp_path: Path,
    interrupt: str,
    sealed: bool,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    slot_path = prepare_dead_validate_slots(project, ("slot01",))["slot01"]
    original_mode = stat.S_IMODE(slot_path.stat().st_mode)

    interrupted = raw_command_with_census_authority_stub(
        project,
        "remove-validate-batch",
        "--coordinator-pid",
        str(os.getpid()),
        "--slot",
        "slot01=1",
        env={"WRKSLOTS_TEST_INTERRUPT": interrupt},
    )

    assert interrupted.returncode == 86, interrupted.stderr
    assert slot_path.is_dir()
    assert stat.S_IMODE(slot_path.stat().st_mode) == (
        0o700 if sealed else original_mode
    )
    config = wrkslots._load_config(str(project), "testhost")
    seal_journal = wrkslots._validate_batch_seal_journal_path(config)
    assert seal_journal.is_file()
    blocked = command(project, "hold", "slot01", "--reason", "blocked")
    assert blocked.returncode == 3
    assert "validation-batch seal" in blocked.stderr

    recovered = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
    )

    assert recovered.returncode == 0, recovered.stderr
    assert slot_path.is_dir()
    assert stat.S_IMODE(slot_path.stat().st_mode) == original_mode
    assert not seal_journal.exists()
    assert len(active_slots(project)) == 1


def test_validate_batch_seal_temp_is_a_fail_closed_partial_update(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    config = wrkslots._load_config(str(project), "testhost")
    journal = {
        "schema": wrkslots._VALIDATE_BATCH_SEAL_SCHEMA,
        "kind": "validate-batch-seal",
        "machine": "testhost",
        "created_at": dt.datetime.now(dt.timezone.utc).isoformat(timespec="seconds"),
        "actor": wrkslots._identity_to_obj(wrkslots._read_process_identity(os.getpid())),
        "targets": [],
    }
    wrkslots._write_validate_batch_seal_journal(config, journal)
    temporary = config.control / f"{wrkslots._validate_batch_seal_journal_path(config).name}.tmp.test"
    temporary.write_text("{}\n", encoding="utf-8")

    refused = command(
        project,
        "recover",
        "--discard-partial",
        "--coordinator-pid",
        str(os.getpid()),
    )

    assert refused.returncode == 3
    assert "cannot automatically discard journal update" in refused.stderr
    assert temporary.is_file()
    assert wrkslots._validate_batch_seal_journal_path(config).is_file()


def test_validate_batch_seal_recovery_refuses_path_disappearing_before_open(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    slot_path = prepare_dead_validate_slots(project, ("slot01",))["slot01"]
    interrupted = raw_command(
        project,
        "remove-validate-batch",
        "--coordinator-pid",
        str(os.getpid()),
        "--slot",
        "slot01=1",
        env={"WRKSLOTS_TEST_INTERRUPT": "after-validate-batch-seal-target"},
    )
    assert interrupted.returncode == 86
    config = wrkslots._load_config(str(project), "testhost")
    seal_journal = wrkslots._validate_batch_seal_journal_path(config)
    real_restore = wrkslots._restore_private_cleanup_path

    def disappear_then_restore(
        path: Path,
        original_mode: int,
        identity: wrkslots._PrivateCleanupIdentity,
        *,
        allow_missing: bool = True,
    ) -> None:
        shutil.rmtree(path)
        real_restore(
            path,
            original_mode,
            identity,
            allow_missing=allow_missing,
        )

    monkeypatch.setattr(
        wrkslots, "_restore_private_cleanup_path", disappear_then_restore
    )

    assert (
        wrkslots.main(
            [
                "--project-root",
                str(project),
                "recover",
                "--coordinator-pid",
                str(os.getpid()),
            ]
        )
        == 3
    )
    assert seal_journal.is_file()
    assert len(active_slots(project)) == 1


def test_validate_batch_recovery_refusal_restores_sealed_mode(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    slot_path = prepare_dead_validate_slots(project, ("slot01",))["slot01"]
    tree = slot_path / "product"
    original_mode = stat.S_IMODE(slot_path.stat().st_mode)

    interrupted = raw_command_with_census_authority_stub(
        project,
        "remove-validate-batch",
        "--coordinator-pid",
        str(os.getpid()),
        "--slot",
        "slot01=1",
        env={"WRKSLOTS_TEST_INTERRUPT": "after-finish-journal"},
    )
    assert interrupted.returncode == 86
    assert stat.S_IMODE(slot_path.stat().st_mode) == 0o700
    config = wrkslots._load_config(str(project), "testhost")
    seal_journal = wrkslots._validate_batch_seal_journal_path(config)
    assert seal_journal.is_file()
    user = subprocess.Popen(["sleep", "60"], cwd=tree, text=True)
    try:
        refused = raw_command_with_census_authority_stub(
            project,
            "recover",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
        )
        assert refused.returncode == 3
        assert "live process" in refused.stderr
        assert stat.S_IMODE(slot_path.stat().st_mode) == original_mode
        assert seal_journal.is_file()
    finally:
        terminate_process(user)

    recovered = raw_command_with_census_authority_stub(
        project,
        "recover",
        "--coordinator-authorized",
        "--coordinator-pid",
        str(os.getpid()),
    )
    assert recovered.returncode == 0, recovered.stderr
    assert not slot_path.exists()
    assert not seal_journal.exists()


@pytest.mark.parametrize(
    "interrupt",
    ("after-finish-journal", "after-path-fence-before-journal"),
)
@pytest.mark.parametrize("substitution", ("mode", "inode"))
def test_validate_batch_finish_recovery_rechecks_private_identity(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    interrupt: str,
    substitution: str,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    slot_path = prepare_dead_validate_slots(project, ("slot01",))["slot01"]
    census_calls: list[tuple[Path, ...]] = []

    def empty_census(paths: Sequence[Path]) -> wrkslots._ProcessPathCensus:
        census_calls.append(tuple(paths))
        return wrkslots._ProcessPathCensus((), ())

    stub_validate_batch_censuses(monkeypatch, empty_census)
    interrupt_validate_batch(project, monkeypatch, interrupt, ("slot01",))
    config = wrkslots._load_config(str(project), "testhost")
    finish_journal = wrkslots._journal_path(config)
    seal_journal = wrkslots._validate_batch_seal_journal_path(config)
    assert finish_journal.is_file()
    assert seal_journal.is_file()
    if interrupt == "after-finish-journal":
        subject = slot_path
    else:
        fenced = tuple(slot_path.parent.glob(".slot01.fenced.1.*"))
        assert len(fenced) == 1
        subject = fenced[0]
    saved = subject.with_name(f"{subject.name}.saved")
    if substitution == "mode":
        subject.chmod(0o750)
    else:
        subject.rename(saved)
        subject.mkdir(mode=0o700)

    census_calls.clear()
    assert (
        wrkslots.main(
            [
                "--project-root",
                str(project),
                "recover",
                "--coordinator-pid",
                str(os.getpid()),
            ]
        )
        == 3
    )
    assert census_calls == []
    assert finish_journal.is_file()
    assert seal_journal.is_file()
    assert len(active_slots(project)) == 1

    if substitution == "mode":
        subject.chmod(0o700)
    else:
        subject.rmdir()
        saved.rename(subject)
    capsys.readouterr()
    census_calls.clear()

    assert (
        wrkslots.main(
            [
                "--project-root",
                str(project),
                "recover",
                "--coordinator-pid",
                str(os.getpid()),
            ]
        )
        == 0
    )
    assert len(census_calls) == 1
    assert not slot_path.exists()
    assert not finish_journal.exists()
    assert not seal_journal.exists()


@pytest.mark.parametrize(
    "interrupt",
    (
        "after-remove-before-journal",
        "after-remove-worktree",
        "after-archive-before-active",
    ),
)
def test_validate_batch_finish_recovery_resumes_destructive_phases(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    interrupt: str,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    slot_path = prepare_dead_validate_slots(project, ("slot01",))["slot01"]
    stub_validate_batch_censuses(monkeypatch)
    interrupt_validate_batch(project, monkeypatch, interrupt, ("slot01",))
    config = wrkslots._load_config(str(project), "testhost")
    finish_journal = wrkslots._journal_path(config)
    seal_journal = wrkslots._validate_batch_seal_journal_path(config)
    assert finish_journal.is_file()
    assert seal_journal.is_file()

    assert (
        wrkslots.main(
            [
                "--project-root",
                str(project),
                "recover",
                "--coordinator-pid",
                str(os.getpid()),
            ]
        )
        == 0
    )
    assert not slot_path.exists()
    assert not finish_journal.exists()
    assert not seal_journal.exists()
    assert active_slots(project) == []


@pytest.mark.parametrize(
    "interrupt",
    ("after-remove-before-journal", "after-remove-worktree"),
)
def test_validate_batch_flat_finish_recovers_after_worktree_removed(
    tmp_path: Path,
    interrupt: str,
) -> None:
    project, _repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    prepare_dead_validate_slots(project, ("slot01",))
    config = wrkslots._load_config(str(project), "testhost")
    slot_path = wrkslots._slot_directory(config, "slot01", "validate")
    assert slot_path.is_dir()

    interrupted = raw_command_with_census_authority_stub(
        project,
        "remove-validate-batch",
        "--coordinator-pid",
        str(os.getpid()),
        "--slot",
        "slot01=1",
        env={"WRKSLOTS_TEST_INTERRUPT": interrupt},
    )

    assert interrupted.returncode == 86, (
        f"stdout:\n{interrupted.stdout}\nstderr:\n{interrupted.stderr}"
    )
    finish_journal = wrkslots._journal_path(config)
    seal_journal = wrkslots._validate_batch_seal_journal_path(config)
    assert finish_journal.is_file()
    assert seal_journal.is_file()
    raw = json.loads(finish_journal.read_text(encoding="utf-8"))
    fenced = wrkslots._finish_fenced_slot(
        config,
        wrkslots._record_from_obj(raw["record"], "finish journal.record"),
        raw,
    )
    assert not slot_path.exists()
    assert not fenced.exists()

    recovered = raw_command_with_census_authority_stub(
        project,
        "recover",
        "--coordinator-authorized",
        "--coordinator-pid",
        str(os.getpid()),
    )

    assert recovered.returncode == 0, recovered.stderr
    assert active_slots(project) == []
    assert not finish_journal.exists()
    assert not seal_journal.exists()
    archive = wrkslots._load_archive(config, require_repository=False)
    assert [(row["slot"], row["generation"]) for row in archive.records] == [
        ("slot01", 1)
    ]
    assert not slot_path.exists()
    assert not fenced.exists()


def test_validate_batch_nested_finish_recovers_after_fence_directory_removed(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    prepare_dead_validate_slots(project, ("slot01",))
    config = wrkslots._load_config(str(project), "testhost")
    slot_path = wrkslots._slot_directory(config, "slot01", "validate")

    interrupted = raw_command_with_census_authority_stub(
        project,
        "remove-validate-batch",
        "--coordinator-pid",
        str(os.getpid()),
        "--slot",
        "slot01=1",
        env={"WRKSLOTS_TEST_INTERRUPT": "after-remove-fenced-directory"},
    )

    assert interrupted.returncode == 86, interrupted.stderr
    finish_journal = wrkslots._journal_path(config)
    seal_journal = wrkslots._validate_batch_seal_journal_path(config)
    assert finish_journal.is_file()
    assert seal_journal.is_file()
    raw = json.loads(finish_journal.read_text(encoding="utf-8"))
    fenced = wrkslots._finish_fenced_slot(
        config,
        wrkslots._record_from_obj(raw["record"], "finish journal.record"),
        raw,
    )
    assert not slot_path.exists()
    assert not fenced.exists()

    recovered = raw_command_with_census_authority_stub(
        project,
        "recover",
        "--coordinator-authorized",
        "--coordinator-pid",
        str(os.getpid()),
    )

    assert recovered.returncode == 0, recovered.stderr
    assert active_slots(project) == []
    assert not finish_journal.exists()
    assert not seal_journal.exists()
    assert not slot_path.exists()
    assert not fenced.exists()


def test_validate_batch_finish_recovery_resumes_after_prior_target_completed(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    paths = prepare_dead_validate_slots(project, ("first", "second"))
    stub_validate_batch_censuses(monkeypatch)
    interrupt_validate_batch(
        project,
        monkeypatch,
        "after-remove-worktree",
        ("first", "second"),
        occurrence=2,
    )
    config = wrkslots._load_config(str(project), "testhost")
    assert not paths["first"].exists()
    assert wrkslots._journal_path(config).is_file()
    assert wrkslots._validate_batch_seal_journal_path(config).is_file()
    assert {row["slot"] for row in active_slots(project) if isinstance(row, dict)} == {
        "second"
    }

    assert (
        wrkslots.main(
            [
                "--project-root",
                str(project),
                "recover",
                "--coordinator-pid",
                str(os.getpid()),
            ]
        )
        == 0
    )
    assert not paths["second"].exists()
    assert not wrkslots._journal_path(config).exists()
    assert not wrkslots._validate_batch_seal_journal_path(config).exists()
    assert active_slots(project) == []


def test_validate_batch_refused_finish_retains_all_sidecar_targets(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    paths = prepare_dead_validate_slots(project, ("first", "second"))
    config = wrkslots._load_config(str(project), "testhost")
    first_path = paths["first"]
    second_path = paths["second"]
    second_original_mode = stat.S_IMODE(second_path.stat().st_mode)
    recovery_in_use = False

    def census(paths: Sequence[Path]) -> wrkslots._ProcessPathCensus:
        if recovery_in_use and tuple(paths) == (first_path,):
            return wrkslots._ProcessPathCensus(
                (), ((98765, str(first_path), "link", str(first_path)),)
            )
        return wrkslots._ProcessPathCensus((), ())

    stub_validate_batch_censuses(monkeypatch, census)
    interrupt_validate_batch(
        project, monkeypatch, "after-finish-journal", ("first", "second")
    )
    saved_second = second_path.with_name("second.saved")
    second_path.rename(saved_second)
    second_path.mkdir(mode=0o700)
    recovery_in_use = True

    assert (
        wrkslots.main(
            [
                "--project-root",
                str(project),
                "recover",
                "--coordinator-pid",
                str(os.getpid()),
            ]
        )
        == 3
    )
    seal_path = wrkslots._validate_batch_seal_journal_path(config)
    _raw, targets = wrkslots._validate_batch_seal_journal(
        config,
        seal_path,
        json.loads(seal_path.read_text(encoding="utf-8")),
    )
    assert {slot for slot, _generation, _target in targets} == {"first", "second"}
    assert wrkslots._journal_path(config).is_file()
    assert first_path.is_dir()

    second_path.rmdir()
    saved_second.rename(second_path)
    recovery_in_use = False
    assert (
        wrkslots.main(
            [
                "--project-root",
                str(project),
                "recover",
                "--coordinator-pid",
                str(os.getpid()),
            ]
        )
        == 0
    )
    assert not first_path.exists()
    assert second_path.is_dir()
    assert stat.S_IMODE(second_path.stat().st_mode) == second_original_mode
    assert not seal_path.exists()
    assert {record["slot"] for record in active_slots(project) if isinstance(record, dict)} == {
        "second"
    }


def test_agent_remove_ignores_a_dead_owner_cgroup_with_a_live_process(
    tmp_path: Path,
) -> None:
    """An agent slot whose exact owner generation is dead is no longer stranded.

    ⚠️ THIS ASSERTION WAS DELIBERATELY REVERSED, and the previous one is worth
    reading before trusting this one. It was
    test_agent_remove_still_refuses_dead_owner_cgroup_with_live_process, and it
    pinned the opposite outcome on purpose: the change that introduced the
    "omit the recorded cgroup comparison once the exact generation is proven
    dead" rule applied it only to disposable checkouts, and this test existed
    to prove the rule had not leaked into ordinary agent slots.

    It is reversed on measurement rather than on preference. On one host the
    comparison blocked 135 slots; in 131 of them the live process supplying the
    refusal touched nothing in the checkout -- not cwd, not root, not exe, not
    any open descriptor -- because a coarse sandbox scope outlives the agent
    that ran in it, and one surviving process stood in for as many as 50 slots.
    The four slots that were genuinely in use are caught by the direct-use
    test, which is unchanged and still runs here.

    What still refuses is covered by the tests around this one: a live owner
    generation, a slot in direct use, an unread handoff, a hold.
    """
    project, repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    tree = checkout(project)
    commit_task(repository, tree, "codex/task")
    handed_off = finish(project)
    assert handed_off.returncode == 0, handed_off.stderr
    mark_owner_dead_in_current_cgroup(project)
    set_liveness(project, "dead")

    removed = remove(project)

    assert removed.returncode == 0, removed.stderr
    assert "remains in recorded owner cgroup" not in removed.stderr
    assert not tree.exists()
    assert not active_slots(project)


def test_recover_resumes_dead_validate_owner_cleanup_from_shared_cgroup(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project, slot_type="validate", branch=None)
    assert made.returncode == 0, made.stderr
    tree = checkout(project, slot_type="validate")
    mark_owner_dead_in_current_cgroup(project)
    set_liveness(project, "dead")
    interrupted = command(
        project,
        "remove",
        "slot01",
        "--validate-complete",
        "--coordinator-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        env={"WRKSLOTS_TEST_INTERRUPT": "after-finish-journal"},
    )
    assert interrupted.returncode == 86
    assert tree.is_dir()

    recovered = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
    )

    assert recovered.returncode == 0, recovered.stderr
    assert not tree.exists()
    assert active_slots(project) == []


@pytest.mark.parametrize(
    "interrupt",
    (None, "after-path-fence-before-journal", "after-remove-before-journal"),
)
def test_validate_remove_with_recursive_submodules_preserves_peers_and_config(
    tmp_path: Path, interrupt: str | None
) -> None:
    project, repository, _remote = make_project(tmp_path)
    component_head, leaf_head = add_recursive_submodules(tmp_path, project, repository)
    peer = project / "peer"
    git(repository, "worktree", "add", "-b", "codex/peer", str(peer), "origin/main")
    git(
        peer,
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "update",
        "--init",
        "--recursive",
    )
    made = create(project, slot_type="validate", branch=None)
    assert made.returncode == 0, made.stderr
    target = checkout(project, slot_type="validate")
    assert git(target / "component", "rev-parse", "HEAD").stdout.strip() == component_head
    assert git(target / "component" / "leaf", "rev-parse", "HEAD").stdout.strip() == leaf_head
    target_admin = tuple(
        Path(git(path, "rev-parse", "--absolute-git-dir").stdout.strip())
        for path in (target, target / "component", target / "component" / "leaf")
    )
    before = submodule_peer_snapshot(repository, peer)
    orphan = slots_directory(project) / "unregistered-history" / "sentinel.txt"
    orphan.parent.mkdir()
    orphan.write_text("must remain\n", encoding="utf-8")
    environment = {} if interrupt is None else {"WRKSLOTS_TEST_INTERRUPT": interrupt}

    removed = raw_command(
        project,
        "--allow-existing-unregistered-worktrees",
        "remove",
        "slot01",
        "--validate-complete",
        "--coordinator-authorized",
        "--coordinator-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        env=environment,
    )

    if interrupt is None:
        assert removed.returncode == 0, removed.stderr
    else:
        assert removed.returncode == 86, removed.stderr
        assert submodule_peer_snapshot(repository, peer) == before
        assert orphan.read_text(encoding="utf-8") == "must remain\n"
        recovered = raw_command(
            project,
            "--allow-existing-unregistered-worktrees",
            "recover",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
        )
        assert recovered.returncode == 0, recovered.stderr

    assert submodule_peer_snapshot(repository, peer) == before
    assert orphan.read_text(encoding="utf-8") == "must remain\n"
    assert not target.exists()
    assert not any(target.parent.glob(".slot01.fenced.*"))
    assert all(not path.exists() and not path.is_symlink() for path in target_admin)
    listed = git(repository, "worktree", "list", "--porcelain").stdout
    assert str(target) not in listed
    assert ".slot01.fenced." not in listed


def test_recursive_submodule_fixture_detects_shared_deinitialization(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    add_recursive_submodules(tmp_path, project, repository)
    peer = project / "peer"
    git(repository, "worktree", "add", "-b", "codex/peer", str(peer), "origin/main")
    git(
        peer,
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "update",
        "--init",
        "--recursive",
    )
    common = Path(
        git(
            repository,
            "rev-parse",
            "--path-format=absolute",
            "--git-common-dir",
        ).stdout.strip()
    )
    config_before = (common / "config").read_bytes()
    assert submodule_peer_snapshot(repository, peer)

    git(peer, "submodule", "deinit", "--force", "--all")

    assert (common / "config").read_bytes() != config_before
    assert any(
        line.startswith("-")
        for line in git(repository, "submodule", "status", "--recursive").stdout.splitlines()
    )


def test_clean_agent_remove_with_recursive_submodules_preserves_peers_and_config(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    add_recursive_submodules(tmp_path, project, repository)
    peer = project / "peer"
    git(repository, "worktree", "add", "-b", "codex/peer", str(peer), "origin/main")
    git(
        peer,
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "update",
        "--init",
        "--recursive",
    )
    made = create(project)
    assert made.returncode == 0, made.stderr
    target = checkout(project)
    before = submodule_peer_snapshot(repository, peer)
    handed_off = finish(project)
    assert handed_off.returncode == 0, handed_off.stderr
    mark_owner_dead(project)
    set_liveness(project, "dead")

    removed = remove(project)

    assert removed.returncode == 0, removed.stderr
    assert not target.exists()
    assert submodule_peer_snapshot(repository, peer) == before
    archive = json.loads(
        (project / "worktrees" / "ARCHIVED.testhost.json").read_text(
            encoding="utf-8"
        )
    )
    assert {
        receipt["disposition"] for receipt in archive["records"][0]["salvage"]
    } == {"already-published"}


def test_validate_slot_removes_checkout_with_unfinished_git_operation(
    tmp_path: Path,
) -> None:
    project, _repository, remote = make_project(tmp_path)
    made = create(project, slot_type="validate", branch=None)
    assert made.returncode == 0, made.stderr
    tree = checkout(project, slot_type="validate")
    merge_head = Path(git(tree, "rev-parse", "--git-path", "MERGE_HEAD").stdout.strip())
    merge_head.write_text(git(tree, "rev-parse", "HEAD").stdout, encoding="utf-8")

    removed = command(
        project,
        "remove",
        "slot01",
        "--validate-complete",
        "--coordinator-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
    )

    assert removed.returncode == 0, removed.stderr
    assert not tree.exists()
    assert git(
        project / "repo",
        "show-ref",
        "--verify",
        "--quiet",
        "refs/heads/wrkslots/validate/testhost/slot01/product",
        check=False,
    ).returncode == 1
    assert (
        git(
            remote,
            "for-each-ref",
            "--format=%(refname)",
            "refs/heads/salvage",
        ).stdout
        == ""
    )


def test_one_agent_may_own_multiple_concurrent_validate_slots(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)
    first = create(project, slot="validate01", slot_type="validate", branch=None)
    second = create(project, slot="validate02", slot_type="validate", branch=None)

    assert first.returncode == 0, first.stderr
    assert second.returncode == 0, second.stderr
    records = [
        wrkslots._as_mapping(record, "test active record")
        for record in active_slots(project)
    ]
    assert {record["slot"] for record in records} == {"validate01", "validate02"}
    assert {record["agent"] for record in records} == {"codex-1"}


def test_later_participant_finishes_interrupted_validate_removal(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project, slot_type="validate", branch=None)
    assert made.returncode == 0, made.stderr
    tree = checkout(project, slot_type="validate")
    (tree / "result.tmp").write_text("disposable result\n", encoding="utf-8")
    set_original_coordinator_dead(project)

    interrupted = command(
        project,
        "remove",
        "slot01",
        "--validate-complete",
        "--coordinator-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        env={"WRKSLOTS_TEST_INTERRUPT": "after-remove-before-journal"},
    )
    assert interrupted.returncode == 86
    assert not tree.exists()
    assert git(
        project / "repo",
        "show-ref",
        "--verify",
        "--quiet",
        "refs/heads/wrkslots/validate/testhost/slot01/product",
        check=False,
    ).returncode == 0

    recovered = raw_command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
    )

    assert recovered.returncode == 0, recovered.stderr
    assert active_slots(project) == []
    assert git(
        project / "repo",
        "show-ref",
        "--verify",
        "--quiet",
        "refs/heads/wrkslots/validate/testhost/slot01/product",
        check=False,
    ).returncode == 1


@pytest.mark.parametrize("slot_type", ("agent", "validate"))
@pytest.mark.parametrize(
    "handoff_state",
    (
        "absent",
        "tracked",
        "modified",
        "untracked",
        "gitignored",
        "configured-cache",
    ),
)
def test_remove_guards_uncommitted_handoffs_for_every_slot_type(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    slot_type: str,
    handoff_state: str,
) -> None:
    project, repository, remote = make_project(
        tmp_path,
        cache_globs=("generated",) if handoff_state == "configured-cache" else (),
    )
    directory = "generated" if handoff_state in {"gitignored", "configured-cache"} else "notes"
    handoff_relative = Path(directory) / "HANDOFF-codex-20260918.md"
    if handoff_state == "gitignored":
        (repository / ".gitignore").write_text("generated/\n", encoding="utf-8")
        git(repository, "add", ".gitignore")
        git(repository, "commit", "-m", "ignore generated output")
        git(repository, "push", "origin", "main")
    if handoff_state in {"tracked", "modified"}:
        source_handoff = repository / handoff_relative
        source_handoff.parent.mkdir()
        source_handoff.write_text("published handoff\n", encoding="utf-8")
        git(repository, "add", str(handoff_relative))
        git(repository, "commit", "-m", "publish handoff")
        git(repository, "push", "origin", "main")
    made = create(
        project,
        slot_type=slot_type,
        branch=None if slot_type == "validate" else "codex/task",
    )
    assert made.returncode == 0, made.stderr
    tree = checkout(project, slot_type=slot_type)
    if slot_type == "agent":
        commit_task(repository, tree, "codex/task")
        handed_off = finish(project)
        assert handed_off.returncode == 0, handed_off.stderr
        mark_owner_dead(project)
        set_liveness(project, "dead")
    handoff = tree / handoff_relative
    if handoff_state in {"untracked", "gitignored", "configured-cache"}:
        handoff.parent.mkdir()
        handoff.write_text("continue from this exact state\n", encoding="utf-8")
    elif handoff_state == "modified":
        handoff.write_text("continue from changed state\n", encoding="utf-8")
    monkeypatch.setattr(
        wrkslots,
        "_assert_slot_unused",
        lambda *_args, **_kwargs: None,
    )
    args = [
        "--project-root",
        str(project),
        "remove",
        "slot01",
        "--coordinator-authorized",
        "--coordinator-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
    ]
    if slot_type == "validate":
        args.append("--validate-complete")
    stdout = io.StringIO()
    stderr = io.StringIO()
    with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
        returncode = wrkslots.main(args)
    removed = subprocess.CompletedProcess(
        args,
        returncode,
        stdout.getvalue(),
        stderr.getvalue(),
    )

    if handoff_state in {"absent", "tracked", "gitignored", "configured-cache"}:
        assert removed.returncode == 0, removed.stderr
        assert not tree.exists()
    else:
        assert removed.returncode == 3
        assert handoff.name in removed.stderr
        assert "slot and every checkout were retained" in removed.stderr
        assert handoff.is_file()
        assert tree.is_dir()
        assert (
            git(remote, "for-each-ref", "--format=%(refname)", "refs/heads/salvage").stdout
            == ""
        )


def test_unread_handoff_blocks_reclaim_until_its_exact_contents_are_read(
    tmp_path: Path,
) -> None:
    project, _repository, remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    handoff = checkout(project).parent / "HANDOFF.md"
    handoff.write_text("Continue from the exact recorded commit.\n", encoding="utf-8")
    mark_owner_dead(project)
    set_liveness(project, "dead")

    refused = remove(project)

    assert refused.returncode == 3
    assert "contains an unread HANDOFF.md" in refused.stderr
    assert "no checkout was salvaged or removed" in refused.stderr
    assert "wrkslots read-handoff" in refused.stderr
    assert checkout(project).is_dir()
    assert git(remote, "for-each-ref", "--format=%(refname)", "refs/heads/salvage").stdout == ""

    read = raw_command(
        project,
        "read-handoff",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
    )
    assert read.returncode == 0, read.stderr
    assert "Continue from the exact recorded commit." in read.stdout

    removed = remove(project)
    assert removed.returncode == 0, removed.stderr
    assert not handoff.exists()
    events = wrkslots._load_events(wrkslots._load_config(str(project), "testhost"))
    read_events = [event for event in events if event["kind"] == "handoff-read"]
    assert len(read_events) == 2
    assert [
        wrkslots._as_mapping(event["payload"], "handoff read payload")[
            "retirement_enqueued"
        ]
        for event in read_events
    ] == [False, True]
    payload = read_events[-1]["payload"]
    assert isinstance(payload, dict)
    assert payload["sha256"] == hashlib.sha256(
        b"Continue from the exact recorded commit.\n"
    ).hexdigest()


def test_write_handoff_is_external_and_immutable_after_finish(tmp_path: Path) -> None:
    project, repository, _remote = make_project(
        tmp_path, worktrees_directory="worktrees/slots", layout="flat"
    )
    assert create(project).returncode == 0
    oversized = tmp_path / "oversized-handoff.md"
    oversized.write_bytes(b"x" * (wrkslots.HANDOFF_BYTES_LIMIT + 1))
    too_large = raw_command(
        project,
        "write-handoff",
        "slot01",
        "--agent",
        "codex-1",
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        "--from-file",
        str(oversized),
    )
    assert too_large.returncode == 3
    assert "1 MiB safety bound" in too_large.stderr
    source = tmp_path / "handoff-input.md"
    source.write_text("continue from the published commit\n", encoding="utf-8")

    written = raw_command(
        project,
        "write-handoff",
        "slot01",
        "--agent",
        "codex-1",
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        "--from-file",
        str(source),
    )

    assert written.returncode == 0, written.stderr
    config = wrkslots._load_config(str(project), "testhost")
    _contents, source_identity = wrkslots._read_regular_file_identity(
        source, "test handoff source", wrkslots.HANDOFF_BYTES_LIMIT
    )
    sidecar = wrkslots._handoff_sidecar_path(config, "slot01", 1)
    assert sidecar.is_file()
    sidecar_payload = json.loads(sidecar.read_text(encoding="utf-8"))
    assert sidecar_payload["schema"] == wrkslots.HANDOFF_SIDECAR_SCHEMA
    assert sidecar_payload["source"] == "write-handoff"
    assert sidecar_payload["source_path"] == str(source.absolute())
    assert sidecar_payload["source_identity"] == wrkslots._regular_file_identity_to_obj(
        source_identity
    )
    assert not (checkout(project) / "HANDOFF.md").exists()
    source.write_text("changed after the atomic projection\n", encoding="utf-8")
    read = raw_command(
        project,
        "read-handoff",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
    )
    assert read.returncode == 0, read.stderr
    assert "continue from the published commit" in read.stdout
    assert "changed after the atomic projection" not in read.stdout
    events = wrkslots._load_events(config)
    for kind in ("handoff-written", "handoff-read"):
        payload = next(event["payload"] for event in events if event["kind"] == kind)
        assert isinstance(payload, dict)
        assert payload["source"] == "write-handoff"
        assert payload["source_path"] == str(source.absolute())
        assert payload["source_identity"] == wrkslots._regular_file_identity_to_obj(
            source_identity
        )
    commit_task(repository, checkout(project), "codex/task")
    handed_off = finish(project)
    assert handed_off.returncode == 0, handed_off.stderr

    refused = raw_command(
        project,
        "write-handoff",
        "slot01",
        "--agent",
        "codex-1",
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        "--from-file",
        str(source),
    )
    assert refused.returncode == 3
    assert "handoff is immutable" in refused.stderr


def test_write_handoff_refuses_tracked_checkout_source_without_queueing(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    broadcast = repository / "HANDOFF.md"
    broadcast.write_text("shared repository instructions\n", encoding="utf-8")
    git(repository, "add", "HANDOFF.md")
    git(repository, "commit", "-m", "add shared instructions")
    git(repository, "push", "origin", "main")
    assert create(project).returncode == 0
    source = checkout(project) / "HANDOFF.md"
    assert git(
        checkout(project), "ls-files", "--error-unmatch", "HANDOFF.md"
    ).returncode == 0
    assert git(checkout(project), "status", "--porcelain").stdout == ""

    refused = raw_command(
        project,
        "write-handoff",
        "slot01",
        "--agent",
        "codex-1",
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        "--from-file",
        str(source),
    )

    assert refused.returncode == 3
    assert "inside Git working tree" in refused.stderr
    unread = raw_command(
        project,
        "read-handoff",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
    )
    assert unread.returncode == 3
    assert "has no handoff sidecar or legacy HANDOFF.md" in unread.stderr
    queued = json.loads(
        raw_command(project, "retirement-queue", "--format", "json").stdout
    )
    assert queued["pending"] == []


def test_two_flat_tracked_handoffs_are_not_legacy_retirement_evidence(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(
        tmp_path, worktrees_directory="worktrees/slots", layout="flat"
    )
    broadcast = repository / "HANDOFF.md"
    broadcast.write_text("identical shared instructions\n", encoding="utf-8")
    git(repository, "add", "HANDOFF.md")
    git(repository, "commit", "-m", "add shared instructions")
    git(repository, "push", "origin", "main")
    assert create(project).returncode == 0
    assert create(
        project, slot="slot02", agent="codex-2", branch="codex/task-2"
    ).returncode == 0
    first = checkout(project)
    second = checkout(project, slot="slot02")
    assert (first / "HANDOFF.md").read_bytes() == (second / "HANDOFF.md").read_bytes()
    for tree in (first, second):
        assert git(
            tree, "ls-files", "--error-unmatch", "HANDOFF.md"
        ).returncode == 0
        assert git(tree, "status", "--porcelain").stdout == ""

    first_read = raw_command(
        project,
        "read-handoff",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
    )
    second_read = raw_command(
        project,
        "read-handoff",
        "slot02",
        "--coordinator-pid",
        str(os.getpid()),
    )

    for refused_read in (first_read, second_read):
        assert refused_read.returncode == 3
        assert "tracked by Git" in refused_read.stderr
    queued = json.loads(
        raw_command(project, "retirement-queue", "--format", "json").stdout
    )
    assert queued["pending"] == []
    config = wrkslots._load_config(str(project), "testhost")
    assert not any(
        event["kind"] == "handoff-read" for event in wrkslots._load_events(config)
    )


def test_write_handoff_refuses_indeterminate_git_source_provenance(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    source = tmp_path / "external-handoff.md"
    source.write_text("slot-specific continuation\n", encoding="utf-8")

    def indeterminate_git(
        _repository: Path,
        _args: Sequence[str],
        *,
        check: bool = True,
        input_text: str | None = None,
        env_overrides: Mapping[str, str] | None = None,
    ) -> subprocess.CompletedProcess[str]:
        del check, input_text, env_overrides
        return subprocess.CompletedProcess(
            ["git"], 2, "", "fatal: simulated provenance failure\n"
        )

    monkeypatch.setattr(wrkslots._GitVcs, "_run", staticmethod(indeterminate_git))
    args = argparse.Namespace(
        project_root=str(project),
        machine="testhost",
        slot="slot01",
        agent="codex-1",
        owner_pid=os.getpid(),
        expected_generation=1,
        from_file=str(source),
        wait_lock=0.0,
    )
    with pytest.raises(wrkslots.Refusal, match="cannot determine Git provenance"):
        wrkslots._cmd_write_handoff(args)

    config = wrkslots._load_config(str(project), "testhost")
    assert not wrkslots._handoff_sidecar_path(config, "slot01", 1).exists()
    assert not any(
        event["kind"] in {"handoff-written", "handoff-read"}
        for event in wrkslots._load_events(config)
    )


def test_legacy_handoff_refuses_indeterminate_git_tracking_provenance(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _repository, _remote = make_project(
        tmp_path, worktrees_directory="worktrees/slots", layout="flat"
    )
    assert create(project).returncode == 0
    legacy = checkout(project) / "HANDOFF.md"
    legacy.write_text("slot-specific continuation\n", encoding="utf-8")
    original_run = wrkslots._GitVcs._run

    def indeterminate_tracking(
        repository: Path,
        args: Sequence[str],
        *,
        check: bool = True,
        input_text: str | None = None,
        env_overrides: Mapping[str, str] | None = None,
    ) -> subprocess.CompletedProcess[str]:
        if "ls-files" in args:
            return subprocess.CompletedProcess(
                ["git"], 2, "", "fatal: simulated tracking failure\n"
            )
        return original_run(
            repository,
            args,
            check=check,
            input_text=input_text,
            env_overrides=env_overrides,
        )

    monkeypatch.setattr(
        wrkslots._GitVcs, "_run", staticmethod(indeterminate_tracking)
    )
    args = argparse.Namespace(
        project_root=str(project),
        machine="testhost",
        slot="slot01",
        coordinator_pid=os.getpid(),
        coordinator_authorized=False,
        adopt_legacy_change=False,
        expected_generation=None,
        wait_lock=0.0,
    )
    with pytest.raises(
        wrkslots.Refusal,
        match="cannot determine whether legacy HANDOFF.md is tracked by Git",
    ):
        wrkslots._cmd_read_handoff(args)

    config = wrkslots._load_config(str(project), "testhost")
    assert not wrkslots._handoff_sidecar_path(config, "slot01", 1).exists()
    assert not any(
        event["kind"] == "handoff-read" for event in wrkslots._load_events(config)
    )


def _install_schema_one_handoff_sidecar(
    project: Path,
    *,
    contents: bytes,
    source: str,
    source_identity: wrkslots._RegularFileIdentity | None,
) -> tuple[wrkslots.Config, Path]:
    config = wrkslots._load_config(str(project), "testhost")
    record = next(item for item in wrkslots._load_active(config).slots if item.slot == "slot01")
    sidecar = wrkslots._handoff_sidecar_path(config, "slot01", 1)
    wrkslots._atomic_write_json(
        sidecar,
        {
            "schema": wrkslots.HANDOFF_SIDECAR_LEGACY_SCHEMA,
            "machine": record.machine,
            "slot": record.slot,
            "generation": record.generation,
            "recorded_at": wrkslots._utc_now(),
            "source": source,
            "source_identity": (
                None
                if source_identity is None
                else wrkslots._regular_file_identity_to_obj(source_identity)
            ),
            "contents_utf8": contents.decode("utf-8"),
            "sha256": hashlib.sha256(contents).hexdigest(),
        },
    )
    return config, sidecar


def _append_historical_unprovenanced_read(
    config: wrkslots.Config, contents: bytes
) -> None:
    wrkslots._ensure_event_log(config)
    wrkslots._write_event_file(
        config,
        config.machine,
        "handoff-read",
        {
            "slot": "slot01",
            "generation": 1,
            "sha256": hashlib.sha256(contents).hexdigest(),
            "contents_utf8": contents.decode("utf-8"),
            "retirement_enqueued": True,
        },
    )


def test_write_handoff_noreplace_preserves_planted_destination_and_source(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    source = tmp_path / "external-handoff.md"
    source.write_text("owner-authored continuation\n", encoding="utf-8")
    config = wrkslots._load_config(str(project), "testhost")
    sidecar = wrkslots._handoff_sidecar_path(config, "slot01", 1)

    def plant(point: str) -> None:
        if point == "before-handoff-sidecar-publish":
            sidecar.write_text("planted destination\n", encoding="utf-8")

    monkeypatch.setattr(wrkslots, "_interrupt_for_test", plant)
    args = argparse.Namespace(
        project_root=str(project),
        machine="testhost",
        slot="slot01",
        agent="codex-1",
        owner_pid=os.getpid(),
        expected_generation=1,
        from_file=str(source),
        wait_lock=0.0,
    )
    with pytest.raises(wrkslots.Refusal, match="destination already exists"):
        wrkslots._cmd_write_handoff(args)

    assert sidecar.read_text(encoding="utf-8") == "planted destination\n"
    assert source.read_text(encoding="utf-8") == "owner-authored continuation\n"
    assert len(list(config.control.glob(f"{sidecar.name}.tmp.*"))) == 1
    assert not any(
        event["kind"] == "handoff-written" for event in wrkslots._load_events(config)
    )


def test_legacy_projection_noreplace_does_not_enqueue_planted_destination(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    legacy = checkout(project).parent / "HANDOFF.md"
    legacy.write_text("legacy continuation\n", encoding="utf-8")
    config = wrkslots._load_config(str(project), "testhost")
    sidecar = wrkslots._handoff_sidecar_path(config, "slot01", 1)

    def plant(point: str) -> None:
        if point == "before-handoff-sidecar-publish":
            sidecar.write_text("planted destination\n", encoding="utf-8")

    monkeypatch.setattr(wrkslots, "_interrupt_for_test", plant)
    args = argparse.Namespace(
        project_root=str(project),
        machine="testhost",
        slot="slot01",
        coordinator_pid=os.getpid(),
        coordinator_authorized=False,
        adopt_legacy_change=False,
        expected_generation=None,
        wait_lock=0.0,
    )
    with pytest.raises(wrkslots.Refusal, match="destination already exists"):
        wrkslots._cmd_read_handoff(args)

    assert sidecar.read_text(encoding="utf-8") == "planted destination\n"
    assert legacy.read_text(encoding="utf-8") == "legacy continuation\n"
    reads = [
        event for event in wrkslots._load_events(config) if event["kind"] == "handoff-read"
    ]
    assert len(reads) == 1
    payload = reads[0]["payload"]
    assert isinstance(payload, dict)
    assert payload["retirement_enqueued"] is False
    queued = raw_command(project, "retirement-queue", "--format", "json")
    assert queued.returncode == 3
    assert "partial atomic update" in queued.stderr



@pytest.mark.parametrize("storage", ["canonical", "temp"])
def test_wrong_writer_intent_cannot_authorize_sidecar_or_recovery(
    tmp_path: Path, storage: str
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    config = wrkslots._load_config(str(project), "testhost")
    record = wrkslots._load_active(config).slots[0]
    assert record.owner is not None
    source = tmp_path / "external-handoff.md"
    source.write_text("wrong writer must not authorize\n", encoding="utf-8")
    contents, identity = wrkslots._read_regular_file_identity(
        source, "wrong-writer source", wrkslots.HANDOFF_BYTES_LIMIT
    )
    sidecar = wrkslots._handoff_sidecar_path(config, "slot01", 1)
    artifact = wrkslots._HandoffArtifact(
        path=sidecar, machine=record.machine, slot=record.slot,
        generation=record.generation, contents=contents, sha256=identity.sha256,
        recorded_at=wrkslots._utc_now(), source="write-handoff",
        source_path=source.resolve(), source_identity=identity,
        provenance_complete=True, storage_identity=identity,
    )
    target = sidecar if storage == "canonical" else sidecar.with_name(
        f"{sidecar.name}.tmp.wrong-writer"
    )
    target.write_text(
        json.dumps(wrkslots._handoff_artifact_to_obj(artifact), indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    wrong_writer = replace(record.owner, boot_id="wrong-writer-boot")
    wrkslots._ensure_event_log(config)
    wrkslots._write_event_file(
        config,
        record.machine,
        "handoff-write-intended",
        {
            "slot": record.slot,
            "generation": record.generation,
            "artifact": wrkslots._handoff_artifact_to_obj(artifact),
            "writer": wrkslots._identity_to_obj(wrong_writer),
        },
    )

    if storage == "canonical":
        refused = raw_command(
            project, "read-handoff", "slot01", "--coordinator-pid", str(os.getpid())
        )
    else:
        refused = raw_command(
            project,
            "recover",
            "--coordinator-pid",
            str(os.getpid()),
            "--discard-partial",
        )

    assert refused.returncode == 3
    assert "writer does not match the recorded slot owner" in refused.stderr
    assert target.is_file()
    if storage == "temp":
        assert not sidecar.exists()



@pytest.mark.parametrize("target_present", [False, True])
def test_write_intent_prevents_legacy_temp_recovery_in_both_branches(
    tmp_path: Path, target_present: bool
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    legacy = checkout(project).parent / "HANDOFF.md"
    legacy.write_text("legacy projection bytes\n", encoding="utf-8")
    interrupted = raw_command(
        project, "read-handoff", "slot01", "--coordinator-pid", str(os.getpid()),
        env={"WRKSLOTS_TEST_INTERRUPT": "before-handoff-sidecar-publish"},
    )
    assert interrupted.returncode == 86
    config = wrkslots._load_config(str(project), "testhost")
    record = wrkslots._load_active(config).slots[0]
    sidecar = wrkslots._handoff_sidecar_path(config, "slot01", 1)
    temps = list(config.control.glob(f"{sidecar.name}.tmp.*"))
    assert len(temps) == 1
    if target_present:
        sidecar.write_bytes(temps[0].read_bytes())
    source = tmp_path / "owner-intended.md"
    source.write_text("owner intent wins\n", encoding="utf-8")
    contents, identity = wrkslots._read_regular_file_identity(
        source, "owner intent source", wrkslots.HANDOFF_BYTES_LIMIT
    )
    artifact = wrkslots._HandoffArtifact(
        path=sidecar, machine=record.machine, slot=record.slot,
        generation=record.generation, contents=contents, sha256=identity.sha256,
        recorded_at=wrkslots._utc_now(), source="write-handoff",
        source_path=source.resolve(), source_identity=identity,
        provenance_complete=True, storage_identity=identity,
    )
    wrkslots._write_event_file(
        config,
        record.machine,
        "handoff-write-intended",
        {
            "slot": record.slot,
            "generation": record.generation,
            "artifact": wrkslots._handoff_artifact_to_obj(artifact),
            "writer": wrkslots._identity_to_obj(
                wrkslots._read_process_identity(os.getpid())
            ),
        },
    )

    refused = raw_command(
        project, "recover", "--coordinator-pid", str(os.getpid()), "--discard-partial"
    )

    assert refused.returncode == 3
    assert "legacy sidecar recovery cannot supersede" in refused.stderr
    assert temps[0].is_file()
    assert sidecar.exists() is target_present



def test_outstanding_write_intent_prevents_legacy_projection_until_owner_retry(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    contents = "owner-selected continuation\n"
    source = tmp_path / "external-handoff.md"
    source.write_text(contents, encoding="utf-8")
    legacy = checkout(project).parent / "HANDOFF.md"
    legacy.write_text(contents, encoding="utf-8")
    write_args = (
        "write-handoff", "slot01", "--agent", "codex-1", "--owner-pid",
        str(os.getpid()), "--expected-generation", "1", "--from-file", str(source),
    )
    interrupted = raw_command(
        project,
        *write_args,
        env={"WRKSLOTS_TEST_INTERRUPT": "after-handoff-write-intent"},
    )
    assert interrupted.returncode == 86
    config = wrkslots._load_config(str(project), "testhost")
    sidecar = wrkslots._handoff_sidecar_path(config, "slot01", 1)
    assert not sidecar.exists()
    assert not list(config.control.glob(f"{sidecar.name}.tmp.*"))

    refused = raw_command(
        project, "read-handoff", "slot01", "--coordinator-pid", str(os.getpid())
    )
    assert refused.returncode == 3
    assert "outstanding owner-authenticated handoff write intent" in refused.stderr
    assert not any(
        event["kind"] == "handoff-read" for event in wrkslots._load_events(config)
    )
    assert json.loads(
        raw_command(project, "retirement-queue", "--format", "json").stdout
    )["pending"] == []

    retried = raw_command(project, *write_args)
    assert retried.returncode == 0, retried.stderr
    publication = wrkslots._handoff_write_publication(
        wrkslots._load_active(config).slots[0], wrkslots._load_events(config)
    )
    assert publication is not None and publication[2] is True
    read = raw_command(
        project, "read-handoff", "slot01", "--coordinator-pid", str(os.getpid())
    )
    assert read.returncode == 0, read.stderr


def test_completed_write_intent_with_missing_sidecar_precedes_legacy(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    contents = "owner-selected continuation\n"
    source = tmp_path / "external-handoff.md"
    source.write_text(contents, encoding="utf-8")
    legacy = checkout(project).parent / "HANDOFF.md"
    legacy.write_text(contents, encoding="utf-8")
    written = raw_command(
        project, "write-handoff", "slot01", "--agent", "codex-1", "--owner-pid",
        str(os.getpid()), "--expected-generation", "1", "--from-file", str(source),
    )
    assert written.returncode == 0, written.stderr
    config = wrkslots._load_config(str(project), "testhost")
    sidecar = wrkslots._handoff_sidecar_path(config, "slot01", 1)
    sidecar.unlink()

    refused = raw_command(
        project, "read-handoff", "slot01", "--coordinator-pid", str(os.getpid())
    )

    assert refused.returncode == 3
    assert "completed owner-authenticated handoff write intent" in refused.stderr
    assert "restore the exact completed sidecar" in refused.stderr
    assert legacy.is_file()
    assert not any(
        event["kind"] == "handoff-read" for event in wrkslots._load_events(config)
    )



def test_write_handoff_rejects_pre_snapshot_temp_substitution(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    source = tmp_path / "external-handoff.md"
    source.write_text("owner-authored continuation\n", encoding="utf-8")
    config = wrkslots._load_config(str(project), "testhost")
    sidecar = wrkslots._handoff_sidecar_path(config, "slot01", 1)
    replacement = b"pre-snapshot replacement must survive\n"

    def replace_temp(point: str) -> None:
        if point != "after-handoff-temp-write-before-snapshot":
            return
        temps = list(config.control.glob(f"{sidecar.name}.tmp.*"))
        assert len(temps) == 1
        planted = config.control / "pre-snapshot-replacement"
        planted.write_bytes(replacement)
        os.replace(planted, temps[0])

    monkeypatch.setattr(wrkslots, "_interrupt_for_test", replace_temp)
    args = argparse.Namespace(
        project_root=str(project), machine="testhost", slot="slot01", agent="codex-1",
        owner_pid=os.getpid(), expected_generation=1, from_file=str(source), wait_lock=0.0,
    )
    with pytest.raises(wrkslots.StateError, match="differs from its durable intent"):
        wrkslots._cmd_write_handoff(args)

    assert not sidecar.exists()
    temps = list(config.control.glob(f"{sidecar.name}.tmp.*"))
    assert len(temps) == 1
    assert temps[0].read_bytes() == replacement
    publication = wrkslots._handoff_write_publication(
        wrkslots._load_active(config).slots[0], wrkslots._load_events(config)
    )
    assert publication is not None and publication[2] is False


def test_write_handoff_post_publish_replacement_stays_untrusted(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    source = tmp_path / "external-handoff.md"
    source.write_text("owner-authored continuation\n", encoding="utf-8")
    config = wrkslots._load_config(str(project), "testhost")
    sidecar = wrkslots._handoff_sidecar_path(config, "slot01", 1)

    def replace_after_helper(point: str) -> None:
        if point != "after-handoff-sidecar-write":
            return
        payload = json.loads(sidecar.read_text(encoding="utf-8"))
        planted = b"post-publication replacement\n"
        digest = hashlib.sha256(planted).hexdigest()
        payload["contents_utf8"] = planted.decode("utf-8")
        payload["sha256"] = digest
        payload["source_identity"]["size"] = len(planted)
        payload["source_identity"]["sha256"] = digest
        sidecar.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")

    monkeypatch.setattr(wrkslots, "_interrupt_for_test", replace_after_helper)
    args = argparse.Namespace(
        project_root=str(project), machine="testhost", slot="slot01", agent="codex-1",
        owner_pid=os.getpid(), expected_generation=1, from_file=str(source), wait_lock=0.0,
    )
    with pytest.raises(wrkslots.StateError, match="differs from the owner's exact"):
        wrkslots._cmd_write_handoff(args)
    publication = wrkslots._handoff_write_publication(
        wrkslots._load_active(config).slots[0], wrkslots._load_events(config)
    )
    assert publication is not None and publication[2] is False

    monkeypatch.setattr(wrkslots, "_interrupt_for_test", lambda _point: None)
    refused = raw_command(
        project, "read-handoff", "slot01", "--coordinator-pid", str(os.getpid())
    )
    assert refused.returncode == 3
    assert "no matching owner-authenticated publication intent" in refused.stderr
    queued = raw_command(project, "retirement-queue", "--format", "json")
    assert queued.returncode == 0, queued.stderr
    assert json.loads(queued.stdout)["pending"] == []


def test_recover_refuses_unauthenticated_orphan_handoff_temp(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    config = wrkslots._load_config(str(project), "testhost")
    record = wrkslots._load_active(config).slots[0]
    source = tmp_path / "orphan-source.md"
    source.write_text("orphan bytes\n", encoding="utf-8")
    contents, identity = wrkslots._read_regular_file_identity(
        source, "orphan source", wrkslots.HANDOFF_BYTES_LIMIT
    )
    sidecar = wrkslots._handoff_sidecar_path(config, "slot01", 1)
    artifact = wrkslots._HandoffArtifact(
        path=sidecar, machine=record.machine, slot=record.slot, generation=record.generation,
        contents=contents, sha256=identity.sha256, recorded_at=wrkslots._utc_now(),
        source="write-handoff", source_path=source.resolve(), source_identity=identity,
        provenance_complete=True, storage_identity=identity,
    )
    temp = sidecar.with_name(f"{sidecar.name}.tmp.orphan")
    temp.write_text(
        json.dumps(wrkslots._handoff_artifact_to_obj(artifact), indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )

    refused = raw_command(
        project, "recover", "--coordinator-pid", str(os.getpid()), "--discard-partial"
    )

    assert refused.returncode == 3
    assert "no matching owner-authenticated publication intent" in refused.stderr
    assert temp.is_file()
    assert not sidecar.exists()


def test_recover_completes_authenticated_write_handoff_publication(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    source = tmp_path / "external-handoff.md"
    source.write_text("recover authenticated write\n", encoding="utf-8")
    interrupted = raw_command(
        project, "write-handoff", "slot01", "--agent", "codex-1", "--owner-pid",
        str(os.getpid()), "--expected-generation", "1", "--from-file", str(source),
        env={"WRKSLOTS_TEST_INTERRUPT": "before-handoff-sidecar-publish"},
    )
    assert interrupted.returncode == 86

    recovered = raw_command(
        project, "recover", "--coordinator-pid", str(os.getpid()), "--discard-partial"
    )

    assert recovered.returncode == 0, recovered.stderr
    config = wrkslots._load_config(str(project), "testhost")
    record = wrkslots._load_active(config).slots[0]
    publication = wrkslots._handoff_write_publication(record, wrkslots._load_events(config))
    assert publication is not None and publication[2] is True
    read = raw_command(
        project, "read-handoff", "slot01", "--coordinator-pid", str(os.getpid())
    )
    assert read.returncode == 0, read.stderr


def test_recover_completes_provenance_bound_legacy_projection(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    legacy = checkout(project).parent / "HANDOFF.md"
    legacy.write_text("recover legacy projection\n", encoding="utf-8")
    interrupted = raw_command(
        project, "read-handoff", "slot01", "--coordinator-pid", str(os.getpid()),
        env={"WRKSLOTS_TEST_INTERRUPT": "before-handoff-sidecar-publish"},
    )
    assert interrupted.returncode == 86

    recovered = raw_command(
        project, "recover", "--coordinator-pid", str(os.getpid()), "--discard-partial"
    )

    assert recovered.returncode == 0, recovered.stderr
    config = wrkslots._load_config(str(project), "testhost")
    reads = [event for event in wrkslots._load_events(config) if event["kind"] == "handoff-read"]
    assert len(reads) == 2
    assert [wrkslots._as_mapping(event["payload"], "read")["retirement_enqueued"] for event in reads] == [False, True]
    queued = json.loads(raw_command(project, "retirement-queue", "--format", "json").stdout)
    assert len(queued["pending"]) == 1



def test_write_handoff_same_bytes_different_source_provenance_refuses(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    first_source = tmp_path / "first-external-handoff.md"
    second_source = tmp_path / "second-external-handoff.md"
    first_source.write_text("identical bytes\n", encoding="utf-8")
    second_source.write_text("identical bytes\n", encoding="utf-8")
    base = (
        "write-handoff",
        "slot01",
        "--agent",
        "codex-1",
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        "--from-file",
    )
    first = raw_command(project, *base, str(first_source))
    assert first.returncode == 0, first.stderr
    config = wrkslots._load_config(str(project), "testhost")
    sidecar = wrkslots._handoff_sidecar_path(config, "slot01", 1)
    before = sidecar.read_bytes()

    refused = raw_command(project, *base, str(second_source))

    assert refused.returncode == 3
    assert "different contents or source provenance" in refused.stderr
    assert sidecar.read_bytes() == before
    writes = [
        event for event in wrkslots._load_events(config) if event["kind"] == "handoff-written"
    ]
    assert len(writes) == 1



def test_write_handoff_rejects_and_restores_replaced_publication_temp(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    source = tmp_path / "external-handoff.md"
    source.write_text("owner-authored continuation\n", encoding="utf-8")
    config = wrkslots._load_config(str(project), "testhost")
    sidecar = wrkslots._handoff_sidecar_path(config, "slot01", 1)
    replacement = b"replacement temp must survive\n"

    def replace_temp(point: str) -> None:
        if point != "before-handoff-sidecar-publish":
            return
        temps = list(config.control.glob(f"{sidecar.name}.tmp.*"))
        assert len(temps) == 1
        planted = config.control / "replacement-publication-temp"
        planted.write_bytes(replacement)
        os.replace(planted, temps[0])

    monkeypatch.setattr(wrkslots, "_interrupt_for_test", replace_temp)
    args = argparse.Namespace(
        project_root=str(project),
        machine="testhost",
        slot="slot01",
        agent="codex-1",
        owner_pid=os.getpid(),
        expected_generation=1,
        from_file=str(source),
        wait_lock=0.0,
    )
    with pytest.raises(wrkslots.StateError, match="preserved the raced artifact"):
        wrkslots._cmd_write_handoff(args)

    assert not sidecar.exists()
    temps = list(config.control.glob(f"{sidecar.name}.tmp.*"))
    assert len(temps) == 1
    assert temps[0].read_bytes() == replacement
    assert source.read_text(encoding="utf-8") == "owner-authored continuation\n"
    assert not any(
        event["kind"] == "handoff-written" for event in wrkslots._load_events(config)
    )


def test_recover_rejects_and_restores_replaced_handoff_temp(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    source = tmp_path / "external-handoff.md"
    source.write_text("owner-authored continuation\n", encoding="utf-8")
    write_args = (
        "write-handoff",
        "slot01",
        "--agent",
        "codex-1",
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        "--from-file",
        str(source),
    )
    interrupted = raw_command(
        project,
        *write_args,
        env={"WRKSLOTS_TEST_INTERRUPT": "before-handoff-sidecar-publish"},
    )
    assert interrupted.returncode == 86
    config = wrkslots._load_config(str(project), "testhost")
    sidecar = wrkslots._handoff_sidecar_path(config, "slot01", 1)
    temps = list(config.control.glob(f"{sidecar.name}.tmp.*"))
    assert len(temps) == 1
    replacement = b"replacement recovery temp must survive\n"

    def replace_temp(point: str) -> None:
        if point != "before-handoff-temp-recovery-publish":
            return
        planted = config.control / "replacement-recovery-temp"
        planted.write_bytes(replacement)
        os.replace(planted, temps[0])

    monkeypatch.setattr(wrkslots, "_interrupt_for_test", replace_temp)
    code = wrkslots.main(
        [
            "--project-root",
            str(project),
            "recover",
            "--coordinator-pid",
            str(os.getpid()),
            "--discard-partial",
        ]
    )

    assert code == 3
    assert not sidecar.exists()
    assert temps[0].read_bytes() == replacement
    assert source.read_text(encoding="utf-8") == "owner-authored continuation\n"



def test_recover_preserves_conflicting_planted_sidecar_and_owner_temp(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    source = tmp_path / "external-handoff.md"
    source.write_text("owner-authored continuation\n", encoding="utf-8")
    config = wrkslots._load_config(str(project), "testhost")
    sidecar = wrkslots._handoff_sidecar_path(config, "slot01", 1)

    def plant_valid_conflict(point: str) -> None:
        if point != "before-handoff-sidecar-publish":
            return
        temps = list(config.control.glob(f"{sidecar.name}.tmp.*"))
        assert len(temps) == 1
        payload = json.loads(temps[0].read_text(encoding="utf-8"))
        planted = b"conflicting planted sidecar\n"
        digest = hashlib.sha256(planted).hexdigest()
        payload["contents_utf8"] = planted.decode("utf-8")
        payload["sha256"] = digest
        payload["source_identity"]["size"] = len(planted)
        payload["source_identity"]["sha256"] = digest
        sidecar.write_text(
            json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )

    monkeypatch.setattr(wrkslots, "_interrupt_for_test", plant_valid_conflict)
    args = argparse.Namespace(
        project_root=str(project),
        machine="testhost",
        slot="slot01",
        agent="codex-1",
        owner_pid=os.getpid(),
        expected_generation=1,
        from_file=str(source),
        wait_lock=0.0,
    )
    with pytest.raises(wrkslots.Refusal, match="destination already exists"):
        wrkslots._cmd_write_handoff(args)
    temps = list(config.control.glob(f"{sidecar.name}.tmp.*"))
    assert len(temps) == 1
    attempted = temps[0].read_bytes()
    planted = sidecar.read_bytes()

    monkeypatch.setattr(wrkslots, "_interrupt_for_test", lambda _point: None)
    recovered = raw_command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--discard-partial",
    )

    assert recovered.returncode == 3
    assert "temp file disagrees with durable target" in recovered.stderr
    assert temps[0].read_bytes() == attempted
    assert sidecar.read_bytes() == planted
    assert source.read_text(encoding="utf-8") == "owner-authored continuation\n"



def test_schema_one_write_handoff_matching_source_remains_manual_only(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    contents = b"slot-specific continuation\n"
    source = tmp_path / "external-handoff.md"
    source.write_bytes(contents)
    config, sidecar = _install_schema_one_handoff_sidecar(
        project, contents=contents, source="write-handoff", source_identity=None
    )

    upgraded = raw_command(
        project,
        "write-handoff",
        "slot01",
        "--agent",
        "codex-1",
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        "--from-file",
        str(source),
    )

    assert upgraded.returncode == 3
    assert "permanently manual-only" in upgraded.stderr
    payload = json.loads(sidecar.read_text(encoding="utf-8"))
    assert payload["schema"] == wrkslots.HANDOFF_SIDECAR_LEGACY_SCHEMA
    assert "source_path" not in payload
    assert not any(
        event["kind"] in {"handoff-written", "handoff-read"}
        for event in wrkslots._load_events(config)
    )
    queued = raw_command(project, "retirement-queue", "--format", "json")
    assert queued.returncode == 0, queued.stderr
    assert json.loads(queued.stdout)["pending"] == []


def test_schema_one_write_handoff_upgrade_mismatch_preserves_sidecar(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    source = tmp_path / "external-handoff.md"
    source.write_text("different continuation\n", encoding="utf-8")
    _config, sidecar = _install_schema_one_handoff_sidecar(
        project,
        contents=b"persisted continuation\n",
        source="write-handoff",
        source_identity=None,
    )
    before = sidecar.read_bytes()

    refused = raw_command(
        project,
        "write-handoff",
        "slot01",
        "--agent",
        "codex-1",
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        "--from-file",
        str(source),
    )

    assert refused.returncode == 3
    assert "permanently manual-only" in refused.stderr
    assert sidecar.read_bytes() == before


def test_schema_one_write_handoff_refusal_never_enters_upgrade_race(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    contents = b"persisted continuation\n"
    source = tmp_path / "external-handoff.md"
    source.write_bytes(contents)
    config, sidecar = _install_schema_one_handoff_sidecar(
        project, contents=contents, source="write-handoff", source_identity=None
    )
    replacement = b"replacement wins\n"

    def replace_before_upgrade(point: str) -> None:
        if point != "before-handoff-sidecar-provenance-upgrade":
            return
        payload = json.loads(sidecar.read_text(encoding="utf-8"))
        payload["contents_utf8"] = replacement.decode("utf-8")
        payload["sha256"] = hashlib.sha256(replacement).hexdigest()
        wrkslots._atomic_write_json(sidecar, payload)

    monkeypatch.setattr(wrkslots, "_interrupt_for_test", replace_before_upgrade)
    args = argparse.Namespace(
        project_root=str(project),
        machine="testhost",
        slot="slot01",
        agent="codex-1",
        owner_pid=os.getpid(),
        expected_generation=1,
        from_file=str(source),
        wait_lock=0.0,
    )
    with pytest.raises(wrkslots.Refusal, match="permanently manual-only"):
        wrkslots._cmd_write_handoff(args)

    assert json.loads(sidecar.read_text(encoding="utf-8"))["contents_utf8"] == contents.decode("utf-8")
    assert not any(
        event["kind"] == "handoff-written" for event in wrkslots._load_events(config)
    )


def test_schema_one_legacy_upgrade_requires_live_recorded_file_identity(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    legacy = checkout(project).parent / "HANDOFF.md"
    contents = b"legacy continuation\n"
    legacy.write_bytes(contents)
    _read, identity = wrkslots._read_regular_file_identity(
        legacy, "legacy test source", wrkslots.HANDOFF_BYTES_LIMIT
    )
    _config, sidecar = _install_schema_one_handoff_sidecar(
        project,
        contents=contents,
        source="legacy-slot-file",
        source_identity=identity,
    )
    legacy.unlink()

    absent = raw_command(
        project, "read-handoff", "slot01", "--coordinator-pid", str(os.getpid())
    )
    assert absent.returncode == 3
    assert "permanently manual-only" in absent.stderr
    assert json.loads(sidecar.read_text(encoding="utf-8"))["schema"] == 1

    legacy.write_bytes(contents)
    replaced = raw_command(
        project, "read-handoff", "slot01", "--coordinator-pid", str(os.getpid())
    )
    assert replaced.returncode == 3
    assert "permanently manual-only" in replaced.stderr
    assert json.loads(sidecar.read_text(encoding="utf-8"))["schema"] == 1


def test_write_handoff_refuses_stale_ancestor_git_marker(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    external = tmp_path / "stale-source"
    external.mkdir()
    source = external / "HANDOFF.md"
    source.write_text("slot-specific continuation\n", encoding="utf-8")
    (external / ".git").write_text(
        "gitdir: /definitely/missing/handoff-source-gitdir\n", encoding="utf-8"
    )

    refused = raw_command(
        project,
        "write-handoff",
        "slot01",
        "--agent",
        "codex-1",
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        "--from-file",
        str(source),
    )

    assert refused.returncode == 3
    assert "Git rejected ancestor marker" in refused.stderr
    config = wrkslots._load_config(str(project), "testhost")
    assert not wrkslots._handoff_sidecar_path(config, "slot01", 1).exists()


def test_historical_unprovenanced_read_is_manual_only_when_legacy_absent(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    legacy = checkout(project).parent / "HANDOFF.md"
    contents = b"historical continuation\n"
    legacy.write_bytes(contents)
    _read, identity = wrkslots._read_regular_file_identity(
        legacy, "legacy test source", wrkslots.HANDOFF_BYTES_LIMIT
    )
    config = wrkslots._load_config(str(project), "testhost")
    _append_historical_unprovenanced_read(config, contents)
    legacy.unlink()

    queued = raw_command(project, "retirement-queue", "--format", "json")
    assert queued.returncode == 0, queued.stderr
    assert json.loads(queued.stdout)["pending"] == []


def test_safe_reread_supersedes_historical_unprovenanced_read(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    legacy = checkout(project).parent / "HANDOFF.md"
    contents = b"safe legacy continuation\n"
    legacy.write_bytes(contents)
    _read, identity = wrkslots._read_regular_file_identity(
        legacy, "legacy test source", wrkslots.HANDOFF_BYTES_LIMIT
    )
    config = wrkslots._load_config(str(project), "testhost")
    sidecar = wrkslots._handoff_sidecar_path(config, "slot01", 1)
    _append_historical_unprovenanced_read(config, contents)
    before = json.loads(
        raw_command(project, "retirement-queue", "--format", "json").stdout
    )
    assert before["pending"] == []

    reread = raw_command(
        project, "read-handoff", "slot01", "--coordinator-pid", str(os.getpid())
    )

    assert reread.returncode == 0, reread.stderr
    reads = [
        event for event in wrkslots._load_events(config) if event["kind"] == "handoff-read"
    ]
    assert len(reads) == 3
    first_payload = reads[0]["payload"]
    latest_payload = reads[-1]["payload"]
    assert isinstance(first_payload, dict)
    assert isinstance(latest_payload, dict)
    assert "source_path" not in first_payload
    assert latest_payload["source"] == "legacy-slot-file"
    assert latest_payload["source_path"] == str(legacy.resolve())
    assert json.loads(sidecar.read_text(encoding="utf-8"))["schema"] == 2
    after = json.loads(
        raw_command(project, "retirement-queue", "--format", "json").stdout
    )
    assert len(after["pending"]) == 1
    assert after["pending"][0]["blocked_reason"] is None


def test_schema_one_write_handoff_sidecar_without_provenance_fails_closed(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    source = tmp_path / "external-handoff.md"
    source.write_text("slot-specific continuation\n", encoding="utf-8")
    written = raw_command(
        project,
        "write-handoff",
        "slot01",
        "--agent",
        "codex-1",
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        "--from-file",
        str(source),
    )
    assert written.returncode == 0, written.stderr
    config = wrkslots._load_config(str(project), "testhost")
    sidecar = wrkslots._handoff_sidecar_path(config, "slot01", 1)
    payload = json.loads(sidecar.read_text(encoding="utf-8"))
    payload["schema"] = wrkslots.HANDOFF_SIDECAR_LEGACY_SCHEMA
    del payload["source_path"]
    payload["source_identity"] = None
    sidecar.write_text(json.dumps(payload), encoding="utf-8")

    refused = raw_command(
        project,
        "read-handoff",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
    )

    assert refused.returncode == 3
    assert "schema 1 lacks auditable source provenance" in refused.stderr
    assert "permanently manual-only" in refused.stderr
    queued = raw_command(project, "retirement-queue", "--format", "json")
    assert queued.returncode == 0, queued.stderr
    assert json.loads(queued.stdout)["pending"] == []


def test_schema_one_legacy_sidecar_is_preserved_manual_only(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    legacy = checkout(project).parent / "HANDOFF.md"
    legacy.write_text("legacy continuation\n", encoding="utf-8")
    first = raw_command(
        project,
        "read-handoff",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
    )
    assert first.returncode == 0, first.stderr
    config = wrkslots._load_config(str(project), "testhost")
    sidecar = wrkslots._handoff_sidecar_path(config, "slot01", 1)
    payload = json.loads(sidecar.read_text(encoding="utf-8"))
    payload["schema"] = wrkslots.HANDOFF_SIDECAR_LEGACY_SCHEMA
    del payload["source_path"]
    sidecar.write_text(json.dumps(payload), encoding="utf-8")

    second = raw_command(
        project,
        "read-handoff",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
    )

    assert second.returncode == 3
    assert "permanently manual-only" in second.stderr
    preserved = json.loads(sidecar.read_text(encoding="utf-8"))
    assert preserved["schema"] == wrkslots.HANDOFF_SIDECAR_LEGACY_SCHEMA
    assert "source_path" not in preserved


@pytest.mark.parametrize(
    ("layout", "worktrees_directory"),
    ((None, "worktrees"), ("flat", "worktrees/slots")),
)
def test_read_handoff_migrates_legacy_without_removing_and_enqueues_once(
    tmp_path: Path, layout: str | None, worktrees_directory: str
) -> None:
    project, _repository, _remote = make_project(
        tmp_path, layout=layout, worktrees_directory=worktrees_directory
    )
    assert create(project).returncode == 0
    tree = checkout(project)
    legacy = tree / "HANDOFF.md" if layout == "flat" else tree.parent / "HANDOFF.md"
    legacy.write_text("read exactly once\n", encoding="utf-8")

    for _attempt in range(2):
        read = raw_command(
            project,
            "read-handoff",
            "slot01",
            "--coordinator-pid",
            str(os.getpid()),
        )
        assert read.returncode == 0, read.stderr
        assert "read exactly once" in read.stdout

    config = wrkslots._load_config(str(project), "testhost")
    sidecar = wrkslots._handoff_sidecar_path(config, "slot01", 1)
    assert legacy.read_text(encoding="utf-8") == "read exactly once\n"
    assert sidecar.is_file()
    queued = raw_command(project, "retirement-queue", "--format", "json")
    assert queued.returncode == 0, queued.stderr
    pending = json.loads(queued.stdout)["pending"]
    assert len(pending) == 1
    assert pending[0]["slot"] == "slot01"
    assert pending[0]["blocked_reason"] is None
    _contents, source_identity = wrkslots._read_regular_file_identity(
        legacy, "test legacy handoff", wrkslots.HANDOFF_BYTES_LIMIT
    )
    sidecar_payload = json.loads(sidecar.read_text(encoding="utf-8"))
    assert sidecar_payload["source"] == "legacy-slot-file"
    assert sidecar_payload["source_path"] == str(legacy.absolute())
    assert sidecar_payload["source_identity"] == wrkslots._regular_file_identity_to_obj(
        source_identity
    )
    events = wrkslots._load_events(config)
    payload = next(event["payload"] for event in events if event["kind"] == "handoff-read")
    assert isinstance(payload, dict)
    assert payload["source"] == "legacy-slot-file"
    assert payload["source_path"] == str(legacy.absolute())
    assert payload["source_identity"] == wrkslots._regular_file_identity_to_obj(
        source_identity
    )


def test_handoff_sidecar_disagreement_refuses_without_read_event(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    source = tmp_path / "handoff-input.md"
    source.write_text("sidecar\n", encoding="utf-8")
    written = raw_command(
        project,
        "write-handoff",
        "slot01",
        "--agent",
        "codex-1",
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        "--from-file",
        str(source),
    )
    assert written.returncode == 0, written.stderr
    (checkout(project).parent / "HANDOFF.md").write_text("legacy\n", encoding="utf-8")

    refused = raw_command(
        project,
        "read-handoff",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
    )

    assert refused.returncode == 3
    assert "disagrees with the legacy file" in refused.stderr
    config = wrkslots._load_config(str(project), "testhost")
    assert not any(
        event["kind"] == "handoff-read" for event in wrkslots._load_events(config)
    )


def test_read_handoff_flush_failure_records_no_event_or_projection(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    legacy = checkout(project).parent / "HANDOFF.md"
    legacy.write_text("must reach the reader\n", encoding="utf-8")
    config = wrkslots._load_config(str(project), "testhost")

    class BrokenFlush(io.StringIO):
        def flush(self) -> None:
            raise BrokenPipeError("reader stopped")

    monkeypatch.setattr(sys, "stdout", BrokenFlush())
    args = argparse.Namespace(
        project_root=str(project),
        machine="testhost",
        slot="slot01",
        coordinator_pid=os.getpid(),
        coordinator_authorized=False,
        adopt_legacy_change=False,
        expected_generation=None,
        wait_lock=0.0,
    )
    with pytest.raises(wrkslots.Refusal, match="could not be flushed"):
        wrkslots._cmd_read_handoff(args)

    assert not any(
        event["kind"] == "handoff-read" for event in wrkslots._load_events(config)
    )
    assert not wrkslots._handoff_sidecar_path(config, "slot01", 1).exists()
    assert legacy.read_text(encoding="utf-8") == "must reach the reader\n"


def test_read_handoff_revalidates_after_unlocked_output(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    legacy = checkout(project).parent / "HANDOFF.md"
    legacy.write_text("displayed bytes\n", encoding="utf-8")
    config = wrkslots._load_config(str(project), "testhost")

    def change_after_output(_path: Path, _rendered: str) -> None:
        legacy.write_text("concurrent bytes\n", encoding="utf-8")

    monkeypatch.setattr(wrkslots, "_write_and_flush_handoff", change_after_output)
    args = argparse.Namespace(
        project_root=str(project),
        machine="testhost",
        slot="slot01",
        coordinator_pid=os.getpid(),
        coordinator_authorized=False,
        adopt_legacy_change=False,
        expected_generation=None,
        wait_lock=0.0,
    )
    with pytest.raises(wrkslots.Refusal, match="changed while it was displayed"):
        wrkslots._cmd_read_handoff(args)

    assert not any(
        event["kind"] == "handoff-read" for event in wrkslots._load_events(config)
    )
    assert not wrkslots._handoff_sidecar_path(config, "slot01", 1).exists()
    assert legacy.read_text(encoding="utf-8") == "concurrent bytes\n"


def test_same_byte_legacy_replacement_requires_manual_review(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    legacy = checkout(project).parent / "HANDOFF.md"
    legacy.write_text("same bytes, new identity\n", encoding="utf-8")
    first = raw_command(
        project, "read-handoff", "slot01", "--coordinator-pid", str(os.getpid())
    )
    assert first.returncode == 0, first.stderr
    config = wrkslots._load_config(str(project), "testhost")
    sidecar = wrkslots._handoff_sidecar_path(config, "slot01", 1)
    sidecar_before = sidecar.read_bytes()
    old_identity = legacy.stat().st_ino
    legacy.unlink()
    legacy.write_text("same bytes, new identity\n", encoding="utf-8")
    assert legacy.stat().st_ino != old_identity

    refused = raw_command(
        project, "read-handoff", "slot01", "--coordinator-pid", str(os.getpid())
    )
    assert refused.returncode == 3
    assert "identity changed even though its bytes are unchanged" in refused.stderr
    adoption = raw_command(
        project,
        "read-handoff",
        "slot01",
        "--coordinator-authorized",
        "--coordinator-pid",
        str(os.getpid()),
        "--adopt-legacy-change",
        "--expected-generation",
        "1",
    )
    assert adoption.returncode == 3
    assert "sidecars are immutable" in adoption.stderr
    assert sidecar.read_bytes() == sidecar_before
    assert legacy.read_text(encoding="utf-8") == "same bytes, new identity\n"
    assert len(
        [event for event in wrkslots._load_events(config) if event["kind"] == "handoff-read"]
    ) == 2


@pytest.mark.parametrize(
    "read_interrupt",
    ("after-handoff-read-event", "after-handoff-sidecar-write"),
)
def test_handoff_sidecar_write_and_migration_are_retry_idempotent(
    tmp_path: Path, read_interrupt: str
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    source = tmp_path / "handoff-input.md"
    source.write_text("durable owner handoff\n", encoding="utf-8")
    write_args = (
        "write-handoff",
        "slot01",
        "--agent",
        "codex-1",
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        "--from-file",
        str(source),
    )
    interrupted_write = raw_command(
        project,
        *write_args,
        env={"WRKSLOTS_TEST_INTERRUPT": "after-handoff-sidecar-write"},
    )
    assert interrupted_write.returncode == 86
    retried_write = raw_command(project, *write_args)
    assert retried_write.returncode == 0, retried_write.stderr

    assert create(
        project,
        slot="slot02",
        agent="codex-2",
        branch="codex/second",
    ).returncode == 0
    legacy = checkout(project, slot="slot02").parent / "HANDOFF.md"
    legacy.write_text("durable migrated handoff\n", encoding="utf-8")
    read_args = (
        "read-handoff",
        "slot02",
        "--coordinator-pid",
        str(os.getpid()),
    )
    interrupted_read = raw_command(
        project,
        *read_args,
        env={"WRKSLOTS_TEST_INTERRUPT": read_interrupt},
    )
    assert interrupted_read.returncode == 86
    retried_read = raw_command(project, *read_args)
    assert retried_read.returncode == 0, retried_read.stderr
    config = wrkslots._load_config(str(project), "testhost")
    slot02_reads = [
        event
        for event in wrkslots._load_events(config)
        if event["kind"] == "handoff-read"
        and wrkslots._as_mapping(event["payload"], "test payload").get("slot")
        == "slot02"
    ]
    assert len(slot02_reads) == 2
    assert [
        wrkslots._as_mapping(event["payload"], "handoff read payload")[
            "retirement_enqueued"
        ]
        for event in slot02_reads
    ] == [False, True]


def test_historical_unprovenanced_read_refuses_manual_remove_until_safe_reread(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    legacy = checkout(project).parent / "HANDOFF.md"
    contents = b"historical read\n"
    legacy.write_bytes(contents)
    config = wrkslots._load_config(str(project), "testhost")
    _append_historical_unprovenanced_read(config, contents)
    assert json.loads(
        raw_command(project, "retirement-queue", "--format", "json").stdout
    )["pending"] == []
    mark_owner_dead(project)
    set_liveness(project, "dead")
    monkeypatch.setattr(wrkslots, "_assert_slot_unused", lambda *_args, **_kwargs: None)

    refused = raw_command(project, "remove", "slot01", "--coordinator-pid", str(os.getpid()), "--expected-generation", "1")
    assert refused.returncode == 3
    assert "contains an unread HANDOFF.md" in refused.stderr
    assert legacy.exists()

    reread = raw_command(
        project, "read-handoff", "slot01", "--coordinator-pid", str(os.getpid())
    )
    assert reread.returncode == 0, reread.stderr
    removed = raw_command(project, "remove", "slot01", "--coordinator-pid", str(os.getpid()), "--expected-generation", "1")
    assert removed.returncode == 0, removed.stderr
    assert not checkout(project).parent.exists()


def test_tracked_flat_handoff_with_historical_read_refuses_manual_remove(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project, repository, _remote = make_project(
        tmp_path, worktrees_directory="worktrees/slots", layout="flat"
    )
    broadcast = repository / "HANDOFF.md"
    broadcast.write_text("shared repository instructions\n", encoding="utf-8")
    git(repository, "add", "HANDOFF.md")
    git(repository, "commit", "-m", "add shared instructions")
    git(repository, "push", "origin", "main")
    assert create(project).returncode == 0
    handoff = checkout(project) / "HANDOFF.md"
    config = wrkslots._load_config(str(project), "testhost")
    _append_historical_unprovenanced_read(config, handoff.read_bytes())
    mark_owner_dead(project)
    set_liveness(project, "dead")
    monkeypatch.setattr(wrkslots, "_assert_slot_unused", lambda *_args, **_kwargs: None)

    refused = raw_command(project, "remove", "slot01", "--coordinator-pid", str(os.getpid()), "--expected-generation", "1")

    assert refused.returncode == 3
    assert "tracked by Git" in refused.stderr
    assert checkout(project).is_dir()
    assert handoff.is_file()



def test_retire_pending_uses_remove_and_cleans_sidecar(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    commit_task(repository, checkout(project), "codex/task")
    source = tmp_path / "handoff-input.md"
    source.write_text("retire safely\n", encoding="utf-8")
    assert raw_command(
        project,
        "write-handoff",
        "slot01",
        "--agent",
        "codex-1",
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        "--from-file",
        str(source),
    ).returncode == 0
    assert finish(project).returncode == 0
    assert raw_command(
        project,
        "read-handoff",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
    ).returncode == 0
    mark_owner_dead(project)
    set_liveness(project, "dead")
    monkeypatch.setattr(wrkslots, "_assert_slot_unused", lambda *_args, **_kwargs: None)
    config = wrkslots._load_config(str(project), "testhost")
    sidecar = wrkslots._handoff_sidecar_path(config, "slot01", 1)

    code = wrkslots.main(
        [
            "--project-root",
            str(project),
            "retire-pending",
            "--limit",
            "1",
            "--coordinator-pid",
            str(os.getpid()),
            "--format",
            "json",
        ]
    )

    assert code == 0
    payload = json.loads(capsys.readouterr().out)
    assert payload["removed"] == [
        {
            "generation": 1,
            "machine": "testhost",
            "outcome": "removed",
            "reason": "ordinary remove completed",
            "sha256": hashlib.sha256(b"retire safely\n").hexdigest(),
            "slot": "slot01",
        }
    ]
    assert payload["retained"] == []
    assert not checkout(project).exists()
    assert not sidecar.exists()


def test_recover_completes_legacy_handoff_quarantine_event(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    commit_task(repository, checkout(project), "codex/task")
    legacy = checkout(project).parent / "HANDOFF.md"
    legacy.write_text("retained across quarantine crash\n", encoding="utf-8")
    assert raw_command(
        project, "read-handoff", "slot01", "--coordinator-pid", str(os.getpid())
    ).returncode == 0
    assert finish(project).returncode == 0
    mark_owner_dead(project)
    set_liveness(project, "dead")

    interrupted = raw_command(
        project,
        "remove",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        env={"WRKSLOTS_TEST_INTERRUPT": "after-legacy-handoff-quarantine"},
    )

    assert interrupted.returncode == 86
    config = wrkslots._load_config(str(project), "testhost")
    retired = list(config.control.glob("HANDOFF-LEGACY-RETIRED.*"))
    assert len(retired) == 1
    assert not any(
        event["kind"] == "handoff-removed" for event in wrkslots._load_events(config)
    )
    recovered = raw_command(
        project, "recover", "--coordinator-pid", str(os.getpid())
    )
    assert recovered.returncode == 0, recovered.stderr
    removed = [
        event for event in wrkslots._load_events(config) if event["kind"] == "handoff-removed"
    ]
    assert len(removed) == 1
    payload = removed[0]["payload"]
    assert isinstance(payload, dict)
    assert payload["retired_path"] == retired[0].name
    assert retired[0].is_file()
    assert not active_slots(project)



def test_legacy_handoff_quarantine_preserves_path_replacement(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    commit_task(repository, checkout(project), "codex/task")
    legacy = checkout(project).parent / "HANDOFF.md"
    legacy.write_text("original legacy handoff\n", encoding="utf-8")
    assert raw_command(
        project, "read-handoff", "slot01", "--coordinator-pid", str(os.getpid())
    ).returncode == 0
    assert finish(project).returncode == 0
    mark_owner_dead(project)
    set_liveness(project, "dead")
    monkeypatch.setattr(wrkslots, "_assert_slot_unused", lambda *_args, **_kwargs: None)

    def replace_at_boundary(point: str) -> None:
        if point != "before-legacy-handoff-quarantine":
            return
        fenced = list(slots_directory(project).glob(".slot01.fenced.*"))
        assert len(fenced) == 1
        selected = fenced[0] / "HANDOFF.md"
        replacement = fenced[0] / "replacement.tmp"
        replacement.write_text("replacement must survive\n", encoding="utf-8")
        os.replace(replacement, selected)

    monkeypatch.setattr(wrkslots, "_interrupt_for_test", replace_at_boundary)
    code = wrkslots.main(
        [
            "--project-root",
            str(project),
            "remove",
            "slot01",
            "--coordinator-pid",
            str(os.getpid()),
            "--expected-generation",
            "1",
        ]
    )

    assert code == 3
    assert "replacement was preserved" in capsys.readouterr().err
    fenced = list(slots_directory(project).glob(".slot01.fenced.*"))
    assert len(fenced) == 1
    assert (fenced[0] / "HANDOFF.md").read_text(encoding="utf-8") == (
        "replacement must survive\n"
    )
    assert (control_directory(project) / "ACTIVE.testhost.journal").is_file()



def test_sidecar_cleanup_preserves_canonical_path_recreation(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    commit_task(repository, checkout(project), "codex/task")
    source = tmp_path / "handoff-input.md"
    source.write_text("original sidecar\n", encoding="utf-8")
    assert raw_command(
        project,
        "write-handoff",
        "slot01",
        "--agent",
        "codex-1",
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        "--from-file",
        str(source),
    ).returncode == 0
    assert finish(project).returncode == 0
    assert raw_command(
        project,
        "read-handoff",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
    ).returncode == 0
    mark_owner_dead(project)
    set_liveness(project, "dead")
    monkeypatch.setattr(wrkslots, "_assert_slot_unused", lambda *_args, **_kwargs: None)
    config = wrkslots._load_config(str(project), "testhost")
    sidecar = wrkslots._handoff_sidecar_path(config, "slot01", 1)
    retired_path: Path | None = None

    def recreate_canonical_after_retire(point: str) -> None:
        nonlocal retired_path
        if point == "after-handoff-sidecar-quarantine":
            matches = list(config.control.glob("HANDOFF-RETIRED.*"))
            assert len(matches) == 1
            retired_path = matches[0]
            sidecar.write_text("replacement must survive\n", encoding="utf-8")

    monkeypatch.setattr(wrkslots, "_interrupt_for_test", recreate_canonical_after_retire)
    code = wrkslots.main(
        [
            "--project-root",
            str(project),
            "remove",
            "slot01",
            "--coordinator-pid",
            str(os.getpid()),
            "--expected-generation",
            "1",
        ]
    )

    assert code == 3
    assert "preserved both identities" in capsys.readouterr().err
    assert retired_path is not None
    assert retired_path.is_file()
    assert sidecar.read_text(encoding="utf-8") == "replacement must survive\n"
    assert not wrkslots._load_active(config).slots
    assert wrkslots._load_archive(config).records[-1]["slot"] == "slot01"
    assert (config.control / "ACTIVE.testhost.journal").is_file()

    sidecar.unlink()
    monkeypatch.setattr(wrkslots, "_interrupt_for_test", lambda _point: None)
    assert (
        wrkslots.main(
            [
                "--project-root",
                str(project),
                "recover",
                "--coordinator-authorized",
                "--coordinator-pid",
                str(os.getpid()),
            ]
        )
        == 0
    )
    assert retired_path.is_file()
    assert not (config.control / "ACTIVE.testhost.journal").exists()


def test_sidecar_cleanup_noreplace_preserves_planted_retired_destination(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    commit_task(repository, checkout(project), "codex/task")
    source = tmp_path / "handoff-input.md"
    source.write_text("original sidecar\n", encoding="utf-8")
    assert raw_command(
        project,
        "write-handoff",
        "slot01",
        "--agent",
        "codex-1",
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        "--from-file",
        str(source),
    ).returncode == 0
    assert finish(project).returncode == 0
    assert raw_command(
        project,
        "read-handoff",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
    ).returncode == 0
    mark_owner_dead(project)
    set_liveness(project, "dead")
    monkeypatch.setattr(wrkslots, "_assert_slot_unused", lambda *_args, **_kwargs: None)
    config = wrkslots._load_config(str(project), "testhost")
    sidecar_path = wrkslots._handoff_sidecar_path(config, "slot01", 1)
    record = wrkslots._find_record(wrkslots._load_active(config), "slot01")
    sidecar = wrkslots._load_handoff_sidecar(config, record)
    assert sidecar is not None
    retired = wrkslots._retired_handoff_path(sidecar)

    def plant_destination_before_move(point: str) -> None:
        if point == "before-handoff-sidecar-retire":
            retired.write_text("planted destination\n", encoding="utf-8")

    monkeypatch.setattr(wrkslots, "_interrupt_for_test", plant_destination_before_move)
    code = wrkslots.main(
        [
            "--project-root",
            str(project),
            "remove",
            "slot01",
            "--coordinator-pid",
            str(os.getpid()),
            "--expected-generation",
            "1",
        ]
    )

    assert code == 3
    assert "destination already exists" in capsys.readouterr().err
    assert sidecar_path.is_file()
    assert retired.read_text(encoding="utf-8") == "planted destination\n"
    assert (config.control / "ACTIVE.testhost.journal").is_file()

    retired.unlink()
    monkeypatch.setattr(wrkslots, "_interrupt_for_test", lambda _point: None)
    assert (
        wrkslots.main(
            [
                "--project-root",
                str(project),
                "recover",
                "--coordinator-authorized",
                "--coordinator-pid",
                str(os.getpid()),
            ]
        )
        == 0
    )
    assert not sidecar_path.exists()
    assert retired.is_file()
    assert not (config.control / "ACTIVE.testhost.journal").exists()


def test_retire_pending_retains_live_slot_and_queue_entry(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    legacy = checkout(project).parent / "HANDOFF.md"
    legacy.write_text("not yet dead\n", encoding="utf-8")
    assert raw_command(
        project,
        "read-handoff",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
    ).returncode == 0
    assert create(
        project,
        slot="slot02",
        agent="codex-2",
        branch="codex/second",
    ).returncode == 0
    second = checkout(project, slot="slot02").parent / "HANDOFF.md"
    second.write_text("also not yet dead\n", encoding="utf-8")
    assert raw_command(
        project,
        "read-handoff",
        "slot02",
        "--coordinator-pid",
        str(os.getpid()),
    ).returncode == 0

    code = wrkslots.main(
        [
            "--project-root",
            str(project),
            "retire-pending",
            "--limit",
            "1",
            "--coordinator-pid",
            str(os.getpid()),
            "--format",
            "json",
        ]
    )

    assert code == 0
    payload = json.loads(capsys.readouterr().out)
    assert payload["removed"] == []
    assert payload["retained"][0]["slot"] == "slot01"
    assert "registered liveness authority reports owner alive" in payload["retained"][0]["reason"]
    assert payload["deferred"][0]["slot"] == "slot02"
    assert payload["deferred"][0]["outcome"] == "deferred"
    assert payload["deferred"][0]["reason"] == "batch limit"
    queued = raw_command(project, "retirement-queue", "--format", "json")
    assert len(json.loads(queued.stdout)["pending"]) == 2

    second_code = wrkslots.main(
        [
            "--project-root",
            str(project),
            "retire-pending",
            "--limit",
            "1",
            "--coordinator-pid",
            str(os.getpid()),
            "--format",
            "json",
        ]
    )
    assert second_code == 0
    second_payload = json.loads(capsys.readouterr().out)
    assert second_payload["retained"][0]["slot"] == "slot02"
    assert second_payload["deferred"][0]["slot"] == "slot01"


def test_retire_pending_defers_superseded_candidate_inside_remove_lock(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    legacy = checkout(project).parent / "HANDOFF.md"
    legacy.write_text("queued once\n", encoding="utf-8")
    assert raw_command(
        project,
        "read-handoff",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
    ).returncode == 0
    config = wrkslots._load_config(str(project), "testhost")
    digest = hashlib.sha256(b"queued once\n").hexdigest()
    original_remove = wrkslots._cmd_remove

    def supersede_then_remove(
        args: argparse.Namespace,
        *,
        private_cleanup: wrkslots._PrivateCleanupContext | None = None,
        validation_removal_proof: wrkslots._ValidationRemovalProof | None = None,
        emit: bool = True,
    ) -> int:
        wrkslots._write_event_file(
            config,
            "testhost",
            "handoff-read",
            {
                "slot": "slot01",
                "generation": 1,
                "sha256": digest,
                "retirement_enqueued": False,
            },
        )
        return original_remove(
            args,
            private_cleanup=private_cleanup,
            validation_removal_proof=validation_removal_proof,
            emit=emit,
        )

    monkeypatch.setattr(wrkslots, "_cmd_remove", supersede_then_remove)
    code = wrkslots.main(
        [
            "--project-root",
            str(project),
            "retire-pending",
            "--limit",
            "1",
            "--coordinator-pid",
            str(os.getpid()),
            "--format",
            "json",
        ]
    )

    assert code == 0
    payload = json.loads(capsys.readouterr().out)
    assert payload["removed"] == []
    assert payload["retained"] == []
    assert payload["deferred"][0]["slot"] == "slot01"
    assert "superseded" in payload["deferred"][0]["reason"]
    assert checkout(project).is_dir()


def test_retirement_attempt_event_is_crash_safe_and_rotates_queue(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    assert create(
        project, slot="slot02", agent="codex-2", branch="codex/second"
    ).returncode == 0
    for slot in ("slot01", "slot02"):
        (checkout(project, slot=slot).parent / "HANDOFF.md").write_text(
            f"queue {slot}\n", encoding="utf-8"
        )
        assert raw_command(
            project,
            "read-handoff",
            slot,
            "--coordinator-pid",
            str(os.getpid()),
        ).returncode == 0

    class Interrupted(RuntimeError):
        pass

    def interrupt(point: str) -> None:
        if point == "after-retirement-attempt":
            raise Interrupted

    monkeypatch.setattr(wrkslots, "_interrupt_for_test", interrupt)
    with pytest.raises(Interrupted):
        wrkslots.main(
            [
                "--project-root",
                str(project),
                "retire-pending",
                "--limit",
                "1",
                "--coordinator-pid",
                str(os.getpid()),
                "--format",
                "json",
            ]
        )

    config = wrkslots._load_config(str(project), "testhost")
    assert {record.slot for record in wrkslots._load_active(config).slots} == {
        "slot01",
        "slot02",
    }
    candidates = wrkslots._retirement_candidates(
        config, wrkslots._load_active(config), wrkslots._load_events(config)
    )
    assert [candidate.slot for candidate in candidates] == ["slot02", "slot01"]
    attempts = [
        event
        for event in wrkslots._load_events(config)
        if event["kind"] == "retirement-attempted"
    ]
    assert len(attempts) == 1
    payload = wrkslots._as_mapping(attempts[0]["payload"], "attempt payload")
    assert payload["slot"] == "slot01"
    assert payload["reason"] == "selected by bounded retirement queue"


def test_same_digest_reenqueue_has_fresh_attempt_identity(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    legacy = checkout(project).parent / "HANDOFF.md"
    legacy.write_text("same bytes\n", encoding="utf-8")
    assert raw_command(
        project,
        "read-handoff",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
    ).returncode == 0
    assert (
        wrkslots.main(
            [
                "--project-root",
                str(project),
                "retire-pending",
                "--limit",
                "1",
                "--coordinator-pid",
                str(os.getpid()),
                "--format",
                "json",
            ]
        )
        == 0
    )
    capsys.readouterr()
    config = wrkslots._load_config(str(project), "testhost")
    first = wrkslots._retirement_candidates(
        config, wrkslots._load_active(config), wrkslots._load_events(config)
    )[0]
    assert first.last_attempt_sequence is not None
    wrkslots._write_event_file(
        config,
        "testhost",
        "handoff-read",
        {
            "slot": "slot01",
            "generation": 1,
            "sha256": first.sha256,
            "retirement_enqueued": False,
        },
    )
    assert raw_command(
        project,
        "read-handoff",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
    ).returncode == 0

    refreshed = wrkslots._retirement_candidates(
        config, wrkslots._load_active(config), wrkslots._load_events(config)
    )[0]
    assert refreshed.sha256 == first.sha256
    assert refreshed.event_sequence > first.event_sequence
    assert refreshed.last_attempt_sequence is None


def test_retire_pending_reclassifies_post_completion_refusal_as_removed(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    commit_task(repository, checkout(project), "codex/task")
    source = tmp_path / "handoff-input.md"
    source.write_text("retire then report\n", encoding="utf-8")
    assert raw_command(
        project,
        "write-handoff",
        "slot01",
        "--agent",
        "codex-1",
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        "--from-file",
        str(source),
    ).returncode == 0
    assert finish(project).returncode == 0
    assert raw_command(
        project,
        "read-handoff",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
    ).returncode == 0
    mark_owner_dead(project)
    set_liveness(project, "dead")
    monkeypatch.setattr(wrkslots, "_assert_slot_unused", lambda *_args, **_kwargs: None)
    original_remove = wrkslots._cmd_remove

    def remove_then_refuse(
        args: argparse.Namespace,
        *,
        private_cleanup: wrkslots._PrivateCleanupContext | None = None,
        validation_removal_proof: wrkslots._ValidationRemovalProof | None = None,
        emit: bool = True,
    ) -> int:
        original_remove(
            args,
            private_cleanup=private_cleanup,
            validation_removal_proof=validation_removal_proof,
            emit=emit,
        )
        raise wrkslots.Refusal("observer lost the successful return")

    monkeypatch.setattr(wrkslots, "_cmd_remove", remove_then_refuse)
    code = wrkslots.main(
        [
            "--project-root",
            str(project),
            "retire-pending",
            "--limit",
            "1",
            "--coordinator-pid",
            str(os.getpid()),
            "--format",
            "json",
        ]
    )

    assert code == 0
    payload = json.loads(capsys.readouterr().out)
    assert payload["retained"] == []
    assert payload["removed"][0]["slot"] == "slot01"
    assert "before reporting" in payload["removed"][0]["reason"]


@pytest.mark.parametrize("failure_type", ("state", "lock"))
def test_retire_pending_distinguishes_corrupt_state_from_lock_deferral(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    failure_type: str,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    legacy = checkout(project).parent / "HANDOFF.md"
    legacy.write_text("queued\n", encoding="utf-8")
    assert raw_command(
        project,
        "read-handoff",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
    ).returncode == 0

    def fail_remove(_args: argparse.Namespace, **_kwargs: object) -> int:
        if failure_type == "state":
            raise wrkslots.StateError("corrupt mutation journal")
        raise wrkslots._LockBusy("state lock is busy")

    monkeypatch.setattr(wrkslots, "_cmd_remove", fail_remove)
    code = wrkslots.main(
        [
            "--project-root",
            str(project),
            "retire-pending",
            "--limit",
            "1",
            "--coordinator-pid",
            str(os.getpid()),
            "--format",
            "json",
        ]
    )
    output = capsys.readouterr()
    if failure_type == "state":
        assert code == 3
        payload = json.loads(output.out)
        assert payload["removed"] == []
        assert payload["retained"] == []
        assert payload["recovery_required"] is True
        assert payload["could_not_determine"][0]["outcome"] == "could-not-determine"
        assert payload["could_not_determine"][0]["recovery_required"] is True
    else:
        assert code == 0
        payload = json.loads(output.out)
        assert payload["retained"] == []
        assert payload["deferred"][0]["reason"] == "state lock is busy"


def test_retire_pending_uses_one_end_to_end_lock_deadline(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    for slot, agent, branch in (
        ("slot01", "codex-1", "codex/one"),
        ("slot02", "codex-2", "codex/two"),
    ):
        assert create(project, slot=slot, agent=agent, branch=branch).returncode == 0
        (checkout(project, slot=slot).parent / "HANDOFF.md").write_text(
            f"queue {slot}\n", encoding="utf-8"
        )
        assert raw_command(
            project,
            "read-handoff",
            slot,
            "--coordinator-pid",
            str(os.getpid()),
        ).returncode == 0
    observed: list[tuple[float, float]] = []

    def busy_remove(
        args: argparse.Namespace,
        *,
        private_cleanup: wrkslots._PrivateCleanupContext | None = None,
        validation_removal_proof: wrkslots._ValidationRemovalProof | None = None,
        emit: bool = True,
    ) -> int:
        del private_cleanup, validation_removal_proof, emit
        observed.append((args._wait_deadline, args.wait_lock))
        raise wrkslots._LockBusy("state lock is busy")

    monkeypatch.setattr(wrkslots, "_cmd_remove", busy_remove)
    assert (
        wrkslots.main(
            [
                "--project-root",
                str(project),
                "--wait-lock",
                "30",
                "retire-pending",
                "--limit",
                "2",
                "--coordinator-pid",
                str(os.getpid()),
                "--format",
                "json",
            ]
        )
        == 0
    )
    payload = json.loads(capsys.readouterr().out)
    assert len(payload["deferred"]) == 2
    assert len(observed) == 2
    assert observed[0][0] == observed[1][0]
    assert 0 <= observed[1][1] <= observed[0][1] <= 30


def test_retire_pending_json_preserves_success_before_partial_state(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    for slot, agent, branch in (
        ("slot01", "codex-1", "codex/one"),
        ("slot02", "codex-2", "codex/two"),
    ):
        assert create(project, slot=slot, agent=agent, branch=branch).returncode == 0
        commit_task(repository, checkout(project, slot=slot), branch)
        source = tmp_path / f"{slot}.md"
        source.write_text(f"retire {slot}\n", encoding="utf-8")
        assert raw_command(
            project,
            "write-handoff",
            slot,
            "--agent",
            agent,
            "--owner-pid",
            str(os.getpid()),
            "--expected-generation",
            "1",
            "--from-file",
            str(source),
        ).returncode == 0
        assert finish(project, slot=slot, agent=agent).returncode == 0
        assert raw_command(
            project,
            "read-handoff",
            slot,
            "--coordinator-pid",
            str(os.getpid()),
        ).returncode == 0
        mark_owner_dead(project, slot=slot)
    set_liveness(project, "dead")
    monkeypatch.setattr(wrkslots, "_assert_slot_unused", lambda *_args, **_kwargs: None)
    config = wrkslots._load_config(str(project), "testhost")
    original_remove = wrkslots._cmd_remove

    def remove_then_corrupt(
        args: argparse.Namespace,
        *,
        private_cleanup: wrkslots._PrivateCleanupContext | None = None,
        validation_removal_proof: wrkslots._ValidationRemovalProof | None = None,
        emit: bool = True,
    ) -> int:
        if args.slot == "slot02":
            partial = config.control / "ACTIVE.testhost.json.tmp.injected"
            partial.write_text("partial\n", encoding="utf-8")
            raise wrkslots.StateError(f"partial state update exists: {partial}")
        return original_remove(
            args,
            private_cleanup=private_cleanup,
            validation_removal_proof=validation_removal_proof,
            emit=emit,
        )

    monkeypatch.setattr(wrkslots, "_cmd_remove", remove_then_corrupt)
    code = wrkslots.main(
        [
            "--project-root",
            str(project),
            "retire-pending",
            "--limit",
            "2",
            "--coordinator-pid",
            str(os.getpid()),
            "--format",
            "json",
        ]
    )

    assert code == 3
    payload = json.loads(capsys.readouterr().out)
    assert [row["slot"] for row in payload["removed"]] == ["slot01"]
    assert payload["retained"] == []
    assert payload["deferred"] == []
    assert payload["recovery_required"] is True
    assert payload["could_not_determine"] == [
        {
            "generation": 1,
            "machine": "testhost",
            "outcome": "could-not-determine",
            "reason": f"partial state update exists: {config.control / 'ACTIVE.testhost.json.tmp.injected'}",
            "recovery_required": True,
            "sha256": hashlib.sha256(b"retire slot02\n").hexdigest(),
            "slot": "slot02",
        }
    ]


def test_finish_still_refuses_a_legacy_flat_handoff(tmp_path: Path) -> None:
    project, repository, _remote = make_project(
        tmp_path, worktrees_directory="worktrees/slots", layout="flat"
    )
    assert create(project).returncode == 0
    commit_task(repository, checkout(project), "codex/task")
    legacy = checkout(project) / "HANDOFF.md"
    legacy.write_text("legacy untracked work\n", encoding="utf-8")

    refused = finish(project)

    assert refused.returncode == 3
    assert "dirty or has untracked/ignored files" in refused.stderr
    assert legacy.read_text(encoding="utf-8") == "legacy untracked work\n"


def test_read_handoff_ignores_unrelated_absent_ownerless_row(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    handoff = checkout(project).parent / "HANDOFF.md"
    contents = b"Continue from the exact recorded commit.\n"
    handoff.write_bytes(contents)

    unrelated_made = create(
        project,
        slot="unrelated",
        agent="unrelated-agent",
        branch="agent/unrelated",
        bind_owner=False,
    )
    assert unrelated_made.returncode == 0, unrelated_made.stderr
    config = wrkslots._load_config(str(project), "testhost")
    unrelated = wrkslots._find_record(wrkslots._load_active(config), "unrelated")
    shutil.rmtree(wrkslots._slot_directory(config, unrelated.slot, "agent"))
    assert unrelated.owner is None
    assert unrelated.checkouts[0].path in {
        path.relative_to(project).as_posix()
        for path in wrkslots._GitVcs().listed_worktrees(repository)
    }
    before = wrkslots._record_to_obj(unrelated)

    read = raw_command(
        project,
        "read-handoff",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
    )

    assert read.returncode == 0, read.stderr
    assert "Continue from the exact recorded commit." in read.stdout
    after = wrkslots._find_record(wrkslots._load_active(config), "unrelated")
    assert wrkslots._record_to_obj(after) == before
    events = wrkslots._load_events(config)
    read_events = [event for event in events if event["kind"] == "handoff-read"]
    assert len(read_events) == 2
    assert [
        wrkslots._as_mapping(event["payload"], "handoff read payload")[
            "retirement_enqueued"
        ]
        for event in read_events
    ] == [False, True]
    payload = wrkslots._as_mapping(read_events[-1]["payload"], "handoff read payload")
    assert payload["slot"] == "slot01"
    assert payload["sha256"] == hashlib.sha256(contents).hexdigest()


def test_read_handoff_ignores_unrelated_unavailable_repository(
    tmp_path: Path,
) -> None:
    project, _repository, remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    handoff = checkout(project).parent / "HANDOFF.md"
    contents = b"Do not let unrelated repository drift hide this handoff.\n"
    handoff.write_bytes(contents)

    unrelated_repository = project / "repo-unrelated"
    subprocess.run(
        ["git", "clone", str(remote), str(unrelated_repository)],
        check=True,
        capture_output=True,
        text=True,
    )
    unrelated_made = create(
        project,
        slot="unrelated",
        agent="unrelated-agent",
        branch="agent/unrelated-repository",
        repository_name="repo-unrelated",
    )
    assert unrelated_made.returncode == 0, unrelated_made.stderr
    config = wrkslots._load_config(str(project), "testhost")
    unrelated = wrkslots._find_record(wrkslots._load_active(config), "unrelated")
    before = wrkslots._record_to_obj(unrelated)
    shutil.rmtree(unrelated_repository)

    read = raw_command(
        project,
        "read-handoff",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
    )

    assert read.returncode == 0, read.stderr
    assert contents.decode("utf-8").strip() in read.stdout
    after = wrkslots._find_record(
        wrkslots._load_active(config, require_repository=False),
        "unrelated",
    )
    assert wrkslots._record_to_obj(after) == before
    events = wrkslots._load_events(config)
    read_events = [event for event in events if event["kind"] == "handoff-read"]
    assert len(read_events) == 2
    assert [
        wrkslots._as_mapping(event["payload"], "handoff read payload")[
            "retirement_enqueued"
        ]
        for event in read_events
    ] == [False, True]
    payload = wrkslots._as_mapping(read_events[-1]["payload"], "handoff read payload")
    assert payload["slot"] == "slot01"
    assert payload["sha256"] == hashlib.sha256(contents).hexdigest()


def test_read_handoff_bootstraps_legacy_events_without_unrelated_repository(
    tmp_path: Path,
) -> None:
    project, _repository, remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    handoff = checkout(project).parent / "HANDOFF.md"
    contents = b"A legacy registry still needs an exact acknowledgement.\n"
    handoff.write_bytes(contents)

    unrelated_repository = project / "repo-unrelated"
    subprocess.run(
        ["git", "clone", str(remote), str(unrelated_repository)],
        check=True,
        capture_output=True,
        text=True,
    )
    unrelated_made = create(
        project,
        slot="unrelated",
        agent="unrelated-agent",
        branch="agent/unrelated-legacy-repository",
        repository_name="repo-unrelated",
    )
    assert unrelated_made.returncode == 0, unrelated_made.stderr
    config = wrkslots._load_config(str(project), "testhost")
    shutil.rmtree(unrelated_repository)
    shutil.rmtree(wrkslots._event_directory(config))

    read = raw_command(
        project,
        "read-handoff",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
    )

    digest = hashlib.sha256(contents).hexdigest()
    assert read.returncode == 0, read.stderr
    assert read.stdout == (
        f"--- {handoff} ---\n"
        f"{contents.decode('utf-8')}"
        f"--- end {handoff} ---\n"
        f"recorded HANDOFF.md read slot=slot01 sha256={digest}\n"
    )
    events = wrkslots._load_events(config)
    assert [event["kind"] for event in events] == [
        "state-imported",
        "handoff-read",
        "handoff-read",
    ]
    first_payload = wrkslots._as_mapping(
        events[1]["payload"], "handoff read payload"
    )
    payload = wrkslots._as_mapping(events[2]["payload"], "handoff read payload")
    assert first_payload["retirement_enqueued"] is False
    assert payload["retirement_enqueued"] is True
    assert payload["slot"] == "slot01"
    assert payload["sha256"] == digest


def test_read_handoff_legacy_bootstrap_refuses_bad_target_before_output(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    handoff = checkout(project).parent / "HANDOFF.md"
    handoff.write_text("Do not acknowledge malformed target authority.\n", encoding="utf-8")
    config = wrkslots._load_config(str(project), "testhost")
    shutil.rmtree(wrkslots._event_directory(config))
    state_path = wrkslots._active_path(config)
    state = json.loads(state_path.read_text(encoding="utf-8"))
    state["slots"][0]["checkouts"][0]["repository"] = "missing-target-repository"
    state_path.write_text(json.dumps(state, indent=2) + "\n", encoding="utf-8")
    before_state = state_path.read_bytes()

    refused = raw_command(
        project,
        "read-handoff",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
    )

    assert refused.returncode == 3
    assert "source repository" in refused.stderr
    assert "missing-target-repository" in refused.stderr
    assert refused.stdout == ""
    assert state_path.read_bytes() == before_state
    assert handoff.read_text(encoding="utf-8") == (
        "Do not acknowledge malformed target authority.\n"
    )
    assert not wrkslots._event_directory(config).exists()


def test_read_handoff_refuses_extra_git_registration_for_target_slot(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    handoff = checkout(project).parent / "HANDOFF.md"
    handoff.write_text("Preserve target-local Git drift.\n", encoding="utf-8")
    stale = handoff.parent / "stale-checkout"
    git(repository, "worktree", "add", "--detach", str(stale), "HEAD")
    shutil.rmtree(stale)
    assert stale.absolute() in wrkslots._GitVcs().listed_worktrees(repository)
    config = wrkslots._load_config(str(project), "testhost")
    before = wrkslots._global_rows(
        [wrkslots._load_active(config)], [wrkslots._load_archive(config)]
    )

    read = raw_command(
        project,
        "read-handoff",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
    )

    assert read.returncode == 3
    assert "no active checkout row" in read.stderr
    assert str(stale) in read.stderr
    after = wrkslots._global_rows(
        [wrkslots._load_active(config)], [wrkslots._load_archive(config)]
    )
    assert after == before
    assert not any(
        event["kind"] == "handoff-read" for event in wrkslots._load_events(config)
    )


@pytest.mark.parametrize("failure", ("missing", "symlink", "non-utf8", "target-missing"))
def test_read_handoff_still_refuses_invalid_target(
    tmp_path: Path, failure: str
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    config = wrkslots._load_config(str(project), "testhost")
    slot_path = checkout(project).parent
    handoff = slot_path / "HANDOFF.md"
    if failure == "symlink":
        target = tmp_path / "outside-handoff"
        target.write_text("outside\n", encoding="utf-8")
        handoff.symlink_to(target)
    elif failure == "non-utf8":
        handoff.write_bytes(b"\xff\xfe")
    elif failure == "target-missing":
        handoff.write_text("unreachable\n", encoding="utf-8")
        shutil.rmtree(slot_path)

    before = wrkslots._global_rows(
        [wrkslots._load_active(config)], [wrkslots._load_archive(config)]
    )
    read = raw_command(
        project,
        "read-handoff",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
    )

    assert read.returncode == 3
    expected = {
        "missing": "no read was recorded",
        "symlink": "no read was recorded",
        "non-utf8": "no read was recorded",
        "target-missing": "active row but no slot directory",
    }
    assert expected[failure] in read.stderr
    after = wrkslots._global_rows(
        [wrkslots._load_active(config)], [wrkslots._load_archive(config)]
    )
    assert after == before


def test_finished_slot_name_cannot_be_reused(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    completed = finish(project)
    assert completed.returncode == 0, completed.stderr
    mark_owner_dead(project)
    set_liveness(project, "dead")
    removed = remove(project)
    assert removed.returncode == 0, removed.stderr

    reused = create(
        project,
        slot="slot01",
        agent="codex-2",
        branch="codex/new-task",
    )

    assert reused.returncode == 3
    assert "archived" in reused.stderr
    assert not (project / "worktrees" / "slot01").exists()
    assert active_slots(project) == []


def test_remove_refuses_dead_owner_while_time_to_live_is_fresh(
    tmp_path: Path,
) -> None:
    project, _repository, remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    mark_owner_dead(project, expire=False)
    set_liveness(project, "dead")

    refused = remove(project)

    assert refused.returncode == 3
    assert "time-to-live has not expired" in refused.stderr
    assert "wait at least" in refused.stderr
    assert "wrkslots heartbeat" in refused.stderr
    assert checkout(project).is_dir()
    assert active_slots(project)
    assert git(remote, "for-each-ref", "--format=%(refname)", "refs/heads/salvage").stdout == ""


def test_validate_complete_cannot_bypass_agent_reclaim(tmp_path: Path) -> None:
    project, _repository, remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr

    refused = command(
        project,
        "remove",
        "slot01",
        "--validate-complete",
        "--coordinator-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
    )

    assert refused.returncode == 3
    assert "applies only to a slot created with --slot-type validate" in refused.stderr
    assert "no checkout was salvaged or removed" in refused.stderr
    assert checkout(project).is_dir()
    assert active_slots(project)
    assert git(remote, "for-each-ref", "--format=%(refname)", "refs/heads/salvage").stdout == ""


def test_later_coordinator_can_complete_reclaim_from_the_record(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    mark_owner_dead(project)
    set_original_coordinator_dead(project)
    set_liveness(project, "dead")

    removed = raw_command(
        project,
        "remove",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
    )

    assert removed.returncode == 0, removed.stderr
    assert not checkout(project).exists()
    archive = json.loads(
        (project / "worktrees" / "ARCHIVED.testhost.json").read_text(
            encoding="utf-8"
        )
    )
    assert archive["records"][0]["physical_storage"] == "removed"


def start_host_context_coordinator() -> tuple[subprocess.Popen[str], int]:
    read_fd, write_fd = os.pipe()
    coordinator = subprocess.Popen(
        [
            sys.executable,
            "-c",
            "import os,time\n"
            "while True:\n"
            " os.write(1, b'x')\n"
            " time.sleep(0.2)\n",
        ],
        text=True,
        stdout=write_fd,
        stderr=subprocess.DEVNULL,
    )
    os.close(write_fd)
    return coordinator, read_fd


def set_host_context_handoff(
    monkeypatch: pytest.MonkeyPatch,
    coordinator: wrkslots.ProcessIdentity,
    proof_fd: int,
    *,
    runner_pid: int | None = None,
) -> None:
    monkeypatch.setenv(
        "WRKSLOTS_REMOVE_RUNNER_PID", str(runner_pid or os.getpid())
    )
    monkeypatch.setenv(
        "WRKSLOTS_REMOVE_COORDINATOR_START_TICKS", str(coordinator.start_ticks)
    )
    monkeypatch.setenv("WRKSLOTS_REMOVE_PROOF_FD", str(proof_fd))


def test_host_context_runner_preserves_coordinator_identity(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project, slot_type="validate", branch=None)
    assert made.returncode == 0, made.stderr
    mark_owner_dead(project)
    set_liveness(project, "dead")
    coordinator, proof_fd = start_host_context_coordinator()
    try:
        identity = wrkslots._read_process_identity(coordinator.pid)
        set_host_context_handoff(monkeypatch, identity, proof_fd)

        removed = wrkslots.main(
            [
                "--project-root",
                str(project),
                "remove",
                "slot01",
                "--coordinator-pid",
                str(coordinator.pid),
                "--expected-generation",
                "1",
            ]
        )

        assert removed == 0
        events = wrkslots._load_events(wrkslots._load_config(str(project), "testhost"))
        started = [event for event in events if event["kind"] == "reclaim-started"]
        assert len(started) == 1
        payload = started[0]["payload"]
        assert isinstance(payload, dict)
        assert payload["actor"] == wrkslots._identity_to_obj(identity)
        runner = wrkslots._as_mapping(payload["runner"], "test runner")
        assert runner["pid"] == os.getpid()
        writer = wrkslots._as_mapping(payload["handoff_writer"], "test writer")
        assert writer["pid"] == coordinator.pid
    finally:
        terminate_process(coordinator)
        os.close(proof_fd)


def test_host_context_runner_rechecks_coordinator_at_locked_removal_boundary(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project, slot_type="validate", branch=None)
    assert made.returncode == 0, made.stderr
    mark_owner_dead(project)
    set_liveness(project, "dead")
    tree = checkout(project, slot_type="validate")
    coordinator, proof_fd = start_host_context_coordinator()
    identity = wrkslots._read_process_identity(coordinator.pid)
    set_host_context_handoff(monkeypatch, identity, proof_fd)
    real_mutation_locks = wrkslots._mutation_locks

    @contextlib.contextmanager
    def terminate_after_lock(
        config: wrkslots.Config, wait_seconds: float
    ) -> Iterator[None]:
        with real_mutation_locks(config, wait_seconds):
            terminate_process(coordinator)
            yield

    monkeypatch.setattr(wrkslots, "_mutation_locks", terminate_after_lock)

    removed = wrkslots.main(
        [
            "--project-root",
            str(project),
            "remove",
            "slot01",
            "--coordinator-pid",
            str(coordinator.pid),
            "--expected-generation",
            "1",
        ]
    )

    assert removed == 3
    assert tree.is_dir()
    assert len(active_slots(project)) == 1
    events = wrkslots._load_events(wrkslots._load_config(str(project), "testhost"))
    assert not [event for event in events if event["kind"] == "reclaim-started"]
    os.close(proof_fd)


def test_host_context_runner_rechecks_pipe_writer_at_locked_removal_boundary(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project, slot_type="validate", branch=None)
    assert made.returncode == 0, made.stderr
    mark_owner_dead(project)
    set_liveness(project, "dead")
    tree = checkout(project, slot_type="validate")
    proof_writer, proof_fd = start_host_context_coordinator()
    coordinator = wrkslots._read_process_identity(os.getpid())
    set_host_context_handoff(monkeypatch, coordinator, proof_fd)
    real_mutation_locks = wrkslots._mutation_locks

    @contextlib.contextmanager
    def terminate_writer_after_lock(
        config: wrkslots.Config, wait_seconds: float
    ) -> Iterator[None]:
        with real_mutation_locks(config, wait_seconds):
            terminate_process(proof_writer)
            yield

    monkeypatch.setattr(wrkslots, "_mutation_locks", terminate_writer_after_lock)
    try:
        removed = wrkslots.main(
            [
                "--project-root",
                str(project),
                "remove",
                "slot01",
                "--coordinator-pid",
                str(os.getpid()),
                "--expected-generation",
                "1",
            ]
        )

        assert removed == 3
        assert tree.is_dir()
        assert len(active_slots(project)) == 1
        events = wrkslots._load_events(
            wrkslots._load_config(str(project), "testhost")
        )
        assert not [event for event in events if event["kind"] == "reclaim-started"]
    finally:
        terminate_process(proof_writer)
        os.close(proof_fd)


def test_host_context_runner_requires_complete_identity_handoff(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.delenv("WRKSLOTS_REMOVE_COORDINATOR_START_TICKS", raising=False)
    monkeypatch.setenv("WRKSLOTS_REMOVE_RUNNER_PID", str(os.getpid()))

    with pytest.raises(wrkslots.Refusal, match="requires runner PID"):
        wrkslots._capture_remove_processes(os.getpid())


def test_host_context_runner_refuses_mismatched_coordinator_generation(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    identity = wrkslots._read_process_identity(os.getpid())
    monkeypatch.setenv("WRKSLOTS_REMOVE_RUNNER_PID", str(os.getpid()))
    monkeypatch.setenv(
        "WRKSLOTS_REMOVE_COORDINATOR_START_TICKS", str(identity.start_ticks + 1)
    )
    monkeypatch.setenv("WRKSLOTS_REMOVE_PROOF_FD", "0")

    with pytest.raises(wrkslots.Refusal, match="generation changed"):
        wrkslots._capture_remove_processes(os.getpid())


def test_host_context_runner_must_be_in_invoking_ancestry(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    sibling = subprocess.Popen(
        [sys.executable, "-c", "import time; time.sleep(120)"],
        text=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        identity = wrkslots._read_process_identity(os.getpid())
        monkeypatch.setenv("WRKSLOTS_REMOVE_RUNNER_PID", str(sibling.pid))
        monkeypatch.setenv(
            "WRKSLOTS_REMOVE_COORDINATOR_START_TICKS", str(identity.start_ticks)
        )
        monkeypatch.setenv("WRKSLOTS_REMOVE_PROOF_FD", "0")

        with pytest.raises(wrkslots.Refusal, match="removal runner PID"):
            wrkslots._capture_remove_processes(os.getpid())
    finally:
        terminate_process(sibling)


def test_host_context_runner_validate_batch_preserves_actor_and_runner(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    paths = prepare_dead_validate_slots(project, ("batch-host",))
    stub_validate_batch_censuses(monkeypatch)
    coordinator, proof_fd = start_host_context_coordinator()
    try:
        identity = wrkslots._read_process_identity(coordinator.pid)
        set_host_context_handoff(monkeypatch, identity, proof_fd)

        removed = wrkslots.main(
            [
                "--project-root",
                str(project),
                "remove-validate-batch",
                "--coordinator-pid",
                str(coordinator.pid),
                "--slot",
                "batch-host=1",
                "--format",
                "json",
            ]
        )

        assert removed == 0
        report = json.loads(capsys.readouterr().out)
        assert report["removed"] == [{"generation": 1, "slot": "batch-host"}]
        assert not paths["batch-host"].exists()
        events = wrkslots._load_events(
            wrkslots._load_config(str(project), "testhost")
        )
        started = [event for event in events if event["kind"] == "reclaim-started"]
        assert len(started) == 1
        payload = wrkslots._as_mapping(started[0]["payload"], "test payload")
        assert payload["actor"] == wrkslots._identity_to_obj(identity)
        runner = wrkslots._as_mapping(payload["runner"], "test runner")
        assert runner["pid"] == os.getpid()
    finally:
        terminate_process(coordinator)
        os.close(proof_fd)


def test_host_context_runner_recovers_ownerless_validation_cargo_home(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    request: pytest.FixtureRequest,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    cargo_home = project / "ignored/validate/cargo-homes/validate-cargo-host"
    cargo_home.mkdir(parents=True)
    cargo_home.chmod(0o700)
    (cargo_home / "cache-entry").write_text("regenerable\n", encoding="utf-8")
    record = prepare_terminal_validation_record(
        project, cargo_home, field="cargo_home"
    )
    install_validation_exclusion_fixture(project, monkeypatch, request)
    monkeypatch.setattr(
        wrkslots,
        "_capture_same_uid_process_path_census",
        lambda *_args, **_kwargs: wrkslots._ProcessPathCensus(
            (), (), owner_cgroup_complete=False
        ),
    )
    coordinator, proof_fd = start_host_context_coordinator()
    try:
        identity = wrkslots._read_process_identity(coordinator.pid)
        set_host_context_handoff(monkeypatch, identity, proof_fd)

        removed = wrkslots.main(
            [
                "--project-root",
                str(project),
                "recover",
                "--coordinator-authorized",
                "--coordinator-pid",
                str(coordinator.pid),
                "--ownerless-validate-cargo-home",
                cargo_home.relative_to(project).as_posix(),
                "--completed-record",
                record.relative_to(project).as_posix(),
            ]
        )

        assert removed == 0
        assert not cargo_home.exists()
    finally:
        terminate_process(coordinator)
        os.close(proof_fd)


def test_host_context_runner_removes_live_validate_owner_with_pipe_proof(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    coordinator, proof_fd = start_host_context_coordinator()
    try:
        identity = wrkslots._read_process_identity(coordinator.pid)
        made = create(project, slot_type="validate", branch=None)
        assert made.returncode == 0, made.stderr
        replace_owner(project, identity)
        set_host_context_handoff(monkeypatch, identity, proof_fd)

        removed = wrkslots.main(
            [
                "--project-root",
                str(project),
                "remove",
                "slot01",
                "--validate-complete",
                "--coordinator-pid",
                str(coordinator.pid),
                "--expected-generation",
                "1",
            ]
        )

        assert removed == 0
        assert not checkout(project, slot_type="validate").exists()
        assert active_slots(project) == []
    finally:
        terminate_process(coordinator)
        os.close(proof_fd)


def test_host_context_runner_refuses_live_nonancestor_validate_owner(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    coordinator = subprocess.Popen(
        [sys.executable, "-c", "import time; time.sleep(120)"],
        text=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    proof_writer, proof_fd = start_host_context_coordinator()
    try:
        identity = wrkslots._read_process_identity(coordinator.pid)
        made = create(project, slot_type="validate", branch=None)
        assert made.returncode == 0, made.stderr
        replace_owner(project, identity)
        set_host_context_handoff(monkeypatch, identity, proof_fd)

        removed = wrkslots.main(
            [
                "--project-root",
                str(project),
                "remove",
                "slot01",
                "--validate-complete",
                "--coordinator-pid",
                str(coordinator.pid),
                "--expected-generation",
                "1",
            ]
        )

        assert removed == 3
        assert checkout(project, slot_type="validate").exists()
        slots = active_slots(project)
        assert len(slots) == 1
        slot = wrkslots._as_mapping(slots[0], "test active slot")
        assert slot["slot"] == "slot01"
    finally:
        terminate_process(coordinator)
        terminate_process(proof_writer)
        os.close(proof_fd)


@pytest.mark.parametrize("liveness", ["alive", "unverifiable"])
def test_remove_refuses_registered_liveness_without_deleting(
    tmp_path: Path, liveness: str
) -> None:
    project, repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    commit_task(repository, checkout(project), "codex/task")
    handed_off = finish(project)
    assert handed_off.returncode == 0, handed_off.stderr
    mark_owner_dead(project)
    set_liveness(project, liveness)

    refused = remove(project)

    assert refused.returncode == 3
    expected = "owner alive" if liveness == "alive" else "unverifiable"
    assert expected in refused.stderr
    assert checkout(project).is_dir()
    assert len(active_slots(project)) == 1


def test_remove_refuses_unexpected_liveness_exit_status_without_deleting(
    tmp_path: Path,
) -> None:
    """An exit status the liveness protocol does not define is not a licence to delete.

    The registered authority speaks exactly three codes: 0 verified-dead, 1 alive,
    2 unverifiable. A crashed, upgraded, or mis-wired probe that exits with anything
    else has said nothing about the owner, and the destructive boundary must treat
    that as a refusal rather than as the absence of a "not dead" answer.
    """
    project, repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    commit_task(repository, checkout(project), "codex/task")
    handed_off = finish(project)
    assert handed_off.returncode == 0, handed_off.stderr
    mark_owner_dead(project)
    # The fixture probe exits 3 for any state name it does not recognise.
    set_liveness(project, "probe-crashed")

    refused = remove(project)

    assert refused.returncode == 3
    assert "unexpected rc 3" in refused.stderr
    assert checkout(project).is_dir()
    assert len(active_slots(project)) == 1


def set_owner_to_machine_init(project: Path, machine: str = "testhost") -> None:
    """Record the machine's init as the owner, and expire the heartbeat.

    This is the degenerate state measured on a real registry: 17 rows naming
    PID 1 in /init.scope, written that way at registration.
    """
    config = wrkslots._load_config(str(project), machine)
    state = wrkslots._load_active(config)
    record = state.slots[0]
    assert record.owner is not None
    heartbeat_at = (
        dt.datetime.now(dt.timezone.utc)
        - dt.timedelta(seconds=record.heartbeat_ttl_seconds + 1)
    ).isoformat(timespec="seconds")
    updated = replace(
        record,
        owner=replace(record.owner, pid=1, cgroup_path="/init.scope"),
        heartbeat_at=heartbeat_at,
    )
    wrkslots._write_active_state(
        config,
        wrkslots._replace_record(state, updated),
        action="test-owner-is-machine-init",
        slot=record.slot,
    )


def test_remove_releases_a_slot_whose_owner_record_is_degenerate(tmp_path: Path) -> None:
    """A slot owned by the machine's init is removable.

    ⚠️ THIS IS THE WIRING TEST, and it exists because the predicate's own unit
    tests could not catch a revert. Restoring the old
    "requires a proven-dead recorded owner" behaviour for degenerate rows makes
    this fail; nothing else in the suite does.

    ⚠️ WHY THE OLD REFUSAL COULD NEVER BE SATISFIED. PID 1 in /init.scope reads
    as LIVE for as long as the machine is up, so owner_state is never "dead" and
    the refusal asked for proof of a historical owner that -- measured across an
    entire 6,841-entry event log -- was never recorded for any row in this
    state. The row naming init IS the reason no such proof exists.
    """
    project, repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    tree = checkout(project)
    commit_task(repository, tree, "codex/task")
    handed_off = finish(project)
    assert handed_off.returncode == 0, handed_off.stderr
    set_owner_to_machine_init(project)
    set_liveness(project, "dead")

    removed = remove(project)

    assert removed.returncode == 0, removed.stderr
    assert "proven-dead recorded owner" not in removed.stderr
    assert "no recorded owner process" not in removed.stderr
    assert not tree.exists()
    assert not active_slots(project)


@pytest.mark.parametrize("owner", ["live", "other-machine"])
def test_remove_refuses_a_not_proven_dead_owner_even_when_the_authority_says_dead(
    tmp_path: Path, owner: str
) -> None:
    """The recorded owner generation is a second, independent veto on deletion.

    This is the reaper incident: an authority that answers "dead" is not enough
    when the row's own owner generation is still running here, or belongs to a
    machine this host cannot speak for. Either case must preserve the slot.
    """
    project, repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    commit_task(repository, checkout(project), "codex/task")
    handed_off = finish(project)
    assert handed_off.returncode == 0, handed_off.stderr
    expire_heartbeat(project)
    if owner == "other-machine":
        set_owner_other_machine(project)
    # The registered authority is made to agree that the owner is gone. It is wrong.
    set_liveness(project, "dead")

    refused = remove(project)

    assert refused.returncode == 3
    assert "requires a proven-dead recorded owner" in refused.stderr
    expected = "owner is live" if owner == "live" else "owner is indeterminate"
    assert expected in refused.stderr
    assert checkout(project).is_dir()
    assert len(active_slots(project)) == 1


def test_remove_releases_expired_unbound_owner_without_recovery_evidence(
    tmp_path: Path,
) -> None:
    """An expired unbound slot follows the normal live-use and salvage checks."""
    project, _repository, _remote = make_project(tmp_path)
    made = create(project, bind_owner=False)
    assert made.returncode == 0, made.stderr
    set_liveness(project, "dead")
    refused = remove(project)
    assert refused.returncode == 3
    assert "without renewal before retrying remove" in refused.stderr
    assert checkout(project).is_dir()
    assert len(active_slots(project)) == 1
    expire_heartbeat(project)

    removed = remove(project)

    assert removed.returncode == 0, removed.stderr
    assert not checkout(project).exists()
    assert active_slots(project) == []
    archive = json.loads(
        (control_directory(project) / "ARCHIVED.testhost.json").read_text(encoding="utf-8")
    )
    assert len(archive["records"]) == 1
    archived = archive["records"][0]
    assert (archived["slot"], archived["generation"]) == ("slot01", 1)
    assert archived["physical_storage"] == "removed"
    assert archived["validation"]
    assert archived["checkouts"]


def test_cross_shard_duplicate_refuses_before_deletion(tmp_path: Path) -> None:
    project, repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    commit_task(repository, checkout(project), "codex/task")
    handed_off = finish(project)
    assert handed_off.returncode == 0, handed_off.stderr
    mark_owner_dead(project)
    set_liveness(project, "dead")
    duplicate = active(project)
    duplicate["machine"] = "machine-b"
    duplicate_slots = duplicate["slots"]
    assert isinstance(duplicate_slots, list)
    duplicate_row = duplicate_slots[0]
    assert isinstance(duplicate_row, dict)
    duplicate_row["machine"] = "machine-b"
    (project / "worktrees" / "ACTIVE.machine-b.json").write_text(
        json.dumps(duplicate, indent=2) + "\n", encoding="utf-8"
    )
    (project / "worktrees" / "ARCHIVED.machine-b.json").write_text(
        json.dumps(
            {
                "schema": wrkslots.SCHEMA,
                "machine": "machine-b",
                "revision": 0,
                "records": [],
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )

    refused = remove(project)

    assert refused.returncode == 3
    assert "active in both" in refused.stderr
    assert checkout(project).is_dir()
    assert len(active_slots(project)) == 1


@pytest.mark.parametrize("mismatch", ["row-without-directory", "directory-without-row"])
def test_status_reports_registry_directory_mismatch(
    tmp_path: Path, mismatch: str
) -> None:
    project, repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    if mismatch == "row-without-directory":
        git(repository, "worktree", "remove", "--force", str(checkout(project)))
        (project / "worktrees" / "slot01").rmdir()
    else:
        orphan = project / "worktrees" / "orphan" / "product"
        orphan.parent.mkdir()
        git(
            repository,
            "worktree",
            "add",
            "-b",
            "codex/orphan",
            str(orphan),
            "origin/main",
        )

    reported = command(
        project, "status", "--all-machines", "--format", "json"
    )

    assert reported.returncode == 0, reported.stderr
    payload = json.loads(reported.stdout)
    assert payload["registry_storage_state"] == "inconsistent"
    findings = payload["registry_storage_inconsistencies"]
    assert mismatch in {finding["kind"] for finding in findings}
    assert all(finding["remedy"] for finding in findings)
    assert [record["slot"] for record in payload["active"]] == ["slot01"]
    row_findings = payload["active"][0]["storage_inconsistencies"]
    if mismatch == "row-without-directory":
        assert mismatch in {finding["kind"] for finding in row_findings}
    else:
        assert row_findings == []
    if mismatch == "directory-without-row":
        finding = next(
            item for item in findings if item["kind"] == "directory-without-row"
        )
        assert "wrkslots register orphan --help" in finding["remedy"]
        repaired = command(
            project,
            "register",
            "orphan",
            "--agent",
            "codex-orphan",
            "--task",
            "repair-registry",
            "--purpose",
            "restore the missing active row",
            "--owner-pid",
            str(os.getpid()),
            "--coordinator-pid",
            str(os.getpid()),
            "--verified-live",
            "--repo",
            "product=repo",
        )
        assert repaired.returncode == 0, repaired.stderr
        status = command(project, "status", "--slot", "orphan")
        assert status.returncode == 0, status.stderr


def test_fresh_registry_status_is_consistent(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)

    status = command(project, "status", "--all-machines", "--format", "json")

    assert status.returncode == 0, status.stderr
    payload = json.loads(status.stdout)
    assert payload["active"] == []
    assert payload["registry_storage_state"] == "consistent"
    assert payload["registry_storage_inconsistencies"] == []


def test_unbound_owner_is_removable_after_recovery_note_and_expiry(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project, bind_owner=False)
    assert made.returncode == 0, made.stderr
    set_liveness(project, "dead")

    recovered = command(
        project,
        "recover-unbound-owner",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        "--recovery-note",
        "historical owner never bound; registered liveness returned dead",
        "--validation",
        "inert fixture",
    )

    assert recovered.returncode == 0, recovered.stderr
    row = active_slots(project)[0]
    assert isinstance(row, dict)
    assert row["owner"] is None
    assert row["coordinator_recovery_note"]
    expire_heartbeat(project)
    removed = remove(project)
    assert removed.returncode == 0, removed.stderr
    assert not checkout(project).exists()
    assert active_slots(project) == []
    archive = json.loads(
        (control_directory(project) / "ARCHIVED.testhost.json").read_text(encoding="utf-8")
    )
    assert len(archive["records"]) == 1
    archived = archive["records"][0]
    assert (archived["slot"], archived["generation"]) == ("slot01", 1)
    assert archived["physical_storage"] == "removed"
    assert archived["validation"]
    assert archived["checkouts"]


@pytest.mark.ordinary_environment
def test_adopt_refuses_pid_outside_invoking_process_ancestry(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project, bind_owner=False)
    assert made.returncode == 0, made.stderr
    sleeper = subprocess.Popen(["sleep", "60"], text=True)
    try:
        refused = command(
            project,
            "adopt",
            "slot01",
            "--agent",
            "codex-1",
            "--owner-pid",
            str(sleeper.pid),
            "--expected-generation",
            "1",
        )
        assert refused.returncode == 3
        assert "not in the invoking process ancestry" in refused.stderr
        row = active_slots(project)[0]
        assert isinstance(row, dict)
        assert row["owner"] is None
    finally:
        terminate_process(sleeper)


@pytest.mark.ordinary_environment
def test_remove_refuses_live_process_using_slot(tmp_path: Path) -> None:
    project, repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    tree = checkout(project)
    sleeper = subprocess.Popen(["sleep", "60"], cwd=tree, text=True)
    try:
        commit_task(repository, tree, "codex/task")
        handed_off = finish(project)
        assert handed_off.returncode == 0, handed_off.stderr
        mark_owner_dead(project)
        set_liveness(project, "dead")
        refused = remove(project)
        assert refused.returncode == 3
        assert "live process" in refused.stderr
        assert tree.is_dir()
    finally:
        terminate_process(sleeper)


def test_import_existing_is_dry_run_then_registers_verified_live_slot(
    tmp_path: Path,
) -> None:
    project, repository, remote = make_project(tmp_path)
    git(repository, "remote", "add", "upstream", str(remote))
    tree = checkout(project)
    tree.parent.mkdir()
    git(repository, "worktree", "add", "-b", "codex/imported", str(tree), "origin/main")
    common = (
        "import-existing",
        "slot01",
        "--agent",
        "codex-1",
        "--task",
        "task-import",
        "--purpose",
        "import existing",
        "--repo",
        "product=repo",
        "--remote",
        "product=upstream",
        "--remote-url",
        f"product={remote}",
    )

    dry_run = command(project, *common)

    assert dry_run.returncode == 0, dry_run.stderr
    assert "dry-run: no state changed" in dry_run.stdout
    assert "--coordinator-pid PID --coordinator-authorized" in dry_run.stdout
    assert active_slots(project) == []
    applied = command(
        project,
        *common,
        "--apply",
        "--verified-live",
        "--owner-pid",
        str(os.getpid()),
        "--coordinator-pid",
        str(os.getpid()),
    )
    assert applied.returncode == 0, applied.stderr
    rows = active_slots(project)
    assert len(rows) == 1
    row = rows[0]
    assert isinstance(row, dict)
    checkouts = row["checkouts"]
    assert isinstance(checkouts, list)
    imported = checkouts[0]
    assert isinstance(imported, dict)
    assert imported["remote"] == "upstream"
    assert tree.is_dir()


def test_import_existing_registers_coordinator_child_using_the_slot(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    tree = checkout(project)
    tree.parent.mkdir()
    git(repository, "worktree", "add", "-b", "codex/imported", str(tree), "origin/main")
    owner = subprocess.Popen(["sleep", "60"], cwd=tree, text=True)
    try:
        applied = command(
            project,
            "import-existing",
            "slot01",
            "--agent",
            "codex-1",
            "--task",
            "task-import",
            "--purpose",
            "import coordinator child",
            "--repo",
            "product=repo",
            "--apply",
            "--verified-live",
            "--owner-pid",
            str(owner.pid),
            "--coordinator-pid",
            str(os.getpid()),
        )

        assert applied.returncode == 0, applied.stderr
        row = active_slots(project)[0]
        assert isinstance(row, dict)
        recorded_owner = row["owner"]
        assert isinstance(recorded_owner, dict)
        assert recorded_owner["pid"] == owner.pid
    finally:
        terminate_process(owner)


def test_import_existing_refuses_coordinator_child_not_using_the_slot(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    tree = checkout(project)
    tree.parent.mkdir()
    git(repository, "worktree", "add", "-b", "codex/imported", str(tree), "origin/main")
    owner = subprocess.Popen(["sleep", "60"], cwd=project, text=True)
    try:
        refused = command(
            project,
            "import-existing",
            "slot01",
            "--agent",
            "codex-1",
            "--task",
            "task-import",
            "--purpose",
            "reject unrelated coordinator child",
            "--repo",
            "product=repo",
            "--apply",
            "--verified-live",
            "--owner-pid",
            str(owner.pid),
            "--coordinator-pid",
            str(os.getpid()),
        )

        assert refused.returncode == 3
        assert "does not have its working directory inside slot" in refused.stderr
        assert active_slots(project) == []
    finally:
        terminate_process(owner)


def test_import_existing_accepts_a_sibling_source_repository(
    tmp_path: Path,
) -> None:
    project, _unused_repository, remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    sibling = tmp_path / "sibling-repository"
    subprocess.run(
        ["git", "clone", str(remote), str(sibling)],
        check=True,
        capture_output=True,
        text=True,
    )
    tree = checkout(project)
    tree.parent.mkdir(exist_ok=True)
    git(sibling, "worktree", "add", "-b", "codex/imported", str(tree), "origin/main")

    relative_sibling = Path("..") / sibling.name
    common = (
        "import-existing",
        "slot01",
        "--agent",
        "codex-1",
        "--task",
        "task-import",
        "--purpose",
        "import sibling repository",
        "--repo",
        f"product={relative_sibling}",
    )
    dry_run = command(project, *common)
    assert dry_run.returncode == 0, dry_run.stderr
    assert "dry-run: no state changed" in dry_run.stdout

    applied = command(
        project,
        *common,
        "--apply",
        "--verified-live",
        "--owner-pid",
        str(os.getpid()),
        "--coordinator-pid",
        str(os.getpid()),
    )

    assert applied.returncode == 0, applied.stderr
    row = active_slots(project)[0]
    assert isinstance(row, dict)
    checkouts = row["checkouts"]
    assert isinstance(checkouts, list)
    imported = checkouts[0]
    assert isinstance(imported, dict)
    assert imported["repository"] == relative_sibling.as_posix()
    status = command(project, "status", "--slot", "slot01", "--format", "json")
    assert status.returncode == 0, status.stderr


@pytest.mark.parametrize(
    ("repository_spelling", "expected_error"),
    (
        ("absolute-sibling", "repository path must be relative"),
        ("beyond-parent", "other parent traversal is refused"),
        ("sibling-symlink", "repository crosses a symlink"),
    ),
)
def test_import_existing_refuses_repository_paths_outside_the_sibling_boundary(
    tmp_path: Path,
    repository_spelling: str,
    expected_error: str,
) -> None:
    project_parent = tmp_path / "project-parent"
    project_parent.mkdir()
    project, _unused_repository, remote = make_project(
        project_parent,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    if repository_spelling == "beyond-parent":
        repository = tmp_path / "external-repository"
        raw_repository = Path("../..") / repository.name
    else:
        repository = project_parent / "sibling-repository"
        raw_repository = repository
    subprocess.run(
        ["git", "clone", str(remote), str(repository)],
        check=True,
        capture_output=True,
        text=True,
    )
    if repository_spelling == "sibling-symlink":
        alias = project_parent / "sibling-alias"
        alias.symlink_to(repository, target_is_directory=True)
        raw_repository = Path("..") / alias.name
    elif repository_spelling == "absolute-sibling":
        raw_repository = repository.resolve()
    marker = repository / "keep.txt"
    marker.write_text("keep\n", encoding="utf-8")
    tree = checkout(project)
    tree.parent.mkdir(exist_ok=True)
    git(
        repository,
        "worktree",
        "add",
        "-b",
        f"codex/{repository_spelling}",
        str(tree),
        "origin/main",
    )

    refused = command(
        project,
        "import-existing",
        "slot01",
        "--agent",
        "codex-1",
        "--task",
        f"task-{repository_spelling}",
        "--purpose",
        f"reject {repository_spelling}",
        "--repo",
        f"product={raw_repository}",
    )

    assert refused.returncode == 3
    assert expected_error in refused.stderr
    assert active_slots(project) == []
    assert marker.read_text(encoding="utf-8") == "keep\n"
    assert tree.is_dir()


def test_repository_path_refuses_non_sibling_parent_forms_before_normalizing(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    config = wrkslots._load_config(str(project), "testhost")
    alias = project / "link"
    alias.symlink_to(tmp_path, target_is_directory=True)

    with pytest.raises(wrkslots.Refusal, match="other parent traversal is refused"):
        wrkslots._repository_path(config, "../..")
    with pytest.raises(wrkslots.Refusal, match="aliases the project root"):
        wrkslots._repository_path(config, f"../{project.name}")
    with pytest.raises(wrkslots.Refusal, match="aliases the project root"):
        wrkslots._stored_repository_path(config, f"../{project.name}")
    with pytest.raises(
        wrkslots.Refusal, match="outside the managed worktrees directory"
    ):
        wrkslots._stored_repository_path(config, "worktrees/recorded-source")
    with pytest.raises(wrkslots.Refusal, match="other parent traversal is refused"):
        wrkslots._repository_path(config, f"{alias.name}/../{repository.name}")
    root_level_config = replace(config, root=Path("/synthetic-project"))
    with pytest.raises(
        wrkslots.Refusal,
        match="stored absolute repository path must name one direct sibling",
    ):
        wrkslots._stored_repository_path(root_level_config, "/")

    assert alias.is_symlink()


def test_legacy_absolute_sibling_active_record_remains_usable(
    tmp_path: Path,
) -> None:
    project, _unused_repository, remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    sibling = tmp_path / "sibling-repository"
    subprocess.run(
        ["git", "clone", str(remote), str(sibling)],
        check=True,
        capture_output=True,
        text=True,
    )
    tree = checkout(project)
    tree.parent.mkdir(exist_ok=True)
    git(sibling, "worktree", "add", "-b", "codex/imported", str(tree), "origin/main")
    applied = command(
        project,
        "import-existing",
        "slot01",
        "--agent",
        "codex-1",
        "--task",
        "task-import",
        "--purpose",
        "import sibling repository",
        "--repo",
        f"product=../{sibling.name}",
        "--apply",
        "--verified-live",
        "--owner-pid",
        str(os.getpid()),
        "--coordinator-pid",
        str(os.getpid()),
    )
    assert applied.returncode == 0, applied.stderr

    config = wrkslots._load_config(str(project), "testhost")
    state = wrkslots._load_active(config)
    record = state.slots[0]
    legacy_checkout = replace(
        record.checkouts[0], repository=str(sibling.resolve())
    )
    legacy_record = replace(record, checkouts=(legacy_checkout,))
    legacy_state = replace(state, slots=(legacy_record,))

    wrkslots._assert_record_paths(config, legacy_record)
    wrkslots._assert_registry_storage_consistent(config, [legacy_state])
    normalized, resolved = wrkslots._stored_repository_path(
        config, legacy_checkout.repository
    )
    assert normalized == f"../{sibling.name}"
    assert resolved == sibling.resolve()


def test_absolute_non_sibling_stored_repository_refuses(
    tmp_path: Path,
) -> None:
    project_parent = tmp_path / "project-parent"
    project_parent.mkdir()
    project, _repository, remote = make_project(project_parent)
    made = create(project)
    assert made.returncode == 0, made.stderr
    external = tmp_path / "external-repository"
    subprocess.run(
        ["git", "clone", str(remote), str(external)],
        check=True,
        capture_output=True,
        text=True,
    )
    config = wrkslots._load_config(str(project), "testhost")
    record = wrkslots._load_active(config).slots[0]
    invalid_checkout = replace(
        record.checkouts[0], repository=str(external.resolve())
    )
    invalid_record = replace(record, checkouts=(invalid_checkout,))

    with pytest.raises(
        wrkslots.Refusal,
        match="stored absolute repository path must name one direct sibling",
    ):
        wrkslots._assert_record_paths(config, invalid_record)


def test_recover_accepts_legacy_absolute_sibling_create_journal(
    tmp_path: Path,
) -> None:
    project, _unused_repository, remote = make_project(tmp_path)
    sibling = tmp_path / "sibling-repository"
    subprocess.run(
        ["git", "clone", str(remote), str(sibling)],
        check=True,
        capture_output=True,
        text=True,
    )
    interrupted = create(
        project,
        repository_name=f"../{sibling.name}",
        env={"WRKSLOTS_TEST_INTERRUPT": "after-create-worktree"},
    )
    assert interrupted.returncode == 86
    scoped_path = create_journal_path(project)
    journal_path = control_directory(project) / "ACTIVE.testhost.journal"
    os.replace(scoped_path, journal_path)
    shutil.rmtree(control_directory(project) / "EVENTS.testhost")
    journal = json.loads(journal_path.read_text(encoding="utf-8"))
    legacy_repository = str(sibling.resolve())
    journal["planned"][0]["repository"] = legacy_repository
    journal["created"][0]["repository"] = legacy_repository
    journal_path.write_text(json.dumps(journal, indent=2) + "\n", encoding="utf-8")

    recovered = command(
        project, "recover", "--coordinator-pid", str(os.getpid())
    )

    assert recovered.returncode == 0, recovered.stderr
    row = active_slots(project)[0]
    assert isinstance(row, dict)
    checkouts = row["checkouts"]
    assert isinstance(checkouts, list)
    recovered_checkout = checkouts[0]
    assert isinstance(recovered_checkout, dict)
    assert recovered_checkout["repository"] == legacy_repository
    assert not journal_path.exists()


def test_every_refusal_prints_a_repair_command(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    code = wrkslots.main(
        ["--project-root", str(tmp_path / "missing-project"), "status"]
    )
    captured = capsys.readouterr()

    assert code == 3
    assert "REFUSED:" in captured.err
    assert "REMEDY:" in captured.err
    assert "wrkslots status --help" in captured.err


def test_import_existing_can_register_multiple_flat_cutover_stragglers(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    remote_url = git(repository, "remote", "get-url", "origin").stdout.strip()
    slots = project / "worktrees" / "slots"
    slots.mkdir(exist_ok=True)
    for slot, branch in (
        ("slot01", "codex/import-one"),
        ("slot02", "codex/import-two"),
    ):
        git(
            repository,
            "worktree",
            "add",
            "-b",
            branch,
            str(slots / slot),
            "origin/main",
        )

    for index, (slot, _branch) in enumerate(
        (("slot01", "codex/import-one"), ("slot02", "codex/import-two")), start=1
    ):
        applied = command(
            project,
            "import-existing",
            slot,
            "--agent",
            f"codex-{index}",
            "--task",
            f"task-import-{index}",
            "--purpose",
            "cutover straggler",
            "--repo",
            "product=repo",
            "--remote-url",
            f"product={remote_url}",
            "--apply",
            "--verified-live",
            "--owner-pid",
            str(os.getpid()),
            "--coordinator-pid",
            str(os.getpid()),
        )
        assert applied.returncode == 0, applied.stderr

    imported_slots: list[object] = []
    for row in active_slots(project):
        assert isinstance(row, dict)
        imported_slots.append(row["slot"])
    assert imported_slots == ["slot01", "slot02"]


def test_historical_import_records_nested_slot_and_salvages_before_removal(
    tmp_path: Path,
) -> None:
    project, repository, remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    slot = "old-slot"
    tree = slots_directory(project) / slot / "product"
    tree.parent.mkdir()
    git(repository, "worktree", "add", "-b", "codex/old", str(tree), "origin/main")
    source = write_historical_state(
        project,
        slot,
        task="current-row-task",
        owner_task="owner-start-task",
    )
    host_id = wrkslots._host_id()

    dry_run = command(
        project,
        "import-existing",
        slot,
        "--from-state-file",
        source.name,
        "--source-host-id",
        host_id,
        "--repo",
        "product=repo",
    )

    assert dry_run.returncode == 0, dry_run.stderr
    assert "dry-run: no state changed" in dry_run.stdout
    assert "--coordinator-pid PID --coordinator-authorized" in dry_run.stdout
    assert active_slots(project) == []
    applied = command(
        project,
        "import-existing",
        slot,
        "--from-state-file",
        source.name,
        "--source-host-id",
        host_id,
        "--repo",
        "product=repo",
        "--coordinator-pid",
        str(os.getpid()),
        "--apply",
    )
    assert applied.returncode == 0, applied.stderr
    row = active_slots(project)[0]
    assert isinstance(row, dict)
    assert row["layout"] == "nested"
    assert row["task"] == "current-row-task"
    assert row["owner"]["boot_id"] == "historical-boot"
    assert row["import_source"]["path"] == source.name
    assert row["import_source"]["row"] == json.loads(
        source.read_text(encoding="utf-8")
    )["slots"][slot]
    audited = command(project, "audit", "--format", "json")
    assert audited.returncode == 0, audited.stderr
    audit_row = json.loads(audited.stdout)["slots"][0]
    assert audit_row["slot"] == slot
    assert audit_row["owner_state"] == "dead"
    assert audit_row["verdict"] == "BLOCKED"
    assert audit_row["heartbeat_expired"] is False

    (tree / "uncommitted.txt").write_text("preserve me\n", encoding="utf-8")
    expire_heartbeat(project)
    set_liveness(project, "dead")
    removed = remove(project, slot)
    assert removed.returncode == 0, removed.stderr
    assert not tree.parent.exists()
    archive = json.loads(
        (control_directory(project) / "ARCHIVED.testhost.json").read_text(
            encoding="utf-8"
        )
    )
    archived = archive["records"][0]
    assert archived["layout"] == "nested"
    assert archived["import_source"]["row_sha256"] == row["import_source"]["row_sha256"]
    assert archived["salvage"][0]["disposition"] == "salvaged"
    salvage_ref = archived["salvage"][0]["remote_ref"]
    assert git(remote, "rev-parse", salvage_ref).returncode == 0


def test_historical_empty_slot_enters_registry_and_removes_without_guessing(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    slot = "old-empty"
    slot_path = slots_directory(project) / slot
    slot_path.mkdir()
    source = write_historical_state(project, slot, task=None, purpose=None)

    imported = command(
        project,
        "import-existing",
        slot,
        "--from-state-file",
        source.name,
        "--source-host-id",
        wrkslots._host_id(),
        "--coordinator-pid",
        str(os.getpid()),
        "--apply",
    )

    assert imported.returncode == 0, imported.stderr
    row = active_slots(project)[0]
    assert isinstance(row, dict)
    assert row["checkouts"] == []
    assert row["layout"] == "nested"
    assert row["task"] == "task-old"
    assert row["purpose"] == "no purpose recorded in worktree-state.json"
    expire_heartbeat(project)
    set_liveness(project, "dead")
    removed = remove(project, slot)
    assert removed.returncode == 0, removed.stderr
    assert not slot_path.exists()
    archive = json.loads(
        (control_directory(project) / "ARCHIVED.testhost.json").read_text(
            encoding="utf-8"
        )
    )
    archived = archive["records"][0]
    assert archived["checkouts"] == []
    assert archived["salvage"] == []
    assert archived["validation"] == [
        "historical agent slot contains no checkout and its exact contents were "
        "verified before deletion, so there was no checkout to salvage"
    ]


def test_historical_import_without_owner_sidecar_refuses_before_journal(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    slot = "old-unknown-owner"
    slot_path = slots_directory(project) / slot
    slot_path.mkdir()
    source = write_historical_state(project, slot, include_owner=False)

    refused = command(
        project,
        "import-existing",
        slot,
        "--from-state-file",
        source.name,
        "--source-host-id",
        wrkslots._host_id(),
        "--coordinator-pid",
        str(os.getpid()),
        "--apply",
    )

    assert refused.returncode == 3
    assert "has no owner_sidecar" in refused.stderr
    assert "owner death would remain unknowable" in refused.stderr
    assert "include it in the migration remainder" in refused.stderr
    assert active_slots(project) == []
    assert not (control_directory(project) / "ACTIVE.testhost.journal").exists()
    assert slot_path.is_dir()


def test_historical_import_refuses_a_still_live_recorded_owner(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    slot = "old-live"
    slot_path = slots_directory(project) / slot
    slot_path.mkdir()
    source = write_historical_state(project, slot, live_owner=True)

    refused = command(
        project,
        "import-existing",
        slot,
        "--from-state-file",
        source.name,
        "--source-host-id",
        wrkslots._host_id(),
        "--coordinator-pid",
        str(os.getpid()),
        "--apply",
    )

    assert refused.returncode == 3
    assert "still has its recorded owner alive" in refused.stderr
    assert "ordinary import-existing with --verified-live and --owner-pid" in refused.stderr
    assert "no import journal or active row was written" in refused.stderr
    assert active_slots(project) == []
    assert not (control_directory(project) / "ACTIVE.testhost.journal").exists()
    assert slot_path.is_dir()


def test_historical_import_does_not_turn_an_absent_source_row_into_active_storage(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    slot = "old-absent"
    source = write_historical_state(project, slot)

    refused = command(
        project,
        "import-existing",
        slot,
        "--from-state-file",
        source.name,
        "--source-host-id",
        wrkslots._host_id(),
        "--coordinator-pid",
        str(os.getpid()),
        "--apply",
    )

    assert refused.returncode == 3
    assert "slot directory is missing or unsafe" in refused.stderr
    assert "no import journal or active row was written" in refused.stderr
    assert active_slots(project) == []
    assert not (control_directory(project) / "ACTIVE.testhost.journal").exists()


def test_historical_import_bypasses_cap_but_counts_against_later_create(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    update_configuration(project, max_active_slots=1)
    slot = "old-cap"
    (slots_directory(project) / slot).mkdir()
    source = write_historical_state(
        project,
        slot,
        task=None,
        owner_task="",
    )
    imported = command(
        project,
        "import-existing",
        slot,
        "--from-state-file",
        source.name,
        "--source-host-id",
        wrkslots._host_id(),
        "--coordinator-pid",
        str(os.getpid()),
        "--apply",
    )
    assert imported.returncode == 0, imported.stderr

    refused = create(
        project,
        slot="slot02",
        agent="codex-2",
        branch="codex/two",
    )

    assert refused.returncode == 3
    assert "max_active_slots=1" in refused.stderr
    rows = active_slots(project)
    assert len(rows) == 1
    row = rows[0]
    assert isinstance(row, dict)
    assert row["slot"] == slot
    assert row["task"] == "no task recorded in worktree-state.json"
    assert not (slots_directory(project) / "slot02").exists()


def test_historical_owner_name_is_provenance_not_a_live_assignment(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    for slot in ("old-first", "old-second"):
        (slots_directory(project) / slot).mkdir()
        source = write_historical_state(project, slot)
        imported = command(
            project,
            "import-existing",
            slot,
            "--from-state-file",
            source.name,
            "--source-host-id",
            wrkslots._host_id(),
            "--coordinator-pid",
            str(os.getpid()),
            "--apply",
        )
        assert imported.returncode == 0, imported.stderr

    current = create(
        project,
        slot="current",
        agent="codex-old",
        branch="codex/current",
    )
    assert current.returncode == 0, current.stderr
    duplicate_current = create(
        project,
        slot="current-two",
        agent="codex-old",
        branch="codex/current-two",
    )
    assert duplicate_current.returncode == 3
    assert "already owns slot 'current' on testhost" in duplicate_current.stderr
    rows = active_slots(project)
    assert all(isinstance(row, dict) for row in rows)
    assert sorted(row["slot"] for row in rows if isinstance(row, dict)) == [
        "current",
        "old-first",
        "old-second",
    ]


def test_interrupted_historical_import_is_recovered_by_a_later_participant(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    slot = "old-interrupted"
    slot_path = slots_directory(project) / slot
    slot_path.mkdir()
    source = write_historical_state(project, slot)
    interrupted = self_coordinator_command(
        project,
        "import-existing",
        slot,
        "--slot-type",
        "agent",
        "--coordinator-authorized",
        "--from-state-file",
        source.name,
        "--source-host-id",
        wrkslots._host_id(),
        "--apply",
        env={"WRKSLOTS_TEST_INTERRUPT": "after-import-journal"},
    )

    assert interrupted.returncode == 86
    assert active_slots(project) == []
    journal = control_directory(project) / "ACTIVE.testhost.journal"
    journal_row = json.loads(journal.read_text(encoding="utf-8"))["record"]
    original_coordinator_pid = journal_row["coordinator_lease"]["pid"]
    assert original_coordinator_pid != os.getpid()
    source.unlink()
    journal.unlink()

    recovered = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
    )

    assert recovered.returncode == 0, recovered.stderr
    assert "recovered import: registered slot=old-interrupted" in recovered.stdout
    row = active_slots(project)[0]
    assert isinstance(row, dict)
    assert row["import_source"]["row"] == journal_row["import_source"]["row"]
    events = [
        json.loads(path.read_text(encoding="utf-8"))
        for path in sorted((control_directory(project) / "EVENTS.testhost").glob("*.json"))
    ]
    recovery = [event for event in events if event["kind"] == "active-state-recorded"][-1]
    evidence = recovery["payload"]["evidence"]
    assert evidence["original_coordinator"]["pid"] == original_coordinator_pid
    assert evidence["recovery_actor"]["pid"] == os.getpid()
    assert not journal.exists()
    assert not source.exists()
    assert slot_path.is_dir()


def test_register_refuses_owner_generation_change_before_publication(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    tree = checkout(project)
    tree.parent.mkdir()
    git(repository, "worktree", "add", "-b", "codex/imported", str(tree), "origin/main")
    current = wrkslots._read_process_identity(os.getpid())
    reused = wrkslots.ProcessIdentity(
        pid=current.pid,
        start_ticks=current.start_ticks + 1,
        boot_id=current.boot_id,
        host_id=current.host_id,
        cgroup_path=current.cgroup_path,
    )
    monkeypatch.setattr(
        wrkslots, "_capture_caller_process", lambda _pid, _label: current
    )
    monkeypatch.setattr(wrkslots, "_capture_registration_owner", lambda _pid: current)
    monkeypatch.setattr(wrkslots, "_assert_caller_process", lambda _identity, _label: None)
    monkeypatch.setattr(wrkslots, "_read_process_identity", lambda _pid: reused)
    returncode = wrkslots.main(
        [
            "--project-root",
            str(project),
            "register",
            "slot01",
            "--slot-type",
            "agent",
            "--coordinator-authorized",
            "--agent",
            "codex-1",
            "--task",
            "task-import",
            "--purpose",
            "register generation test",
            "--owner-pid",
            str(os.getpid()),
            "--coordinator-pid",
            str(os.getpid()),
            "--verified-live",
            "--repo",
            "product=repo",
        ]
    )
    captured = capsys.readouterr()

    assert returncode == 3
    assert "owner process generation changed during registration" in captured.err
    assert active_slots(project) == []
    assert tree.is_dir()


def test_path_escape_and_symlink_are_refused_without_touching_target(
    tmp_path: Path,
) -> None:
    project_parent = tmp_path / "project-parent"
    project_parent.mkdir()
    project, _repository, _remote = make_project(project_parent)
    outside = tmp_path / "outside"
    outside.mkdir()
    marker = outside / "keep.txt"
    marker.write_text("keep\n", encoding="utf-8")
    (project / "worktrees" / "slot01").symlink_to(outside, target_is_directory=True)

    symlink_refusal = create(project)
    escape_refusal = command(
        project,
        "create",
        "slot02",
        "--agent",
        "codex-2",
        "--task",
        "task-escape",
        "--purpose",
        "escape",
        "--owner-pid",
        str(os.getpid()),
        "--coordinator-pid",
        str(os.getpid()),
        "--repo",
        "product=../../outside",
        "--branch",
        "product=codex/escape",
    )

    assert symlink_refusal.returncode == 3
    assert escape_refusal.returncode == 3
    assert "other parent traversal is refused" in escape_refusal.stderr
    assert marker.read_text(encoding="utf-8") == "keep\n"
    assert active_slots(project) == []


def test_partial_state_refuses_then_recovers_last_durable_state(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)
    partial = project / "worktrees" / "ACTIVE.testhost.json.tmp.crash"
    partial.write_text("{\n", encoding="utf-8")

    refused = command(project, "status")

    assert refused.returncode == 3
    assert "partial atomic update" in refused.stderr
    recovered = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--discard-partial",
    )
    assert recovered.returncode == 0, recovered.stderr
    assert not partial.exists()


def test_init_recovers_complete_initial_state_temp_without_durable_target(
    tmp_path: Path,
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    initialize(project)
    active_path = project / "worktrees" / "ACTIVE.testhost.json"
    temp = active_path.with_name(f"{active_path.name}.tmp.crash")
    os.replace(active_path, temp)

    initialize(project)

    assert active_path.is_file()
    assert not temp.exists()
    assert active(project)["slots"] == []


def test_init_recovers_complete_configuration_temp_without_durable_target(
    tmp_path: Path,
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    initialize(project)
    config_path = project / ".wrkslots.yml"
    temp = config_path.with_name(f"{config_path.name}.tmp.crash")
    os.replace(config_path, temp)

    initialize(project)

    assert config_path.is_file()
    assert not temp.exists()
    config = json.loads(config_path.read_text(encoding="utf-8"))
    assert config["liveness_command"] == "liveness.py"


def test_create_recovery_refuses_journal_filename_for_another_machine(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    interrupted = create(
        project, env={"WRKSLOTS_TEST_INTERRUPT": "after-create-worktree"}
    )
    assert interrupted.returncode == 86
    original = create_journal_path(project)
    changed = project / "worktrees" / "CREATE.9.machine-b.6.slot01.journal"
    os.replace(original, changed)

    refused = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        machine="testhost",
    )

    assert refused.returncode == 3
    assert "multiple interrupted mutations" in refused.stderr
    assert changed.is_file()
    assert active_slots(project) == []


def test_partial_recovery_preserves_temp_when_durable_state_is_corrupt(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    active_path = project / "worktrees" / "ACTIVE.testhost.json"
    partial = project / "worktrees" / "ACTIVE.testhost.json.tmp.crash"
    partial.write_bytes(active_path.read_bytes())
    active_path.write_text('{"schema": 1}\n', encoding="utf-8")

    recovered = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--discard-partial",
    )

    assert recovered.returncode == 0, recovered.stderr
    assert not partial.exists()
    assert active(project)["slots"] == []
    assert "append-only log" in recovered.stdout


def test_partial_recovery_promotes_a_complete_append_only_event(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)
    config = wrkslots._load_config(str(project), "testhost")
    written = wrkslots._write_event_file(
        config,
        "testhost",
        "test-observation",
        {"fact": "complete before rename"},
    )
    event_path = (
        project
        / "worktrees"
        / "EVENTS.testhost"
        / f"{written['sequence']:020d}.json"
    )
    partial = event_path.with_name(f"{event_path.name}.tmp.crash")
    event_path.rename(partial)

    recovered = raw_command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--discard-partial",
    )

    assert recovered.returncode == 0, recovered.stderr
    assert event_path.is_file()
    assert not partial.exists()
    events = wrkslots._load_events(config)
    assert any(event["kind"] == "test-observation" for event in events)
    assert events[-1]["kind"] == "partial-updates-recovered"


def test_doctor_reports_an_interrupted_append_only_event_write(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)
    event_directory = project / "worktrees" / "EVENTS.testhost"
    partial = event_directory / "00000000000000000002.json.tmp.crash"
    partial.write_text("incomplete\n", encoding="utf-8")

    diagnosed = command(
        project, "doctor", "--all-machines", "--format", "json"
    )

    assert diagnosed.returncode == 0, diagnosed.stderr
    findings = json.loads(diagnosed.stdout)["findings"]
    assert any(
        finding["kind"] == "partial-atomic-update"
        and partial.name in finding["detail"]
        for finding in findings
    )


def test_corrupt_archive_view_does_not_override_append_only_history(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    archive = project / "worktrees" / "ARCHIVED.testhost.json"
    archive.write_text('{"schema": 1, "machine": "testhost"}\n', encoding="utf-8")

    finished = finish(project)

    assert finished.returncode == 0, finished.stderr
    assert checkout(project).is_dir()
    assert len(active_slots(project)) == 1
    assert not (project / "worktrees" / "ACTIVE.testhost.journal").exists()


def test_malformed_active_view_does_not_override_append_only_history(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    active_path = project / "worktrees" / "ACTIVE.testhost.json"
    state = json.loads(active_path.read_text(encoding="utf-8"))
    state["slots"][0]["created_at"] = "not-a-timestamp"
    active_path.write_text(json.dumps(state, indent=2) + "\n", encoding="utf-8")

    finished = finish(project)

    assert finished.returncode == 0, finished.stderr
    assert checkout(project).is_dir()
    archive = json.loads(
        (project / "worktrees" / "ARCHIVED.testhost.json").read_text(encoding="utf-8")
    )
    assert archive["records"] == []


def test_stale_generation_and_second_slot_for_same_agent_are_refused(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    stale = command(
        project,
        "heartbeat",
        "slot01",
        "--agent",
        "codex-1",
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "2",
    )
    second = create(
        project,
        slot="slot02",
        agent="codex-1",
        branch="codex/second",
    )
    assert stale.returncode == 3
    assert "stale generation" in stale.stderr
    assert second.returncode == 3
    assert "already owns" in second.stderr
    assert checkout(project).is_dir()
    assert not (project / "worktrees" / "slot02").exists()


def test_process_identity_distinguishes_remote_host_from_finished_boot() -> None:
    current = wrkslots._read_process_identity(os.getpid())
    remote_host = wrkslots.ProcessIdentity(
        pid=current.pid,
        start_ticks=current.start_ticks,
        boot_id=current.boot_id,
        host_id=f"different-{current.host_id}",
        cgroup_path=current.cgroup_path,
    )
    prior_boot = wrkslots.ProcessIdentity(
        pid=current.pid,
        start_ticks=current.start_ticks,
        boot_id=f"different-{current.boot_id}",
        host_id=current.host_id,
        cgroup_path=current.cgroup_path,
    )

    assert wrkslots._process_state(remote_host)[0] == "indeterminate"
    assert wrkslots._process_state(prior_boot)[0] == "dead"


def test_lsof_warning_must_be_proven_unrelated_to_slot() -> None:
    slot = Path("/worktrees/slot01")
    unrelated = "lsof: WARNING: can't stat() tmpfs file system /dev/shm/private\n"
    unrelated_directory = (
        "lsof: WARNING: can't opendir(/worktrees/other): Permission denied\n"
    )
    relevant = "lsof: WARNING: can't stat() btrfs file system /worktrees\n"
    relevant_directory = (
        "lsof: WARNING: can't opendir(/worktrees/slot01/private): Permission denied\n"
    )
    unknown = "lsof: WARNING: unexpected text\n"

    assert wrkslots._unrelated_lsof_warnings(unrelated, slot)
    assert wrkslots._unrelated_lsof_warnings(unrelated_directory, slot)
    assert not wrkslots._unrelated_lsof_warnings(relevant, slot)
    assert not wrkslots._unrelated_lsof_warnings(relevant_directory, slot)
    assert not wrkslots._unrelated_lsof_warnings(unknown, slot)


def test_changed_create_journal_cannot_redirect_recovery(tmp_path: Path) -> None:
    project, repository, _remote = make_project(tmp_path)
    interrupted = create(
        project,
        env={"WRKSLOTS_TEST_INTERRUPT": "after-create-worktree"},
    )
    assert interrupted.returncode == 86
    journal_path = create_journal_path(project)
    journal = json.loads(journal_path.read_text(encoding="utf-8"))
    journal["planned"][0]["destination"] = "repo/redirected"
    journal_path.write_text(json.dumps(journal, indent=2) + "\n", encoding="utf-8")

    refused = command(
        project, "recover", "--coordinator-pid", str(os.getpid())
    )

    assert refused.returncode == 3
    assert "differs from append-only progress evidence" in refused.stderr
    assert checkout(project).is_dir()
    assert repository.is_dir()
    assert not (repository / "redirected").exists()
    assert active_slots(project) == []


def test_completed_finish_recovery_requires_exact_archive_entry(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    journal_path = prepare_removal_journal(project)

    def interrupt_before_journal_removal(_path: Path) -> None:
        raise RuntimeError("simulated stop after durable state updates")

    monkeypatch.setattr(wrkslots, "_remove_control_file", interrupt_before_journal_removal)
    with pytest.raises(RuntimeError, match="simulated stop"):
        wrkslots.main(
            [
                "--project-root",
                str(project),
                    "recover",
                    "--coordinator-authorized",
                    "--coordinator-pid",
                    str(os.getpid()),
                ]
        )
    monkeypatch.undo()
    assert journal_path.is_file()
    assert active_slots(project) == []
    archive_path = project / "worktrees" / "ARCHIVED.testhost.json"
    archive = json.loads(archive_path.read_text(encoding="utf-8"))
    archive["records"][0]["purpose"] = "different but structurally valid"
    archive_path.write_text(json.dumps(archive, indent=2) + "\n", encoding="utf-8")

    recovered = command(
        project, "recover", "--coordinator-pid", str(os.getpid())
    )

    assert recovered.returncode == 0, recovered.stderr
    assert not journal_path.exists()
    repaired = json.loads(archive_path.read_text(encoding="utf-8"))
    assert repaired["records"][0]["purpose"] != "different but structurally valid"
    assert not (project / "worktrees" / "slot01").exists()


def _repair(
    project: Path, *extra: str, machine: str = "testhost"
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [
            sys.executable, str(WRKSLOTS), "--machine", machine, "init", str(project),
            "--worktrees-dir", "worktrees", "--liveness-command", "liveness.py",
            "--repair", *extra,
        ],
        text=True, capture_output=True, check=False,
    )


def remove_event_history_to_simulate_older_build(
    project: Path, machine: str = "testhost"
) -> None:
    """Turn current init output into the pre-event layout these migration tests need."""
    shutil.rmtree(project / "worktrees" / f"EVENTS.{machine}")


def test_configuration_from_an_older_build_is_dead_until_repaired(
    tmp_path: Path,
) -> None:
    """A config missing a field this build requires must not be a one-way door.

    Before the repair path existed this state was terminal: every command
    refused for the missing field, and `init` refused to touch the file because
    it "already has different configuration". Nothing could read it and nothing
    could fix it.
    """
    project, _repository, _remote = make_project(tmp_path)
    config_path = project / ".wrkslots.yml"
    stale = json.loads(config_path.read_text(encoding="utf-8"))
    del stale["liveness_command"]
    stale["schema"] = 1
    config_path.write_text(json.dumps(stale, indent=2, sort_keys=True) + "\n", encoding="utf-8")

    wedged = command(project, "status", "--all-machines")
    assert wedged.returncode == 3
    assert "missing liveness_command" in wedged.stderr
    assert "init --repair" in wedged.stderr

    repaired = _repair(project)
    assert repaired.returncode == 0, repaired.stderr
    assert "added liveness_command" in repaired.stdout
    assert "schema 1 -> 2" in repaired.stdout

    healthy = command(project, "status", "--all-machines")
    assert healthy.returncode == 0, healthy.stderr
    current = json.loads(config_path.read_text(encoding="utf-8"))
    assert current["schema"] == wrkslots.SCHEMA
    assert current["liveness_command"] == "liveness.py"
    # Repair must not have disturbed anything else.
    assert current["machine"] == stale["machine"]
    assert current["worktrees_dir"] == stale["worktrees_dir"]
    assert current["heartbeat_ttl_seconds"] == stale["heartbeat_ttl_seconds"]


def test_empty_state_from_an_older_build_is_dead_until_repaired(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    remove_event_history_to_simulate_older_build(project)
    active_path = project / "worktrees" / "ACTIVE.testhost.json"
    archive_path = project / "worktrees" / "ARCHIVED.testhost.json"
    for path in (active_path, archive_path):
        state = json.loads(path.read_text(encoding="utf-8"))
        state["schema"] = 1
        path.write_text(
            json.dumps(state, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )

    refused = command(project, "status", "--all-machines", "--format", "json")
    assert refused.returncode == 3
    assert "unsupported active state schema" in refused.stderr
    assert refused.stdout == ""

    repaired = _repair(project)
    assert repaired.returncode == 0, repaired.stderr
    assert f"REPAIRED {active_path}: schema 1 -> {wrkslots.SCHEMA}" in repaired.stdout
    assert f"REPAIRED {archive_path}: schema 1 -> {wrkslots.SCHEMA}" in repaired.stdout

    healthy = command(project, "status", "--all-machines", "--format", "json")
    assert healthy.returncode == 0, healthy.stderr
    assert json.loads(healthy.stdout)["active"] == []
    assert json.loads(active_path.read_text(encoding="utf-8"))["schema"] == wrkslots.SCHEMA
    assert json.loads(archive_path.read_text(encoding="utf-8"))["schema"] == wrkslots.SCHEMA


def test_state_repair_refuses_non_empty_state_without_changes(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    remove_event_history_to_simulate_older_build(project)
    active_path = project / "worktrees" / "ACTIVE.testhost.json"
    archive_path = project / "worktrees" / "ARCHIVED.testhost.json"
    active = json.loads(active_path.read_text(encoding="utf-8"))
    active["schema"] = 1
    active["revision"] = 1
    active_path.write_text(
        json.dumps(active, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    archive_path.write_text('{"schema": 1}\n', encoding="utf-8")
    before_active = active_path.read_bytes()
    before_archive = archive_path.read_bytes()

    refused = _repair(project)

    assert refused.returncode == 3
    assert "cannot repair non-empty active state" in refused.stderr
    assert active_path.read_bytes() == before_active
    assert archive_path.read_bytes() == before_archive
    status = command(project, "status", "--all-machines", "--format", "json")
    assert status.returncode == 3
    assert status.stdout == ""


def test_state_repair_refuses_a_malformed_file_without_reporting_empty(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    remove_event_history_to_simulate_older_build(project)
    active_path = project / "worktrees" / "ACTIVE.testhost.json"
    active_path.write_text('{"schema": 1}\n', encoding="utf-8")
    before = active_path.read_bytes()

    refused = _repair(project)

    assert refused.returncode == 3
    assert "active state has invalid fields" in refused.stderr
    assert active_path.read_bytes() == before
    status = command(project, "status", "--all-machines", "--format", "json")
    assert status.returncode == 3
    assert "active state has invalid fields" in status.stderr
    assert status.stdout == ""


def test_repair_updates_the_pre_packaging_control_symlink(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)
    link = project / "worktrees" / "wrkslots"
    source_alias = tmp_path / "agent-utils"
    source_alias.symlink_to(PY_ROOT.parent, target_is_directory=True)
    old_command_path = source_alias / "py" / "wrkslots.py"
    old_target = Path(os.path.relpath(old_command_path, start=link.parent))
    link.unlink()
    link.symlink_to(old_target)
    old_command = subprocess.run(
        [str(link), "--version"],
        text=True,
        capture_output=True,
        check=False,
    )
    assert old_command.returncode == 0, old_command.stderr
    assert old_command.stdout.strip() == "wrkslots 0.6.0"

    repaired = _repair(project)

    assert repaired.returncode == 0, repaired.stderr
    assert f"REPAIRED {link}:" in repaired.stdout
    assert link.resolve() == WRKSLOTS.resolve()
    status = subprocess.run(
        [str(link), "--project-root", str(project), "status", "--all-machines"],
        text=True,
        capture_output=True,
        check=False,
    )
    assert status.returncode == 0, status.stderr


def test_repair_refuses_to_relocate_live_state_and_lists_every_conflict(
    tmp_path: Path,
) -> None:
    """Repair adds missing fields; it must never redirect the tool elsewhere."""
    project, _repository, _remote = make_project(tmp_path)
    config_path = project / ".wrkslots.yml"
    stale = json.loads(config_path.read_text(encoding="utf-8"))
    before = dict(stale)
    stale["worktrees_dir"] = "somewhere-else"
    stale["default_remote"] = "upstream"
    config_path.write_text(json.dumps(stale, indent=2, sort_keys=True) + "\n", encoding="utf-8")

    refused = _repair(project)

    assert refused.returncode == 3
    assert "worktrees_dir" in refused.stderr
    # Every conflict at once, not one per run.
    assert "default_remote" in refused.stderr
    unchanged = json.loads(config_path.read_text(encoding="utf-8"))
    assert unchanged["worktrees_dir"] == "somewhere-else"
    assert unchanged["default_remote"] == "upstream"
    assert before["worktrees_dir"] != unchanged["worktrees_dir"]


def test_repair_never_reinterprets_implicit_nested_layout_as_flat(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    config_path = project / ".wrkslots.yml"
    before = config_path.read_bytes()

    refused = _repair(project, "--layout", "flat")

    assert refused.returncode == 3
    assert "absent means nested" in refused.stderr
    assert config_path.read_bytes() == before
    assert (project / "worktrees" / "ACTIVE.testhost.json").is_file()
    assert not (project / "ACTIVE.testhost.json").exists()


def test_init_treats_explicit_optional_defaults_as_idempotent(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)
    config_path = project / ".wrkslots.yml"
    config = json.loads(config_path.read_text(encoding="utf-8"))
    config["layout"] = "nested"
    config["cache_globs"] = []
    config["post_provision_hooks"] = []
    config_path.write_text(
        json.dumps(config, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )

    repeated = _repair(project)

    assert repeated.returncode == 0, repeated.stderr
    assert "UNCHANGED" in repeated.stdout


def test_active_slot_cap_refuses_the_allocation_that_would_breach_it(
    tmp_path: Path,
) -> None:
    """A cap that only appears in a report is not a cap."""
    project, repository, _remote = make_project(tmp_path)
    assert _repair(project, "--max-active-slots", "1").returncode == 0

    first = create(project, slot="slot01", agent="codex-1", branch="codex/one")
    assert first.returncode == 0, first.stderr

    second = create(project, slot="slot02", agent="codex-2", branch="codex/two")

    assert second.returncode == 3
    assert "max_active_slots=1" in second.stderr
    assert "slot01" in second.stderr
    # The refused allocation left nothing behind.
    assert [
        row["slot"] for row in active_slots(project) if isinstance(row, dict)
    ] == ["slot01"]
    assert not (project / "worktrees" / "slot02").exists()
    assert git(repository, "worktree", "list").stdout.count("slot02") == 0


def test_absent_cap_means_uncapped_and_init_stays_idempotent(tmp_path: Path) -> None:
    """Absent must keep meaning "no cap", or every pre-existing config changes."""
    project, _repository, _remote = make_project(tmp_path)
    config = json.loads((project / ".wrkslots.yml").read_text(encoding="utf-8"))
    assert "max_active_slots" not in config

    assert create(project, slot="slot01", agent="codex-1", branch="codex/one").returncode == 0
    second = create(project, slot="slot02", agent="codex-2", branch="codex/two")
    assert second.returncode == 0, second.stderr


def test_status_and_doctor_report_every_disagreement_at_once(
    tmp_path: Path,
) -> None:
    """Read-only views show the whole drift without relabelling it healthy."""
    project, repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    git(repository, "worktree", "remove", "--force", str(checkout(project)))
    (project / "worktrees" / "slot01").rmdir()
    (project / "worktrees" / "orphan-a").mkdir()
    (project / "worktrees" / "orphan-b").mkdir()

    status = command(project, "status", "--all-machines", "--format", "json")
    assert status.returncode == 0, status.stderr
    status_payload = json.loads(status.stdout)
    assert status_payload["registry_storage_state"] == "inconsistent"
    assert [row["slot"] for row in status_payload["active"]] == ["slot01"]
    status_findings = status_payload["registry_storage_inconsistencies"]
    assert {item["kind"] for item in status_findings} >= {
        "row-without-directory",
        "directory-without-row",
    }
    assert sorted(
        item["slot"]
        for item in status_findings
        if item["kind"] == "directory-without-row"
    ) == ["orphan-a", "orphan-b"]

    report = command(project, "doctor", "--all-machines", "--format", "json")
    assert report.returncode == 0, report.stderr
    findings = json.loads(report.stdout)["findings"]
    kinds = [item["kind"] for item in findings]
    assert "row-without-directory" in kinds
    orphans = sorted(
        item["slot"] for item in findings if item["kind"] == "directory-without-row"
    )
    assert orphans == ["orphan-a", "orphan-b"], findings
    # Diagnosis authorizes nothing and changes nothing.
    assert (project / "worktrees" / "orphan-a").is_dir()
    assert len(active_slots(project)) == 1


def test_create_ignores_unrelated_drift_but_refuses_target_collisions(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    git(repository, "worktree", "remove", "--force", str(checkout(project)))
    (project / "worktrees" / "slot01").rmdir()
    (project / "worktrees" / "orphan").mkdir()

    distinct = create(
        project,
        slot="slot02",
        agent="codex-2",
        branch="codex/distinct",
    )

    assert distinct.returncode == 0, distinct.stderr
    assert "registry/storage state remains INCONSISTENT" in distinct.stderr
    assert "kind=row-without-directory" in distinct.stderr
    assert "kind=directory-without-row" in distinct.stderr
    assert "retained_storage_inconsistencies=2" in distinct.stdout
    assert checkout(project, "slot02").is_dir()

    same_name = create(
        project,
        slot="slot02",
        agent="codex-3",
        branch="codex/same-name",
    )
    assert same_name.returncode == 3
    assert "slot 'slot02' is already active" in same_name.stderr

    same_path = project / "worktrees" / "occupied"
    same_path.mkdir()
    occupied = create(
        project,
        slot="occupied",
        agent="codex-4",
        branch="codex/same-path",
    )
    assert occupied.returncode == 3
    assert f"slot path already exists: {same_path}" in occupied.stderr

    registered = checkout(project, "git-held")
    git(
        repository,
        "worktree",
        "add",
        "-b",
        "codex/stale-registration",
        str(registered),
        "origin/main",
    )
    assert registered.is_dir()
    shutil.rmtree(registered.parent)
    assert not registered.exists()
    assert str(registered) in git(repository, "worktree", "list", "--porcelain").stdout

    git_collision = create(
        project,
        slot="git-held",
        agent="codex-5",
        branch="codex/new-registration",
    )
    assert git_collision.returncode == 3
    assert "overlaps Git-registered worktree" in git_collision.stderr
    assert not registered.exists()


@pytest.mark.parametrize("registration_location", ["slot-root", "descendant"])
def test_create_refuses_git_registration_overlapping_target_slot(
    tmp_path: Path, registration_location: str
) -> None:
    """A stale Git registration cannot be hidden beside the planned checkout name."""
    project, repository, _remote = make_project(tmp_path)
    target_slot = project / "worktrees" / "slot01"
    registered = (
        target_slot
        if registration_location == "slot-root"
        else target_slot / "stale-checkout"
    )
    registered.parent.mkdir(parents=True, exist_ok=True)
    git(
        repository,
        "worktree",
        "add",
        "-b",
        f"codex/stale-{registration_location}",
        str(registered),
        "origin/main",
    )
    shutil.rmtree(target_slot)
    assert not target_slot.exists()

    refused = create(project)

    assert refused.returncode == 3
    assert f"requested slot path {target_slot} overlaps Git-registered worktree" in refused.stderr
    assert str(registered) in refused.stderr
    assert not target_slot.exists()
    assert active_slots(project) == []


def test_status_reports_missing_recorded_repository_but_create_refuses(
    tmp_path: Path,
) -> None:
    """Missing Git evidence is row-local for status and fail-closed for mutation."""
    project, repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    unavailable = project / "repository-unavailable"
    repository.rename(unavailable)

    status = command(project, "status", "--all-machines", "--format", "json")

    assert status.returncode == 0, status.stderr
    payload = json.loads(status.stdout)
    assert [row["slot"] for row in payload["active"]] == ["slot01"]
    assert payload["registry_storage_state"] == "inconsistent"
    findings = payload["registry_storage_inconsistencies"]
    unavailable_findings = [
        item for item in findings if item["kind"] == "repository-evidence-unavailable"
    ]
    assert len(unavailable_findings) == 1
    assert unavailable_findings[0]["slot"] == "slot01"
    assert unavailable_findings[0]["checkout"] == "product"
    assert "restore the recorded source repository" in unavailable_findings[0]["remedy"]
    assert payload["active"][0]["storage_inconsistencies"] == unavailable_findings

    refused = create(
        project,
        slot="slot02",
        agent="codex-2",
        branch="codex/unavailable-source",
    )
    assert refused.returncode == 3
    assert "source repository 'repo' does not exist or cannot be resolved" in refused.stderr
    assert not checkout(project, "slot02").exists()


def test_status_reports_git_registration_without_an_active_row(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    stale = project / "worktrees" / "stale-slot" / "product"
    stale.parent.mkdir()
    git(
        repository,
        "worktree",
        "add",
        "-b",
        "codex/stale-status-registration",
        str(stale),
        "origin/main",
    )
    shutil.rmtree(stale.parent)

    status = command(project, "status", "--all-machines", "--format", "json")

    assert status.returncode == 0, status.stderr
    payload = json.loads(status.stdout)
    findings = [
        item
        for item in payload["registry_storage_inconsistencies"]
        if item["kind"] == "git-registration-without-row"
    ]
    assert len(findings) == 1
    assert findings[0]["scope"] == "directory"
    assert findings[0]["slot"] == "stale-slot"
    assert findings[0]["slot_type"] == "agent"
    assert str(stale) in findings[0]["detail"]
    assert payload["active"][0]["storage_inconsistencies"] == []

    # ⚠️ THIS HALF ASSERTED THE DEFECT, NOT THE REQUIREMENT, AND IS CHANGED
    # DELIBERATELY. It required `heartbeat slot01` to be REFUSED by a stranded
    # Git registration belonging to `stale-slot` -- a slot this command does not
    # name, does not touch, and cannot repair. That is the exact condition five
    # agents hit from the outside on 2026-09-04: each asked to operate on its own
    # slot, read a message naming a slot it had never seen, investigated its own
    # state, found nothing wrong, and correctly declined to touch another lane's
    # worktree. Nobody saw it was one directory refusing for everyone.
    #
    # The refusal is NOT removed. It is retained for the slot the command names,
    # asserted immediately below, and every other finding is reported by name
    # instead of stopping unrelated work.
    heartbeat = command(
        project,
        "heartbeat",
        "slot01",
        "--agent",
        "codex-1",
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
    )
    assert heartbeat.returncode == 0, heartbeat.stderr
    assert "stale-slot" in heartbeat.stderr
    assert "RETAINED" in heartbeat.stderr
    # Reported, not swept: the unrelated registration is untouched afterwards.
    assert str(stale) in git(repository, "worktree", "list", "--porcelain").stdout

    # The other direction, in the same test so neither can be changed alone: a
    # stranded registration under the slot the command NAMES still refuses, and
    # the refusal still names that slot.
    own = checkout(project) / "stale-checkout"
    git(
        repository,
        "worktree",
        "add",
        "-b",
        "codex/stale-own-registration",
        str(own),
        "origin/main",
    )
    shutil.rmtree(own)
    before_events = tuple(
        path.read_bytes()
        for path in sorted((project / "worktrees" / "EVENTS.testhost").glob("*.json"))
    )
    refused = command(
        project,
        "heartbeat",
        "slot01",
        "--agent",
        "codex-1",
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
    )
    assert refused.returncode == 3, refused.stdout
    assert "Git common directory" in refused.stderr
    assert "no active checkout row" in refused.stderr
    assert "slot01" in refused.stderr
    after_events = tuple(
        path.read_bytes()
        for path in sorted((project / "worktrees" / "EVENTS.testhost").glob("*.json"))
    )
    assert after_events == before_events


def test_status_attaches_extra_registration_inside_active_slot_to_row(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    extra = project / "worktrees" / "slot01" / "stale-registration"
    git(
        repository,
        "worktree",
        "add",
        "-b",
        "codex/stale-inside-active-slot",
        str(extra),
        "origin/main",
    )

    status = command(project, "status", "--all-machines", "--format", "json")

    assert status.returncode == 0, status.stderr
    row = json.loads(status.stdout)["active"][0]
    findings = row["storage_inconsistencies"]
    registration = next(
        item for item in findings if item["kind"] == "git-registration-without-row"
    )
    assert registration["scope"] == "row"
    assert registration["machine"] == "testhost"
    assert registration["slot"] == "slot01"
    assert str(extra) in registration["detail"]


def test_status_reports_unresolvable_checkout_head_and_create_warns(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    tree = checkout(project)
    head_path = Path(
        git(tree, "rev-parse", "--path-format=absolute", "--git-path", "HEAD")
        .stdout.strip()
    )
    head_path.write_text("ref: refs/heads/missing-after-record\n", encoding="utf-8")

    status = command(project, "status", "--all-machines", "--format", "json")

    assert status.returncode == 0, status.stderr
    row_findings = json.loads(status.stdout)["active"][0]["storage_inconsistencies"]
    unreadable = next(
        item for item in row_findings if item["kind"] == "git-worktree-unreadable"
    )
    assert "cannot verify recorded checkout" in unreadable["detail"]

    distinct = create(
        project,
        slot="slot02",
        agent="codex-2",
        branch="codex/distinct-from-broken-head",
    )
    assert distinct.returncode == 0, distinct.stderr
    assert "kind=git-worktree-unreadable" in distinct.stderr
    assert checkout(project, "slot02").is_dir()


def test_status_does_not_invent_missing_entries_when_a_slot_is_unreadable(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    config = wrkslots._load_config(str(project), "testhost")
    states, _archives = wrkslots._validate_global_state(config)
    slot_path = project / "worktrees" / "slot01"
    original_iterdir = Path.iterdir

    def guarded_iterdir(path: Path) -> Iterator[Path]:
        if path == slot_path:
            raise OSError("injected unreadable slot")
        return original_iterdir(path)

    monkeypatch.setattr(Path, "iterdir", guarded_iterdir)

    findings = wrkslots._registry_storage_inconsistencies(config, states)

    row_kinds = {
        item.kind for item in findings if item.machine == "testhost" and item.slot == "slot01"
    }
    assert "slot-unreadable" in row_kinds
    assert "missing-checkout" not in row_kinds
    assert "unexpected-entry" not in row_kinds


def test_status_reports_symlinked_slot_without_hiding_roster(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    tree = checkout(project)
    git(repository, "worktree", "remove", "--force", str(tree))
    slot_path = project / "worktrees" / "slot01"
    slot_path.rmdir()
    external = tmp_path / "external-slot"
    external.mkdir()
    slot_path.symlink_to(external, target_is_directory=True)

    status = command(project, "status", "--all-machines", "--format", "json")

    assert status.returncode == 0, status.stderr
    payload = json.loads(status.stdout)
    assert [row["slot"] for row in payload["active"]] == ["slot01"]
    row_findings = payload["active"][0]["storage_inconsistencies"]
    unsafe = next(
        item for item in row_findings if item["kind"] == "unsafe-slot-directory"
    )
    assert unsafe["scope"] == "row"
    assert str(slot_path) in unsafe["detail"]

    heartbeat = command(
        project,
        "heartbeat",
        "slot01",
        "--agent",
        "codex-1",
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
    )
    assert heartbeat.returncode == 3
    assert "slot crosses a symlink" in heartbeat.stderr


def test_status_reports_missing_slot_and_repository_as_independent_facts(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    git(repository, "worktree", "remove", "--force", str(checkout(project)))
    (project / "worktrees" / "slot01").rmdir()
    repository.rename(project / "repository-unavailable")

    status = command(project, "status", "--all-machines", "--format", "json")

    assert status.returncode == 0, status.stderr
    findings = json.loads(status.stdout)["active"][0]["storage_inconsistencies"]
    assert {item["kind"] for item in findings} >= {
        "row-without-directory",
        "repository-evidence-unavailable",
    }


def test_status_reports_unregistered_symlink_under_managed_root(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    external = tmp_path / "external-unregistered"
    external.mkdir()
    orphan = project / "worktrees" / "orphan-link"
    orphan.symlink_to(external, target_is_directory=True)

    status = command(project, "status", "--all-machines", "--format", "json")

    assert status.returncode == 0, status.stderr
    payload = json.loads(status.stdout)
    assert payload["registry_storage_state"] == "inconsistent"
    finding = next(
        item
        for item in payload["registry_storage_inconsistencies"]
        if item["slot"] == "orphan-link"
    )
    assert finding["kind"] == "directory-without-row"
    assert finding["scope"] == "directory"
    assert orphan.is_symlink()


def test_status_parses_archived_row_when_old_slot_path_is_symlinked(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    commit_task(repository, checkout(project), "codex/task")
    handed_off = finish(project)
    assert handed_off.returncode == 0, handed_off.stderr
    mark_owner_dead(project)
    set_liveness(project, "dead")
    removed = remove(project)
    assert removed.returncode == 0, removed.stderr
    old_slot = project / "worktrees" / "slot01"
    external = tmp_path / "external-archived-slot"
    external.mkdir()
    old_slot.symlink_to(external, target_is_directory=True)

    status = command(project, "status", "--all-machines", "--format", "json")

    assert status.returncode == 0, status.stderr
    payload = json.loads(status.stdout)
    assert payload["active"] == []
    assert payload["registry_storage_state"] == "inconsistent"
    assert any(
        item["slot"] == "slot01" and item["kind"] == "directory-without-row"
        for item in payload["registry_storage_inconsistencies"]
    )


def test_create_reports_unrelated_git_registration_from_new_repository(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    stale = project / "worktrees" / "stale-slot" / "product"
    stale.parent.mkdir()
    git(
        repository,
        "worktree",
        "add",
        "-b",
        "codex/stale-before-first-row",
        str(stale),
        "origin/main",
    )
    shutil.rmtree(stale.parent)

    distinct = create(
        project,
        slot="slot02",
        agent="codex-2",
        branch="codex/distinct-from-stale",
    )

    assert distinct.returncode == 0, distinct.stderr
    assert "registry/storage state remains INCONSISTENT" in distinct.stderr
    assert "kind=git-registration-without-row" in distinct.stderr
    assert str(stale) in distinct.stderr
    assert "retained_storage_inconsistencies=1" in distinct.stdout
    assert checkout(project, "slot02").is_dir()


def test_create_accepts_the_documented_project_root_source_repository(
    tmp_path: Path,
) -> None:
    remote = tmp_path / "remote.git"
    project = tmp_path / "project"
    subprocess.run(
        ["git", "init", "--bare", "--initial-branch=main", str(remote)],
        check=True,
        capture_output=True,
        text=True,
    )
    subprocess.run(
        ["git", "clone", str(remote), str(project)],
        check=True,
        capture_output=True,
        text=True,
    )
    git(project, "config", "user.name", "Wrkslots Test")
    git(project, "config", "user.email", "wrkslots@example.invalid")
    (project / "seed.txt").write_text("seed\n", encoding="utf-8")
    git(project, "add", "seed.txt")
    git(project, "commit", "-m", "seed")
    git(project, "push", "-u", "origin", "main")
    initialize(project)

    made = create(project, repository_name=".")

    assert made.returncode == 0, made.stderr
    assert checkout(project).is_dir()


def test_create_refuses_a_foreign_registered_ancestor_of_target(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    config = wrkslots._load_config(str(project), "testhost")
    destination, _path = wrkslots._checkout_path(
        config, "slot01", "product", "agent"
    )
    plan = (
        wrkslots.PlannedCheckout(
            name="product",
            destination=destination,
            repository="repo",
            branch="codex/task",
            start_point="0" * 40,
            remote="origin",
            remote_url_sha256="0" * 64,
            landed_ref="refs/remotes/origin/main",
        ),
    )
    foreign_ancestor = project / "worktrees"

    class ForeignAncestorVcs(wrkslots._GitVcs):
        def listed_worktrees(self, source: Path) -> set[Path]:
            assert source == repository
            return {source.absolute(), foreign_ancestor.absolute()}

    with pytest.raises(
        wrkslots.Refusal, match="overlaps Git-registered worktree"
    ):
        wrkslots._assert_create_target_clear(
            config,
            (),
            "slot01",
            "agent",
            plan,
            ForeignAncestorVcs(),
        )


def test_status_reports_malformed_journal_without_hiding_roster(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    journal = project / "worktrees" / "ACTIVE.testhost.journal"
    journal.write_text("{broken\n", encoding="utf-8")

    status = command(project, "status", "--all-machines", "--format", "json")

    assert status.returncode == 0, status.stderr
    payload = json.loads(status.stdout)
    assert [row["slot"] for row in payload["active"]] == ["slot01"]
    assert payload["journals"] == [journal.name]
    assert payload["registry_storage_state"] == "inconsistent"
    finding = next(
        item
        for item in payload["registry_storage_inconsistencies"]
        if item["kind"] == "journal-unreadable"
    )
    assert finding["scope"] == "journal"
    assert finding["machine"] == "testhost"
    assert str(journal) in finding["detail"]

    refused = create(
        project,
        slot="slot02",
        agent="codex-2",
        branch="codex/blocked-by-journal",
    )
    assert refused.returncode == 3
    assert "recovery journal is malformed JSON" in refused.stderr
    assert not checkout(project, "slot02").exists()


@pytest.mark.parametrize("source", ["standalone", "append-only"])
def test_status_reports_structurally_incomplete_journal_from_each_source(
    tmp_path: Path, source: str
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    config = wrkslots._load_config(str(project), "testhost")
    journal = project / "worktrees" / "ACTIVE.testhost.journal"
    incomplete: dict[str, object] = {
        "schema": wrkslots.SCHEMA,
        "machine": "testhost",
        "kind": "create",
        "slot": "slot01",
    }
    if source == "standalone":
        journal.write_text(json.dumps(incomplete) + "\n", encoding="utf-8")
    else:
        wrkslots._write_event_file(
            config,
            "testhost",
            "operation-progress-recorded",
            {"slot": "slot01", "operation": "create", "journal": incomplete},
        )

    status = command(project, "status", "--all-machines", "--format", "json")

    assert status.returncode == 0, status.stderr
    payload = json.loads(status.stdout)
    assert [row["slot"] for row in payload["active"]] == ["slot01"]
    assert payload["journals"] == [journal.name]
    finding = next(
        item
        for item in payload["registry_storage_inconsistencies"]
        if item["kind"] == "journal-unreadable"
    )
    assert "create journal has invalid fields" in finding["detail"]
    assert "agent" in finding["detail"]


def test_status_reports_semantically_invalid_finish_journal(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    commit_task(repository, checkout(project), "codex/task")
    handed_off = finish(project)
    assert handed_off.returncode == 0, handed_off.stderr
    mark_owner_dead(project)
    set_liveness(project, "dead")
    interrupted = command(
        project,
        "remove",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        env={"WRKSLOTS_TEST_INTERRUPT": "after-finish-journal"},
    )
    assert interrupted.returncode == 86
    journal = project / "worktrees" / "ACTIVE.testhost.journal"
    raw = json.loads(journal.read_text(encoding="utf-8"))
    raw["mode"] = "bogus"
    raw["actor"] = "not-coordinator"
    raw["phase"] = "nonsense"
    journal.write_text(json.dumps(raw, indent=2) + "\n", encoding="utf-8")
    shutil.rmtree(project / "worktrees" / "EVENTS.testhost")

    status = command(project, "status", "--all-machines", "--format", "json")

    assert status.returncode == 0, status.stderr
    payload = json.loads(status.stdout)
    assert [row["slot"] for row in payload["active"]] == ["slot01"]
    finding = next(
        item
        for item in payload["registry_storage_inconsistencies"]
        if item["kind"] == "journal-unreadable"
    )
    assert "finish journal actor does not match its operation" in finding["detail"]


@pytest.mark.parametrize(
    ("stored_repository", "expected_error"),
    (
        ("worktrees/recorded-source", "outside the managed worktrees directory"),
        ("../project", "aliases the project root"),
    ),
)
def test_status_refuses_an_ambiguous_stored_repository_identity(
    tmp_path: Path, stored_repository: str, expected_error: str
) -> None:
    """Read-only tolerance applies to availability, never ambiguous authority."""
    project, _repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    shutil.rmtree(project / "worktrees" / "EVENTS.testhost")
    state_path = project / "worktrees" / "ACTIVE.testhost.json"
    state = json.loads(state_path.read_text(encoding="utf-8"))
    state["slots"][0]["checkouts"][0]["repository"] = stored_repository
    state_path.write_text(json.dumps(state, indent=2) + "\n", encoding="utf-8")

    refused = command(project, "status", "--all-machines", "--format", "json")

    assert refused.returncode == 3
    assert expected_error in refused.stderr
    assert refused.stdout == ""


def test_flat_layout_keeps_controls_beside_slots_and_completes_lifecycle(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
        cache_globs=("target",),
    )
    control = project / "worktrees"
    slots = control / "slots"

    assert (control / "ACTIVE.testhost.json").is_file()
    assert (control / "ARCHIVED.testhost.json").is_file()
    assert (control / "wrkslots").is_symlink()
    assert not (slots / "ACTIVE.testhost.json").exists()
    assert not (project / ".wrkslots.yml.lock").exists()
    donor_cache = repository / "target" / "release"
    donor_cache.mkdir(parents=True)
    (donor_cache / "artifact").write_bytes(b"donor cache must not be copied")

    made = create(project)

    assert made.returncode == 0, made.stderr
    tree = checkout(project)
    assert tree == slots / "slot01"
    assert (tree / "seed.txt").is_file()
    assert not (tree / "product").exists()
    assert not (tree / "target").exists()
    copied_config = tree / ".wrkslots.yml"
    copied_config.write_bytes((project / ".wrkslots.yml").read_bytes())
    discovered = subprocess.run(
        [sys.executable, str(WRKSLOTS), "status", "--slot", "slot01"],
        cwd=tree,
        text=True,
        capture_output=True,
        check=False,
    )
    assert discovered.returncode == 0, discovered.stderr
    assert f"project={project}" in discovered.stdout
    copied_config.unlink()
    row = active_slots(project)[0]
    assert isinstance(row, dict)
    assert row["checkouts"][0]["path"] == "worktrees/slots/slot01"

    task_head = commit_task(repository, tree, "codex/task")
    cache = tree / "target"
    cache.mkdir()
    (cache / "artifact").write_bytes(b"regenerable cache")
    handed_off = finish(project)
    assert handed_off.returncode == 0, handed_off.stderr
    mark_owner_dead(project)
    set_liveness(project, "dead")
    monkeypatch.setattr(
        wrkslots, "_assert_slot_unused", lambda *_args, **_kwargs: None
    )
    removed_caches: list[tuple[Path, bool]] = []
    original_remove_cache_directory = wrkslots._remove_cache_directory

    def record_cache_removal(
        config: wrkslots.Config,
        cache_directory: wrkslots.CacheDirectory,
        *,
        allow_git_metadata: bool = False,
    ) -> int:
        removed_caches.append((cache_directory.path, allow_git_metadata))
        return original_remove_cache_directory(
            config,
            cache_directory,
            allow_git_metadata=allow_git_metadata,
        )

    monkeypatch.setattr(wrkslots, "_remove_cache_directory", record_cache_removal)

    remove_code = wrkslots.main(
        [
            "--project-root",
            str(project),
            "remove",
            "slot01",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--expected-generation",
            "1",
        ]
    )
    captured = capsys.readouterr()

    assert remove_code == 0, captured.err
    assert len(removed_caches) == 1
    removed_cache_path, allow_git_metadata = removed_caches[0]
    assert removed_cache_path.name == "target"
    assert any(part.startswith(".slot01.fenced.") for part in removed_cache_path.parts)
    assert allow_git_metadata is False
    assert not tree.exists()
    assert slots.is_dir()
    archive = json.loads(
        (control / "ARCHIVED.testhost.json").read_text(encoding="utf-8")
    )
    archived = archive["records"][0]
    assert archived["purpose"] == "test slot01"
    assert archived["finished_at"]
    assert archived["checkouts"][0]["path"] == "worktrees/slots/slot01"
    assert archived["checkouts"][0]["branch"] == "codex/task"
    assert archived["checkouts"][0]["head"] == task_head


def test_flat_layout_refuses_more_than_one_repo_before_mutation(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
    )
    remote_url = git(repository, "remote", "get-url", "origin").stdout.strip()

    refused = command(
        project,
        "create",
        "slot01",
        "--agent",
        "codex-1",
        "--task",
        "task-flat",
        "--purpose",
        "flat rejects multiple repositories",
        "--owner-pid",
        str(os.getpid()),
        "--coordinator-pid",
        str(os.getpid()),
        "--repo",
        "first=repo",
        "--remote-url",
        f"first={remote_url}",
        "--branch",
        "first=codex/first",
        "--repo",
        "second=repo",
        "--remote-url",
        f"second={remote_url}",
        "--branch",
        "second=codex/second",
    )

    assert refused.returncode == 3
    assert "flat" in refused.stderr
    assert "exactly one" in refused.stderr
    assert active_slots(project) == []
    assert not checkout(project).exists()
    assert not (control_directory(project) / "ACTIVE.testhost.journal").exists()


def test_post_provision_hooks_run_in_order_before_registration(
    tmp_path: Path,
) -> None:
    project_path = tmp_path / "project"
    active_path = project_path / "worktrees" / "ACTIVE.testhost.json"
    first = python_hook(
        "import json; from pathlib import Path; "
        "assert Path('seed.txt').is_file(); "
        f"assert json.loads(Path({str(active_path)!r}).read_text())['slots'] == []; "
        "Path('hook-order').write_text('first\\n')"
    )
    second = python_hook(
        "from pathlib import Path; p = Path('hook-order'); "
        "assert p.read_text() == 'first\\n'; "
        "p.write_text(p.read_text() + 'second\\n')"
    )
    project, repository, remote = make_project(
        tmp_path,
        post_provision_hooks=(first, second),
    )
    second_repository = project / "repo-two"
    subprocess.run(
        ["git", "clone", str(remote), str(second_repository)],
        check=True,
        capture_output=True,
        text=True,
    )
    remote_url = git(repository, "remote", "get-url", "origin").stdout.strip()

    made = command(
        project,
        "create",
        "slot01",
        "--agent",
        "codex-1",
        "--task",
        "task-hooks",
        "--purpose",
        "ordered hooks",
        "--owner-pid",
        str(os.getpid()),
        "--coordinator-pid",
        str(os.getpid()),
        "--repo",
        "product=repo",
        "--remote-url",
        f"product={remote_url}",
        "--branch",
        "product=codex/product-hooks",
        "--repo",
        "second=repo-two",
        "--remote-url",
        f"second={remote_url}",
        "--branch",
        "second=codex/second-hooks",
    )

    assert made.returncode == 0, made.stderr
    assert (checkout(project) / "hook-order").read_text(encoding="utf-8") == (
        "first\nsecond\n"
    )
    assert (checkout(project, name="second") / "hook-order").read_text(
        encoding="utf-8"
    ) == "first\nsecond\n"
    assert len(active_slots(project)) == 1
    row = active_slots(project)[0]
    assert isinstance(row, dict)
    assert len(row["checkouts"]) == 2


def test_create_json_keeps_hook_progress_off_machine_readable_stdout(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(
        tmp_path,
        post_provision_hooks=("printf 'hook-progress\\n'",),
    )

    made = raw_command(
        project,
        "create",
        "slot01",
        "--format",
        "json",
        "--slot-type",
        "validate",
        "--coordinator-authorized",
        "--agent",
        "validate-json",
        "--task",
        "task-json",
        "--purpose",
        "prove create JSON is parseable",
        "--owner-pid",
        str(os.getpid()),
        "--coordinator-pid",
        str(os.getpid()),
        "--repo",
        "product=repo",
        "--start",
        "product=HEAD",
    )

    assert made.returncode == 0, made.stderr
    payload = json.loads(made.stdout)
    assert payload["slot"] == "slot01"
    assert payload["slot_type"] == "validate"
    assert payload["checkouts"][0]["path"] == str(
        checkout(project, slot_type="validate")
    )
    assert "hook-progress" in made.stderr


def test_failed_post_provision_hook_is_loud_and_recovery_resumes_hooks(
    tmp_path: Path,
) -> None:
    project_path = tmp_path / "project"
    attempts = project_path / "hook-attempts"
    allow = project_path / "allow-hook"
    first = python_hook(
        "from pathlib import Path; "
        f"p = Path({str(attempts)!r}); "
        "p.write_text(p.read_text() + 'first\\n' if p.exists() else 'first\\n')"
    )
    failing = python_hook(
        "import sys; from pathlib import Path; "
        f"p = Path({str(attempts)!r}); "
        "p.write_text(p.read_text() + 'second\\n'); "
        "Path('target').mkdir(exist_ok=True); "
        "Path('target/generated').write_text('cache'); "
        "print('hook stdout evidence'); "
        "print('hook stderr evidence', file=sys.stderr); "
        f"raise SystemExit(0 if Path({str(allow)!r}).exists() else 17)"
    )
    project, repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
        cache_globs=("target",),
        post_provision_hooks=(first, failing),
    )

    refused = create(project)

    assert refused.returncode == 3
    output = refused.stdout + refused.stderr
    assert "hook stdout evidence" in output
    assert "hook stderr evidence" in output
    assert "17" in output
    assert "recover" in output.lower()
    assert checkout(project).is_dir()
    assert active_slots(project) == []
    journal = create_journal_path(project)
    assert journal.is_file()
    assert attempts.read_text(encoding="utf-8") == "first\nsecond\n"
    audit = command(project, "audit", "--format", "json")
    assert audit.returncode == 0, audit.stderr
    audit_row = json.loads(audit.stdout)["slots"][0]
    assert audit_row["slot"] == "slot01"
    assert audit_row["owner_state"] == "create-journal"
    assert "wrkslots recover" in audit_row["reasons"][0]
    git(checkout(project), "checkout", "-b", "scratch/cache-cleanup")
    reclaimed = command(project, "clean-caches", "--only", "slot01")
    assert reclaimed.returncode == 0, reclaimed.stderr
    assert not (checkout(project) / "target").exists()
    assert journal.is_file()
    git(checkout(project), "checkout", "codex/task")

    allow.write_text("retry may proceed\n", encoding="utf-8")
    interrupted_state = json.loads(journal.read_text(encoding="utf-8"))
    legacy = control_directory(project) / "ACTIVE.testhost.journal"
    os.replace(journal, legacy)
    journal = legacy
    shutil.rmtree(control_directory(project) / "EVENTS.testhost")
    interrupted_state["hook_progress"] = 2
    journal.write_text(
        json.dumps(interrupted_state, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    inconsistent = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
    )
    assert inconsistent.returncode == 3
    assert "hook" in inconsistent.stderr.lower()
    assert active_slots(project) == []

    interrupted_state["hook_progress"] = 1
    interrupted_state["hook_failure"]["status"] = "running"
    del interrupted_state["hook_failure"]["returncode"]
    journal.write_text(
        json.dumps(interrupted_state, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    ambiguous = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
    )
    assert ambiguous.returncode == 3
    assert "--retry-running-hook" in ambiguous.stderr
    assert attempts.read_text(encoding="utf-8") == "first\nsecond\n"

    recovered = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--retry-running-hook",
    )

    assert recovered.returncode == 0, recovered.stderr
    assert len(active_slots(project)) == 1
    assert not journal.exists()
    assert attempts.read_text(encoding="utf-8") == "first\nsecond\nsecond\n"


def test_failed_post_provision_hook_can_be_explicitly_aborted(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
        post_provision_hooks=("exit 9",),
    )
    refused = create(project)
    assert refused.returncode == 3
    journal = create_journal_path(project)
    assert journal.is_file()
    assert checkout(project).is_dir()

    aborted = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--abort-create",
    )

    assert aborted.returncode == 0, aborted.stderr
    assert active_slots(project) == []
    assert not checkout(project).exists()
    assert not journal.exists()
    assert git(
        repository,
        "show-ref",
        "--verify",
        "--quiet",
        "refs/heads/codex/task",
        check=False,
    ).returncode == 1


def test_audit_associates_recovery_journals_from_other_machine_shards(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(
        tmp_path,
        post_provision_hooks=("exit 9",),
    )
    refused = create(project, machine="otherhost")
    assert refused.returncode == 3

    audit = command(project, "audit", "--format", "json")

    assert audit.returncode == 0, audit.stderr
    row = json.loads(audit.stdout)["slots"][0]
    assert row["slot"] == "slot01"
    assert row["machine"] == "otherhost"
    assert row["owner_state"] == "create-journal"
    assert "wrkslots recover" in row["reasons"][0]

    git(repository, "worktree", "remove", "--force", str(checkout(project)))
    without_directory = command(project, "audit", "--format", "json")
    assert without_directory.returncode == 0, without_directory.stderr
    missing_row = json.loads(without_directory.stdout)["slots"][0]
    assert missing_row["slot"] == "slot01"
    assert missing_row["owner_state"] == "create-journal"


def test_audit_blocks_exact_validation_batch_seal_target(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    prepare_dead_validate_slots(project, ("slot01",))
    config = wrkslots._load_config(str(project), "testhost")
    monkeypatch.setattr(
        wrkslots,
        "_capture_process_path_census",
        lambda _paths: wrkslots._ProcessPathCensus((), ()),
    )
    interrupted = raw_command(
        project,
        "remove-validate-batch",
        "--coordinator-pid",
        str(os.getpid()),
        "--slot",
        "slot01=1",
        env={"WRKSLOTS_TEST_INTERRUPT": "after-validate-batch-seal-target"},
    )
    assert interrupted.returncode == 86, interrupted.stderr
    active_before = wrkslots._active_path(config).read_bytes()
    seal_path = wrkslots._validate_batch_seal_journal_path(config)
    seal_before = seal_path.read_bytes()
    assert (
        wrkslots.main(
            ["--project-root", str(project), "audit", "--gate", "--format", "json"]
        )
        == 1
    )
    payload = json.loads(capsys.readouterr().out)
    row = next(row for row in payload["slots"] if row["slot"] == "slot01")
    assert row["verdict"] == "BLOCKED"
    assert "validation-batch seal" in row["reasons"][0]
    assert wrkslots._active_path(config).read_bytes() == active_before
    assert seal_path.read_bytes() == seal_before


def test_audit_reports_malformed_validation_batch_seal_as_unknown(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    config = wrkslots._load_config(str(project), "testhost")
    seal_path = wrkslots._validate_batch_seal_journal_path(config)
    seal_path.write_text("{not-json\n", encoding="utf-8")
    before = seal_path.read_bytes()

    assert (
        wrkslots.main(
            ["--project-root", str(project), "audit", "--gate", "--format", "json"]
        )
        == 2
    )
    payload = json.loads(capsys.readouterr().out)
    row = next(row for row in payload["slots"] if row["slot"] == seal_path.name)
    assert row["verdict"] == "UNKNOWN"
    assert "seal evidence is unreadable" in row["reasons"][0]
    assert seal_path.read_bytes() == before


def test_audit_reports_unattributable_validation_batch_seal_as_unknown(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    prepare_dead_validate_slots(project, ("slot01",))
    config = wrkslots._load_config(str(project), "testhost")
    monkeypatch.setattr(
        wrkslots,
        "_capture_process_path_census",
        lambda _paths: wrkslots._ProcessPathCensus((), ()),
    )
    interrupted = raw_command(
        project,
        "remove-validate-batch",
        "--coordinator-pid",
        str(os.getpid()),
        "--slot",
        "slot01=1",
        env={"WRKSLOTS_TEST_INTERRUPT": "after-validate-batch-seal-target"},
    )
    assert interrupted.returncode == 86, interrupted.stderr
    seal_path = wrkslots._validate_batch_seal_journal_path(config)
    seal = json.loads(seal_path.read_text(encoding="utf-8"))
    seal["targets"][0]["generation"] = 2
    seal_path.write_text(json.dumps(seal, indent=2) + "\n", encoding="utf-8")
    before = seal_path.read_bytes()
    assert (
        wrkslots.main(
            ["--project-root", str(project), "audit", "--gate", "--format", "json"]
        )
        == 2
    )
    payload = json.loads(capsys.readouterr().out)
    row = next(row for row in payload["slots"] if row["slot"] == "slot01")
    assert row["verdict"] == "UNKNOWN"
    assert "cannot be attributed" in row["reasons"][0]
    assert seal_path.read_bytes() == before


def test_abort_create_preflights_every_checkout_before_removing_any(
    tmp_path: Path,
) -> None:
    project, repository, remote = make_project(
        tmp_path,
        post_provision_hooks=("exit 9",),
    )
    second_repository = project / "repo-two"
    subprocess.run(
        ["git", "clone", str(remote), str(second_repository)],
        check=True,
        capture_output=True,
        text=True,
    )
    remote_url = git(repository, "remote", "get-url", "origin").stdout.strip()
    made = command(
        project,
        "create",
        "slot01",
        "--agent",
        "codex-1",
        "--task",
        "task-abort",
        "--purpose",
        "abort preflight",
        "--owner-pid",
        str(os.getpid()),
        "--coordinator-pid",
        str(os.getpid()),
        "--repo",
        "first=repo",
        "--remote-url",
        f"first={remote_url}",
        "--branch",
        "first=codex/first-abort",
        "--repo",
        "second=repo-two",
        "--remote-url",
        f"second={remote_url}",
        "--branch",
        "second=codex/second-abort",
    )
    assert made.returncode == 3
    first = checkout(project, name="first")
    second = checkout(project, name="second")
    evidence = second / "hook-notes.txt"
    evidence.write_text("preserve me\n", encoding="utf-8")

    aborted = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--abort-create",
    )

    assert aborted.returncode == 3
    assert "source state is dirty" in aborted.stderr
    assert first.is_dir()
    assert second.is_dir()
    assert evidence.read_text(encoding="utf-8") == "preserve me\n"


def test_abort_create_refuses_and_preserves_untracked_source_from_failed_hook(
    tmp_path: Path,
) -> None:
    failing = python_hook(
        "from pathlib import Path; "
        "Path('important-source.txt').write_text('preserve me\\n'); "
        "raise SystemExit(9)"
    )
    project, repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
        post_provision_hooks=(failing,),
    )
    refused = create(project)
    assert refused.returncode == 3
    tree = checkout(project)
    source = tree / "important-source.txt"
    journal = create_journal_path(project)
    assert source.read_text(encoding="utf-8") == "preserve me\n"

    aborted = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--abort-create",
    )

    assert aborted.returncode == 3
    assert "source state is dirty" in aborted.stderr
    assert source.read_text(encoding="utf-8") == "preserve me\n"
    assert tree.is_dir()
    assert journal.is_file()
    assert git(
        repository,
        "show-ref",
        "--verify",
        "--quiet",
        "refs/heads/codex/task",
        check=False,
    ).returncode == 0


def test_disk_ladder_warns_refuses_allows_override_and_keeps_emergency_hard(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    gib = 1024**3
    project, repository, _remote = make_project(
        tmp_path,
        disk_thresholds_gib=(250, 200, 100),
    )
    config = configuration(project)
    assert config["disk_advisory_bytes"] == 250 * gib
    assert config["disk_provisioning_floor_bytes"] == 200 * gib
    assert config["disk_emergency_bytes"] == 100 * gib
    remote_url = git(repository, "remote", "get-url", "origin").stdout.strip()

    def run_create(
        slot: str, agent: str, branch: str, *, override: bool = False
    ) -> tuple[int, str]:
        argv = [
            "--project-root",
            str(project),
            "create",
            slot,
            "--slot-type",
            "agent",
            "--coordinator-authorized",
            "--agent",
            agent,
            "--task",
            f"task-{slot}",
            "--purpose",
            f"disk policy {slot}",
            "--owner-pid",
            str(os.getpid()),
            "--coordinator-pid",
            str(os.getpid()),
            "--repo",
            "product=repo",
            "--remote-url",
            f"product={remote_url}",
            "--branch",
            f"product={branch}",
        ]
        if override:
            argv.append("--override-disk-floor")
        code = wrkslots.main(argv)
        captured = capsys.readouterr()
        return code, captured.out + captured.err

    monkeypatch.setattr(wrkslots, "_free_bytes", lambda _path: 225 * gib)
    advisory_code, advisory_output = run_create(
        "slot01", "codex-1", "codex/advisory"
    )

    assert advisory_code == 0, advisory_output
    assert "advisory" in advisory_output.lower()

    monkeypatch.setattr(wrkslots, "_free_bytes", lambda _path: 150 * gib)
    floor_code, floor_output = run_create(
        "slot02", "codex-2", "codex/floor"
    )

    assert floor_code == 3
    assert "provision" in floor_output.lower()
    assert "audit" in floor_output.lower()
    assert "clean-caches" in floor_output.lower()
    assert not checkout(project, "slot02").exists()

    override_code, override_output = run_create(
        "slot02", "codex-2", "codex/floor", override=True
    )
    assert override_code == 0, override_output

    monkeypatch.setattr(wrkslots, "_free_bytes", lambda _path: 50 * gib)
    emergency_code, emergency_output = run_create(
        "slot03", "codex-3", "codex/emergency", override=True
    )

    assert emergency_code == 3
    assert "emergency" in emergency_output.lower()
    assert not checkout(project, "slot03").exists()


def test_clean_caches_reports_then_cleans_only_explicit_or_all_slots(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(
        tmp_path,
        cache_globs=("target", "node_modules"),
    )
    assert create(project, slot="slot01", agent="codex-1", branch="codex/one").returncode == 0
    assert create(project, slot="slot02", agent="codex-2", branch="codex/two").returncode == 0
    first = checkout(project, "slot01")
    second = checkout(project, "slot02")
    for tree, payload in ((first, b"a" * 1024), (second, b"b" * 2048)):
        (tree / "target" / "debug").mkdir(parents=True)
        (tree / "target" / "debug" / "artifact").write_bytes(payload)
        (tree / "node_modules" / "package").mkdir(parents=True)
        (tree / "node_modules" / "package" / "index.js").write_text(
            "generated\n", encoding="utf-8"
        )
    source = first / "uncommitted-source.txt"
    source.write_text("must survive cache cleanup\n", encoding="utf-8")

    report = command(project, "clean-caches", "--format", "json")

    assert report.returncode == 0, report.stderr
    report_rows = {row["slot"]: row for row in json.loads(report.stdout)["slots"]}
    assert report_rows["slot01"]["action"] == "REPORT"
    assert report_rows["slot02"]["action"] == "REPORT"
    assert report_rows["slot01"]["cache_bytes"] >= 1024
    assert report_rows["slot02"]["cache_bytes"] >= 2048
    assert (first / "target").is_dir()
    assert (second / "target").is_dir()

    explicit = command(
        project,
        "clean-caches",
        "--only",
        "slot01",
        "--format",
        "json",
    )

    assert explicit.returncode == 0, explicit.stderr
    explicit_rows = {
        row["slot"]: row for row in json.loads(explicit.stdout)["slots"]
    }
    assert explicit_rows["slot01"]["action"] == "REMOVED"
    assert explicit_rows["slot02"]["action"] == "REPORT"
    assert not (first / "target").exists()
    assert not (first / "node_modules").exists()
    assert (second / "target").is_dir()
    assert source.read_text(encoding="utf-8") == "must survive cache cleanup\n"

    sweep = command(project, "clean-caches", "--yes", "--format", "json")

    assert sweep.returncode == 0, sweep.stderr
    sweep_rows = {row["slot"]: row for row in json.loads(sweep.stdout)["slots"]}
    assert sweep_rows["slot02"]["action"] == "REMOVED"
    assert not (second / "target").exists()
    assert not (second / "node_modules").exists()

    empty_report = command(project, "clean-caches", "--format", "json")
    assert empty_report.returncode == 0, empty_report.stderr
    empty_rows = {
        row["slot"]: row for row in json.loads(empty_report.stdout)["slots"]
    }
    assert empty_rows["slot01"]["cache_bytes"] == 0
    assert empty_rows["slot02"]["cache_bytes"] == 0


def test_agent_removal_and_clean_caches_refuse_nested_git_metadata(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(tmp_path, cache_globs=("target",))
    assert create(project).returncode == 0
    tree = checkout(project)
    commit_task(repository, tree, "codex/task")
    nested_git = tree / "target" / "install-build" / "dependency" / ".git"
    nested_git.mkdir(parents=True)
    sentinel = nested_git / "HEAD"
    sentinel.write_text("ref: refs/heads/main\n", encoding="utf-8")

    cleaned = command(project, "clean-caches", "--only", "slot01")

    assert cleaned.returncode == 3
    assert "nested Git metadata" in cleaned.stderr
    assert sentinel.is_file()

    handed_off = finish(project)
    assert handed_off.returncode == 0, handed_off.stderr
    mark_owner_dead(project)
    set_liveness(project, "dead")

    removed = remove(project)

    assert removed.returncode == 3
    assert "nested Git metadata" in removed.stderr
    assert sentinel.is_file()
    assert tree.is_dir()
    assert active_slots(project)


def test_repository_specific_cache_globs_do_not_apply_to_sibling_checkouts(
    tmp_path: Path,
) -> None:
    project, repository, remote = make_project(
        tmp_path,
        repo_cache_globs=(("first", "target"),),
    )
    second_repository = project / "repo-two"
    subprocess.run(
        ["git", "clone", str(remote), str(second_repository)],
        check=True,
        capture_output=True,
        text=True,
    )
    git(second_repository, "config", "user.name", "Wrkslots Test")
    git(second_repository, "config", "user.email", "wrkslots@example.invalid")
    (second_repository / "target").mkdir()
    (second_repository / "target" / "tracked.txt").write_text(
        "source, not cache\n", encoding="utf-8"
    )
    git(second_repository, "add", "target/tracked.txt")
    git(second_repository, "commit", "-m", "track target source")
    second_start = git(second_repository, "rev-parse", "HEAD").stdout.strip()
    remote_url = git(repository, "remote", "get-url", "origin").stdout.strip()
    made = command(
        project,
        "create",
        "slot01",
        "--agent",
        "codex-1",
        "--task",
        "task-cache-policy",
        "--purpose",
        "repository-specific caches",
        "--owner-pid",
        str(os.getpid()),
        "--coordinator-pid",
        str(os.getpid()),
        "--repo",
        "first=repo",
        "--remote-url",
        f"first={remote_url}",
        "--branch",
        "first=codex/first-cache",
        "--repo",
        "second=repo-two",
        "--remote-url",
        f"second={remote_url}",
        "--branch",
        "second=codex/second-cache",
        "--start",
        f"second={second_start}",
    )
    assert made.returncode == 0, made.stderr
    first = checkout(project, name="first")
    second = checkout(project, name="second")
    (first / "target").mkdir()
    (first / "target" / "artifact").write_bytes(b"cache")

    cleaned = command(project, "clean-caches", "--only", "slot01")

    assert cleaned.returncode == 0, cleaned.stderr
    assert not (first / "target").exists()
    assert (second / "target" / "tracked.txt").read_text(encoding="utf-8") == (
        "source, not cache\n"
    )


def test_clean_caches_can_reclaim_an_explicit_unregistered_flat_slot(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(
        tmp_path,
        worktrees_directory="worktrees/slots",
        layout="flat",
        cache_globs=("target",),
    )
    tree = checkout(project)
    tree.parent.mkdir(exist_ok=True)
    git(repository, "worktree", "add", "-b", "codex/leaked", str(tree), "origin/main")
    artifact = tree / "target" / "debug" / "artifact"
    artifact.parent.mkdir(parents=True)
    artifact.write_bytes(b"regenerable")

    report = command(project, "clean-caches", "--format", "json")
    assert report.returncode == 0, report.stderr
    row = json.loads(report.stdout)["slots"][0]
    assert row["slot"] == "slot01"
    assert row["registered"] is False
    assert row["action"] == "REPORT"
    assert artifact.is_file()

    cleaned = command(project, "clean-caches", "--only", "slot01")
    assert cleaned.returncode == 0, cleaned.stderr
    assert not (tree / "target").exists()
    assert (tree / "seed.txt").is_file()


def test_clean_caches_reports_and_reclaims_across_machine_shards(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path, cache_globs=("target",))
    assert create(project, slot="slot01", agent="codex-1", branch="codex/one").returncode == 0
    assert create(
        project,
        slot="slot02",
        agent="codex-2",
        branch="codex/two",
        machine="otherhost",
    ).returncode == 0
    for slot in ("slot01", "slot02"):
        artifact = checkout(project, slot) / "target" / "artifact"
        artifact.parent.mkdir()
        artifact.write_bytes(slot.encode())

    cleaned = command(project, "clean-caches", "--yes", "--format", "json")

    assert cleaned.returncode == 0, cleaned.stderr
    rows = {row["slot"]: row for row in json.loads(cleaned.stdout)["slots"]}
    assert set(rows) == {"slot01", "slot02"}
    assert rows["slot01"]["machine"] == "testhost"
    assert rows["slot02"]["machine"] == "otherhost"
    assert not (checkout(project, "slot01") / "target").exists()
    assert not (checkout(project, "slot02") / "target").exists()


def test_clean_caches_only_isolated_from_an_unselected_malformed_leak(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path, cache_globs=("target",))
    assert create(project).returncode == 0
    tree = checkout(project)
    artifact = tree / "target" / "artifact"
    artifact.parent.mkdir()
    artifact.write_bytes(b"cache")
    junk = project / "worktrees" / "junk"
    junk.mkdir()
    (junk / "note").write_text("not a checkout\n", encoding="utf-8")
    invalid_name = project / "worktrees" / "bad slot"
    invalid_name.mkdir()
    (invalid_name / "note").write_text("also not a checkout\n", encoding="utf-8")

    cleaned = command(project, "clean-caches", "--only", "slot01", "--format", "json")

    assert cleaned.returncode == 0, cleaned.stderr
    rows = {row["slot"]: row for row in json.loads(cleaned.stdout)["slots"]}
    assert rows["slot01"]["action"] == "REMOVED"
    assert rows["junk"]["action"] == "BLOCKED"
    assert rows["junk"]["cache_error"]
    assert rows["bad slot"]["action"] == "BLOCKED"
    assert not (tree / "target").exists()
    assert (junk / "note").is_file()

    artifact.parent.mkdir()
    artifact.write_bytes(b"cache again")
    bulk = command(project, "clean-caches", "--yes")
    assert bulk.returncode == 3
    assert "malformed unregistered directory" in bulk.stderr
    assert artifact.is_file()


def test_init_rejects_malformed_recursive_cache_glob_cleanly(tmp_path: Path) -> None:
    project = tmp_path / "project"
    project.mkdir()
    liveness = project / "liveness.sh"
    liveness.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
    liveness.chmod(0o755)

    refused = subprocess.run(
        [
            sys.executable,
            str(WRKSLOTS),
            "--machine",
            "testhost",
            "init",
            str(project),
            "--liveness-command",
            "liveness.sh",
            "--cache-glob",
            "foo/***",
        ],
        text=True,
        capture_output=True,
        check=False,
    )

    assert refused.returncode == 3
    assert "recursive wildcard" in refused.stderr.lower()
    assert "traceback" not in refused.stderr.lower()
    assert not (project / ".wrkslots.yml").exists()


def test_cache_glob_with_redundant_relative_separators_is_canonicalized(
    tmp_path: Path,
) -> None:
    project = tmp_path / "project"
    project.mkdir()

    initialize(project, cache_globs=(".//target",))

    config = json.loads((project / ".wrkslots.yml").read_text(encoding="utf-8"))
    assert config["cache_globs"] == ["target"]


def test_init_rejects_nonpositive_disk_threshold_before_writing_config(
    tmp_path: Path,
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    liveness = project / "liveness.sh"
    liveness.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
    liveness.chmod(0o755)

    refused = subprocess.run(
        [
            sys.executable,
            str(WRKSLOTS),
            "--machine",
            "testhost",
            "init",
            str(project),
            "--liveness-command",
            "liveness.sh",
            "--disk-advisory-gib",
            "1",
            "--disk-provisioning-floor-gib",
            "0",
            "--disk-emergency-gib",
            "-1",
        ],
        text=True,
        capture_output=True,
        check=False,
    )

    assert refused.returncode == 2
    assert "--disk-provisioning-floor-gib must be positive" in refused.stderr
    assert not (project / ".wrkslots.yml").exists()


def test_clean_caches_refuses_intermediate_symlink_escape(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(
        tmp_path,
        cache_globs=("cache-link/data",),
    )
    assert create(project).returncode == 0
    tree = checkout(project)
    outside = tmp_path / "outside-cache"
    data = outside / "data"
    data.mkdir(parents=True)
    artifact = data / "artifact"
    artifact.write_text("must survive\n", encoding="utf-8")
    (tree / "cache-link").symlink_to(outside, target_is_directory=True)

    refused = command(project, "clean-caches", "--only", "slot01")

    assert refused.returncode == 3
    assert "symlink" in refused.stderr.lower()
    assert artifact.read_text(encoding="utf-8") == "must survive\n"


def test_clean_caches_refuses_checkout_replacement_after_planning(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, _repository, _remote = make_project(
        tmp_path,
        cache_globs=("target",),
    )
    assert create(project).returncode == 0
    tree = checkout(project)
    original_cache = tree / "target"
    original_cache.mkdir()
    (original_cache / "artifact").write_text("old cache\n", encoding="utf-8")
    displaced = tree.with_name("product-displaced")
    replacement_artifact = tree / "target" / "must-survive"
    original_measure = wrkslots._allocated_cache_bytes
    swapped = False

    def replace_checkout_after_planning(
        config: wrkslots.Config, cache: wrkslots.CacheDirectory
    ) -> int:
        nonlocal swapped
        size = original_measure(config, cache)
        if not swapped:
            swapped = True
            tree.rename(displaced)
            tree.mkdir()
            replacement_artifact.parent.mkdir()
            replacement_artifact.write_text("replacement data\n", encoding="utf-8")
        return size

    monkeypatch.setattr(
        wrkslots, "_allocated_cache_bytes", replace_checkout_after_planning
    )

    returncode = wrkslots.main(
        ["--project-root", str(project), "clean-caches", "--only", "slot01"]
    )
    captured = capsys.readouterr()

    assert returncode == 3
    assert swapped
    assert "checkout identity changed" in captured.err
    assert replacement_artifact.read_text(encoding="utf-8") == "replacement data\n"
    assert (displaced / "target" / "artifact").is_file()


def test_cache_policy_refuses_tracked_source_overlap(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path, cache_globs=("src",))
    assert create(project).returncode == 0
    tree = checkout(project)
    (tree / "src").mkdir()
    commit_local(tree, "src/tracked.txt", "tracked\n", "track source")
    tracked = tree / "src" / "tracked.txt"
    git(tree, "rm", "--cached", "src/tracked.txt")
    tracked.write_text("uncommitted source\n", encoding="utf-8")

    cleaned = command(project, "clean-caches", "--only", "slot01")
    handed_off = finish(project)

    assert cleaned.returncode == 3
    assert "tracked source" in cleaned.stderr.lower()
    assert handed_off.returncode == 3
    assert "tracked source" in handed_off.stderr.lower()
    assert tracked.read_text(encoding="utf-8") == "uncommitted source\n"


def test_cache_policy_refuses_paths_inside_git_submodules(tmp_path: Path) -> None:
    project, repository, _remote = make_project(
        tmp_path,
        cache_globs=("deps/src",),
    )
    submodule_remote = tmp_path / "dependency.git"
    submodule_checkout = tmp_path / "dependency"
    subprocess.run(
        ["git", "init", "--bare", "--initial-branch=main", str(submodule_remote)],
        check=True,
        capture_output=True,
        text=True,
    )
    subprocess.run(
        ["git", "clone", str(submodule_remote), str(submodule_checkout)],
        check=True,
        capture_output=True,
        text=True,
    )
    git(submodule_checkout, "config", "user.name", "Wrkslots Test")
    git(submodule_checkout, "config", "user.email", "wrkslots@example.invalid")
    (submodule_checkout / "src").mkdir()
    (submodule_checkout / "src" / "tracked.txt").write_text(
        "tracked dependency source\n", encoding="utf-8"
    )
    git(submodule_checkout, "add", "src/tracked.txt")
    git(submodule_checkout, "commit", "-m", "dependency seed")
    git(submodule_checkout, "push", "-u", "origin", "main")
    git(
        repository,
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        str(submodule_remote),
        "deps",
    )
    git(repository, "commit", "-m", "add dependency")
    git(repository, "push", "origin", "main")
    assert create(project).returncode == 0
    tree = checkout(project)
    git(
        tree,
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "update",
        "--init",
    )
    dependency_source = tree / "deps" / "src" / "tracked.txt"
    git(tree / "deps", "rm", "--cached", "src/tracked.txt")
    git(tree, "rm", "--cached", "deps")

    cleaned = command(project, "clean-caches", "--only", "slot01")
    handed_off = finish(project)

    assert cleaned.returncode == 3
    assert "submodule" in cleaned.stderr.lower()
    assert handed_off.returncode == 3
    assert "submodule" in handed_off.stderr.lower()
    assert dependency_source.read_text(encoding="utf-8") == "tracked dependency source\n"


def test_hold_blocks_cache_cleanup_and_removal_until_released(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(
        tmp_path,
        cache_globs=("target",),
    )
    assert create(project).returncode == 0
    tree = checkout(project)
    commit_task(repository, tree, "codex/task")
    cache = tree / "target" / "debug"
    cache.mkdir(parents=True)
    (cache / "artifact").write_bytes(b"cache")
    held = command(project, "hold", "slot01", "--reason", "active roster")
    assert held.returncode == 0, held.stderr

    handed_off = finish(project)

    assert handed_off.returncode == 0, handed_off.stderr
    mark_owner_dead(project)
    set_liveness(project, "dead")

    named_clean = command(
        project,
        "clean-caches",
        "--only",
        "slot01",
        "--format",
        "json",
    )
    assert named_clean.returncode == 0, named_clean.stderr
    row = json.loads(named_clean.stdout)["slots"][0]
    assert row["slot"] == "slot01"
    assert row["action"] == "HELD"
    assert cache.is_dir()

    all_clean = command(project, "clean-caches", "--yes")
    assert all_clean.returncode == 0, all_clean.stderr
    assert cache.is_dir()
    refused = remove(project)
    assert refused.returncode == 3
    assert "hold" in refused.stderr.lower()
    assert tree.is_dir()

    released = command(project, "unhold", "slot01")
    assert released.returncode == 0, released.stderr
    assert cache.is_dir()
    monkeypatch.setattr(
        wrkslots, "_assert_slot_unused", lambda *_args, **_kwargs: None
    )
    remove_code = wrkslots.main(
        [
            "--project-root",
            str(project),
            "remove",
            "slot01",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--expected-generation",
            "1",
        ]
    )
    captured = capsys.readouterr()
    assert remove_code == 0, captured.err
    assert not tree.exists()


def test_audit_reports_deletable_blocked_held_and_the_leak_invariant(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    assert create(project, slot="slot01", agent="codex-1", branch="codex/one").returncode == 0
    commit_task(repository, checkout(project, "slot01"), "codex/one")
    assert finish(project, slot="slot01", agent="codex-1").returncode == 0
    assert create(project, slot="slot02", agent="codex-2", branch="codex/two").returncode == 0
    assert create(project, slot="slot03", agent="codex-3", branch="codex/three").returncode == 0
    held = command(project, "hold", "slot03", "--reason", "keep for inspection")
    assert held.returncode == 0, held.stderr
    mark_owner_dead(project)
    set_liveness(project, "dead")
    state_before = (control_directory(project) / "ACTIVE.testhost.json").read_bytes()
    monkeypatch.setattr(
        wrkslots, "_assert_slot_unused", lambda *_args, **_kwargs: None
    )
    monkeypatch.setattr(
        wrkslots,
        "_capture_process_path_census",
        lambda _paths: wrkslots._ProcessPathCensus((), ()),
    )

    audit_code = wrkslots.main(
        ["--project-root", str(project), "audit", "--format", "json"]
    )
    captured = capsys.readouterr()

    assert audit_code == 0, captured.err
    payload = json.loads(captured.out)
    assert payload["worktree_count"] == 3
    assert payload["running_agent_count"] == 2
    assert payload["leak"] is True
    rows = {row["slot"]: row for row in payload["slots"]}
    assert rows["slot01"]["verdict"] == "DELETABLE"
    assert rows["slot02"]["verdict"] == "BLOCKED"
    assert rows["slot02"]["reasons"]
    assert rows["slot03"]["verdict"] == "HELD"
    assert (control_directory(project) / "ACTIVE.testhost.json").read_bytes() == state_before

    gate_code = wrkslots.main(["--project-root", str(project), "audit", "--gate"])
    captured = capsys.readouterr()
    assert gate_code == 1
    assert "state=actionable" in captured.out
    assert "summary=1 worktree slot(s) need coordinator attention" in captured.out
    assert "summary=1 worktree slot(s) need coordinator attention:" not in captured.out
    assert "ACTION:" in captured.out

    def scan_failed(*_args: object, **_kwargs: object) -> None:
        raise wrkslots.Refusal("occupancy scan failed")

    monkeypatch.setattr(wrkslots, "_assert_slot_unused", scan_failed)
    uncertain_code = wrkslots.main(
        ["--project-root", str(project), "audit", "--format", "json"]
    )
    captured = capsys.readouterr()
    assert uncertain_code == 0, captured.err
    uncertain = json.loads(captured.out)
    assert uncertain["running_agent_count"] == 3
    assert uncertain["leak"] is False
    uncertain_rows = {row["slot"]: row for row in uncertain["slots"]}
    assert uncertain_rows["slot01"]["verdict"] == "BLOCKED"
    assert any(
        "occupancy scan failed" in reason
        for reason in uncertain_rows["slot01"]["reasons"]
    )

    uncertain_gate_code = wrkslots.main(
        ["--project-root", str(project), "audit", "--gate"]
    )
    captured = capsys.readouterr()
    assert uncertain_gate_code == 2
    assert "state=unknown" in captured.out
    assert "could not determine reclaim state" in captured.out
    assert "do not remove an UNKNOWN slot" in captured.out

    monkeypatch.setattr(
        wrkslots, "_assert_slot_unused", lambda *_args, **_kwargs: None
    )
    set_liveness(project, "probe-crashed")
    failed_code = wrkslots.main(
        ["--project-root", str(project), "audit", "--format", "json"]
    )
    captured = capsys.readouterr()
    assert failed_code == 0, captured.err
    failed_rows = {
        row["slot"]: row for row in json.loads(captured.out)["slots"]
    }
    assert failed_rows["slot01"]["verdict"] == "BLOCKED"
    assert any(
        "liveness" in reason.lower() or "unexpected rc" in reason.lower()
        for reason in failed_rows["slot01"]["reasons"]
    )


def test_audit_treats_validate_checkout_contents_as_disposable(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project, slot_type="validate", branch=None)
    assert made.returncode == 0, made.stderr
    tree = checkout(project, slot_type="validate")
    (tree / "result.tmp").write_text("disposable result\n", encoding="utf-8")
    merge_head = Path(git(tree, "rev-parse", "--git-path", "MERGE_HEAD").stdout.strip())
    merge_head.write_text(git(tree, "rev-parse", "HEAD").stdout, encoding="utf-8")
    mark_owner_dead(project)
    set_liveness(project, "dead")
    expire_heartbeat(project)
    monkeypatch.setattr(
        wrkslots, "_assert_slot_unused", lambda *_args, **_kwargs: None
    )
    monkeypatch.setattr(
        wrkslots,
        "_capture_process_path_census",
        lambda _paths: wrkslots._ProcessPathCensus((), ()),
    )

    audit_code = wrkslots.main(
        ["--project-root", str(project), "audit", "--format", "json"]
    )
    captured = capsys.readouterr()

    assert audit_code == 0, captured.err
    payload = json.loads(captured.out)
    assert payload["attention_slots"] == ["slot01"]
    rows = {row["slot"]: row for row in payload["slots"]}
    assert rows["slot01"]["slot_type"] == "validate"
    assert rows["slot01"]["verdict"] == "DELETABLE"
    assert rows["slot01"]["reasons"] == []


def test_audit_uses_one_process_path_census_for_all_rows(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    for slot in ("unused", "in-use"):
        made = create(project, slot=slot, slot_type="validate", branch=None)
        assert made.returncode == 0, made.stderr
    config = wrkslots._load_config(str(project), "testhost")
    for slot in ("unused", "in-use"):
        state = wrkslots._load_active(config)
        record = next(item for item in state.slots if item.slot == slot)
        assert record.owner is not None
        wrkslots._write_active_state(
            config,
            wrkslots._replace_record(
                state,
                replace(
                    record,
                    owner=replace(record.owner, boot_id="finished-boot"),
                    heartbeat_at=(
                        dt.datetime.now(dt.timezone.utc)
                        - dt.timedelta(seconds=record.heartbeat_ttl_seconds + 1)
                    ).isoformat(timespec="seconds"),
                ),
            ),
            action="test-owner-exited",
            slot=slot,
        )
    set_liveness(project, "dead")
    in_use = checkout(project, slot="in-use", slot_type="validate")
    calls: list[tuple[Path, ...]] = []

    def census(paths: list[Path]) -> wrkslots._ProcessPathCensus:
        calls.append(tuple(paths))
        return wrkslots._ProcessPathCensus(
            (), ((12345, str(in_use.parent), "link", str(in_use)),)
        )

    monkeypatch.setattr(wrkslots, "_capture_process_path_census", census)

    assert (
        wrkslots.main(
            ["--project-root", str(project), "audit", "--format", "json"]
        )
        == 0
    )
    payload = json.loads(capsys.readouterr().out)
    rows = {row["slot"]: row for row in payload["slots"]}
    assert len(calls) == 1
    assert set(calls[0]) == {
        checkout(project, slot="unused", slot_type="validate").parent,
        in_use.parent,
    }
    assert rows["unused"]["verdict"] == "DELETABLE"
    assert rows["in-use"]["verdict"] == "BLOCKED"
    assert any("live process 12345" in reason for reason in rows["in-use"]["reasons"])


def test_audit_reuses_one_git_worktree_list_for_shared_repository(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    for index in (1, 2):
        made = create(
            project,
            slot=f"slot{index:02d}",
            agent=f"codex-{index}",
            branch=f"codex/task-{index}",
        )
        assert made.returncode == 0, made.stderr
    calls: list[Path] = []
    original = wrkslots._GitVcs.listed_worktrees

    def counted(vcs: wrkslots._GitVcs, source: Path) -> set[Path]:
        calls.append(source)
        return original(vcs, source)

    monkeypatch.setattr(wrkslots._GitVcs, "listed_worktrees", counted)
    monkeypatch.setattr(
        wrkslots,
        "_capture_process_path_census",
        lambda _paths: wrkslots._ProcessPathCensus((), ()),
    )

    assert (
        wrkslots.main(
            ["--project-root", str(project), "audit", "--format", "json"]
        )
        == 0
    )
    payload = json.loads(capsys.readouterr().out)

    assert len(payload["slots"]) == 2
    assert calls == [repository]


def test_audit_does_not_replay_event_log_per_record(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(
        project, slot="slot01", agent="codex-1", branch="codex/one"
    ).returncode == 0
    assert create(
        project, slot="slot02", agent="codex-2", branch="codex/two"
    ).returncode == 0
    held = command(project, "hold", "slot01", "--reason", "retain")
    assert held.returncode == 0, held.stderr
    handoff = slots_directory(project) / "slot02" / "HANDOFF.md"
    handoff.write_text("resume here\n", encoding="utf-8")
    read = raw_command(
        project,
        "read-handoff",
        "slot02",
        "--coordinator-pid",
        str(os.getpid()),
    )
    assert read.returncode == 0, read.stderr

    calls: list[str] = []
    original = wrkslots._load_events

    def counted(
        config: wrkslots.Config, machine: str | None = None
    ) -> tuple[dict[str, object], ...]:
        calls.append(machine or config.machine)
        return original(config, machine)

    monkeypatch.setattr(wrkslots, "_load_events", counted)
    monkeypatch.setattr(
        wrkslots,
        "_capture_process_path_census",
        lambda _paths: wrkslots._ProcessPathCensus((), ()),
    )

    assert (
        wrkslots.main(
            ["--project-root", str(project), "audit", "--format", "json"]
        )
        == 0
    )
    payload = json.loads(capsys.readouterr().out)
    rows = {row["slot"]: row for row in payload["slots"]}

    # Active state, archived state, audit row observations, and recovery
    # journals each inspect the log once; neither audit row adds another pass.
    assert calls == ["testhost"] * 4
    assert rows["slot01"]["verdict"] == "HELD"
    assert not any(
        "unread HANDOFF.md" in reason for reason in rows["slot02"]["reasons"]
    )


def test_git_operation_paths_uses_one_git_query(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    checkout = tmp_path / "checkout"
    checkout.mkdir()
    merge_head = checkout / ".git" / "MERGE_HEAD"
    merge_head.parent.mkdir()
    merge_head.write_text("0" * 40 + "\n", encoding="ascii")
    paths = (
        merge_head,
        checkout / ".git" / "CHERRY_PICK_HEAD",
        checkout / ".git" / "REVERT_HEAD",
        checkout / ".git" / "BISECT_LOG",
        checkout / ".git" / "rebase-apply",
        checkout / ".git" / "rebase-merge",
        checkout / ".git" / "sequencer",
    )
    calls: list[tuple[Path, tuple[str, ...]]] = []
    vcs = wrkslots._GitVcs()

    def run(
        repository: Path,
        arguments: Sequence[str],
        **_kwargs: object,
    ) -> subprocess.CompletedProcess[str]:
        calls.append((repository, tuple(arguments)))
        return subprocess.CompletedProcess(
            arguments, 0, "".join(f"{path}\n" for path in paths), ""
        )

    monkeypatch.setattr(vcs, "_run", run)

    assert vcs.operation_paths(checkout) == [merge_head]
    assert len(calls) == 1
    assert calls[0][0] == checkout
    assert calls[0][1].count("--git-path") == len(paths)


def test_remove_validates_the_target_without_unrelated_storage(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    made = create(project, slot="target", slot_type="validate", branch=None)
    assert made.returncode == 0, made.stderr
    unrelated = create(project, slot="missing", agent="other", branch="other/task")
    assert unrelated.returncode == 0, unrelated.stderr
    config = wrkslots._load_config(str(project), "testhost")
    state = wrkslots._load_active(config)
    target = next(record for record in state.slots if record.slot == "target")
    assert target.owner is not None
    wrkslots._write_active_state(
        config,
        wrkslots._replace_record(
            state,
            replace(
                target,
                owner=replace(target.owner, boot_id="finished-boot"),
            ),
        ),
        action="test-owner-exited",
        slot="target",
    )
    set_liveness(project, "dead")
    missing = checkout(project, slot="missing")
    git(repository, "worktree", "remove", "--force", str(missing))
    monkeypatch.setattr(
        wrkslots, "_assert_slot_unused", lambda *_args, **_kwargs: None
    )

    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "remove",
            "target",
            "--validate-complete",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--expected-generation",
            "1",
        ]
    )

    assert rc == 0, capsys.readouterr().err
    assert not checkout(project, slot="target", slot_type="validate").exists()
    assert any(
        isinstance(row, dict) and row.get("slot") == "missing"
        for row in active_slots(project)
    )


def prepare_validation_remove_beside_create(
    tmp_path: Path, *, legacy_journal: bool = False
) -> tuple[Path, Path]:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project, slot="target", slot_type="validate", branch=None)
    assert made.returncode == 0, made.stderr
    mark_owner_dead(project)
    set_liveness(project, "dead")
    interrupted = create(
        project,
        slot="unrelated",
        agent="other",
        branch="other/unrelated",
        env={"WRKSLOTS_TEST_INTERRUPT": "after-create-worktree"},
    )
    assert interrupted.returncode == 86, interrupted.stderr
    journal = create_journal_path(project, "unrelated")
    if legacy_journal:
        config = wrkslots._load_config(str(project), "testhost")
        raw = json.loads(journal.read_text(encoding="utf-8"))
        wrkslots._clear_journal(config, raw, journal_path=journal)
        journal = wrkslots._journal_path(config)
        wrkslots._write_journal(config, raw, journal_path=journal)
    return project, journal


def remove_completed_validation(project: Path, slot: str = "target") -> int:
    return wrkslots.main(
        [
            "--project-root",
            str(project),
            "remove",
            slot,
            "--validate-complete",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--expected-generation",
            "1",
        ]
    )


def stable_file(path: Path) -> tuple[bytes, int, int]:
    metadata = path.stat(follow_symlinks=False)
    return path.read_bytes(), metadata.st_ino, metadata.st_mtime_ns


class _FinishInterrupted(RuntimeError):
    pass


def interrupt_after_finish_journal(point: str) -> None:
    if point == "after-finish-journal":
        raise _FinishInterrupted


@pytest.mark.parametrize(
    ("create_state", "legacy_journal"),
    (("live", False), ("dead", False), ("live", True), ("dead", True)),
)
def test_remove_completed_validation_with_unrelated_interrupted_create(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    create_state: str,
    legacy_journal: bool,
) -> None:
    project, journal = prepare_validation_remove_beside_create(
        tmp_path, legacy_journal=legacy_journal
    )
    unfinished = checkout(project, slot="unrelated") / "unfinished.txt"
    unfinished.write_text("authored work\n", encoding="utf-8")
    journal_before = stable_file(journal)
    unfinished_before = stable_file(unfinished)
    slot_before = checkout(project, slot="unrelated").stat(follow_symlinks=False)
    if create_state == "dead":
        original = wrkslots._process_state
        monkeypatch.setattr(
            wrkslots,
            "_process_state",
            lambda identity: (
                ("dead", "test process exited")
                if identity is not None and identity.pid == os.getpid()
                else original(identity)
            ),
        )
    monkeypatch.setattr(wrkslots, "_assert_slot_unused", lambda *_a, **_k: None)

    assert remove_completed_validation(project) == 0, capsys.readouterr().err
    assert not checkout(project, slot="target", slot_type="validate").exists()
    assert stable_file(journal) == journal_before
    assert stable_file(unfinished) == unfinished_before
    slot_after = checkout(project, slot="unrelated").stat(follow_symlinks=False)
    assert (slot_after.st_ino, slot_after.st_mtime_ns) == (
        slot_before.st_ino,
        slot_before.st_mtime_ns,
    )


@pytest.mark.parametrize("change_create", (False, True))
def test_recover_exact_scoped_finish_beside_unrelated_create(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    change_create: bool,
) -> None:
    project, create_journal = prepare_validation_remove_beside_create(
        tmp_path, legacy_journal=True
    )
    create_before = stable_file(create_journal)
    monkeypatch.setattr(wrkslots, "_assert_slot_unused", lambda *_a, **_k: None)
    monkeypatch.setattr(
        wrkslots, "_interrupt_for_test", interrupt_after_finish_journal
    )
    with pytest.raises(_FinishInterrupted):
        remove_completed_validation(project)
    finish_journal = finish_journal_path(project, "target")
    assert finish_journal.is_file()
    monkeypatch.setattr(wrkslots, "_interrupt_for_test", lambda _point: None)
    if change_create:
        raw = json.loads(create_journal.read_text(encoding="utf-8"))
        raw["purpose"] = "changed before exact recovery"
        create_journal.write_text(
            json.dumps(raw, sort_keys=True) + "\n", encoding="utf-8"
        )
        create_before = stable_file(create_journal)

    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "recover",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--slot",
            "target",
        ]
    )
    if change_create:
        assert rc == 3
        assert (
            "standalone recovery journal differs from append-only operation evidence"
            in capsys.readouterr().err
        )
        assert finish_journal.is_file()
        assert checkout(project, slot="target", slot_type="validate").is_dir()
        assert stable_file(create_journal) == create_before
        return

    assert rc == 0
    assert not finish_journal.exists()
    assert not checkout(project, slot="target", slot_type="validate").exists()
    assert stable_file(create_journal) == create_before


def test_remove_rechecks_unrelated_create_before_first_mutation(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, journal = prepare_validation_remove_beside_create(tmp_path)
    active = control_directory(project) / "ACTIVE.testhost.json"
    active_before = active.read_bytes()

    def change_journal(*_args: object, **_kwargs: object) -> None:
        raw = json.loads(journal.read_text(encoding="utf-8"))
        raw["purpose"] = "changed during removal classification"
        journal.write_text(json.dumps(raw, sort_keys=True) + "\n", encoding="utf-8")

    monkeypatch.setattr(wrkslots, "_assert_slot_unused", change_journal)

    assert remove_completed_validation(project) == 3
    assert "changed while it was classified" in capsys.readouterr().err
    assert active.read_bytes() == active_before
    assert checkout(project, slot="target", slot_type="validate").is_dir()
    assert not finish_journal_path(project, "target").exists()


@pytest.mark.parametrize(
    ("collision", "expected"),
    (
        ("slot", "already targets slot 'target'"),
        ("path", "requested slot path"),
        ("branch", "is claimed by interrupted create"),
    ),
)
def test_remove_refuses_interrupted_create_collision_before_mutation(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    collision: str,
    expected: str,
) -> None:
    project, journal = prepare_validation_remove_beside_create(tmp_path)
    config = wrkslots._load_config(str(project), "testhost")
    states, archives = wrkslots._validate_global_state(
        config, require_repository=False
    )
    record = wrkslots._find_record(
        next(state for state in states if state.machine == config.machine), "target"
    )
    row = wrkslots._classify_create_journals(config, states, archives)[0]
    if collision == "slot":
        conflicting = replace(row, slot=record.slot)
    elif collision == "path":
        conflicting = replace(
            row, slot_path=wrkslots._slot_directory(config, record.slot, record.slot_type)
        )
    else:
        target_checkout = record.checkouts[0]
        conflicting = replace(
            row,
            planned=(replace(row.planned[0], repository=target_checkout.repository,
                             branch=target_checkout.branch),),
        )
    active = control_directory(project) / "ACTIVE.testhost.json"
    active_before, journal_before = active.read_bytes(), stable_file(journal)
    monkeypatch.setattr(
        wrkslots, "_classify_create_journals", lambda *_a, **_k: (conflicting,)
    )
    monkeypatch.setattr(
        wrkslots,
        "_write_event_file",
        lambda *_a, **_k: (_ for _ in ()).throw(
            AssertionError("collision reached mutation")
        ),
    )

    assert remove_completed_validation(project) == 3
    assert expected in capsys.readouterr().err
    assert active.read_bytes() == active_before
    assert stable_file(journal) == journal_before
    assert checkout(project, slot="target", slot_type="validate").is_dir()


@pytest.mark.parametrize(
    ("damage", "expected"),
    (
        ("malformed", "malformed JSON"),
        ("provenance", "differs from append-only progress evidence"),
        ("indeterminate", "process state is indeterminate"),
        ("disagreeing", "has disagreeing process state"),
    ),
)
def test_remove_refuses_untrusted_create_journal_before_mutation(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    damage: str,
    expected: str,
) -> None:
    project, journal = prepare_validation_remove_beside_create(tmp_path)
    if damage == "malformed":
        journal.write_text("{broken\n", encoding="utf-8")
    elif damage == "provenance":
        raw = json.loads(journal.read_text(encoding="utf-8"))
        raw["purpose"] = "changed after append-only publication"
        journal.write_text(json.dumps(raw, sort_keys=True) + "\n", encoding="utf-8")
    elif damage == "indeterminate":
        monkeypatch.setattr(
            wrkslots,
            "_process_state",
            lambda _identity: ("indeterminate", "process table unavailable"),
        )
    else:
        states = iter((("live", "owner is live"), ("dead", "coordinator exited")))
        monkeypatch.setattr(wrkslots, "_process_state", lambda _identity: next(states))
    active = control_directory(project) / "ACTIVE.testhost.json"
    active_before = active.read_bytes()
    monkeypatch.setattr(
        wrkslots,
        "_write_event_file",
        lambda *_a, **_k: (_ for _ in ()).throw(
            AssertionError("untrusted journal reached mutation")
        ),
    )

    assert remove_completed_validation(project) == 3
    assert expected in capsys.readouterr().err
    assert active.read_bytes() == active_before
    assert checkout(project, slot="target", slot_type="validate").is_dir()
    assert not finish_journal_path(project, "target").exists()


def test_remove_refuses_unrelated_finish_journal_before_target_mutation(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    for slot in ("target", "other"):
        made = create(project, slot=slot, slot_type="validate", branch=None)
        assert made.returncode == 0, made.stderr
    mark_owner_dead(project)
    config = wrkslots._load_config(str(project), "testhost")
    state = wrkslots._load_active(config)
    other = wrkslots._find_record(state, "other")
    assert other.owner is not None
    dead_other = replace(other, owner=replace(other.owner, boot_id="finished-boot"))
    wrkslots._write_active_state(
        config,
        wrkslots._replace_record(state, dead_other),
        action="test-owner-exited",
        slot="other",
    )
    set_liveness(project, "dead")
    monkeypatch.setattr(wrkslots, "_assert_slot_unused", lambda *_a, **_k: None)
    monkeypatch.setattr(
        wrkslots, "_interrupt_for_test", interrupt_after_finish_journal
    )
    with pytest.raises(_FinishInterrupted):
        remove_completed_validation(project, "other")
    other_journal = finish_journal_path(project, "other")
    active = control_directory(project) / "ACTIVE.testhost.json"
    active_before = active.read_bytes()
    monkeypatch.setattr(wrkslots, "_interrupt_for_test", lambda _point: None)

    assert remove_completed_validation(project) == 3
    assert "interrupted 'finish' mutation" in capsys.readouterr().err
    assert active.read_bytes() == active_before
    assert checkout(project, slot="target", slot_type="validate").is_dir()
    assert other_journal.is_file()


@pytest.mark.parametrize(
    ("obstruction", "expected"),
    (("partial", "partial atomic update found"),
     ("batch-seal", "interrupted validation-batch seal")),
)
def test_scoped_validation_remove_keeps_global_recovery_obstructions(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
    obstruction: str,
    expected: str,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project, slot="target", slot_type="validate", branch=None)
    assert made.returncode == 0, made.stderr
    mark_owner_dead(project)
    set_liveness(project, "dead")
    config = wrkslots._load_config(str(project), "testhost")
    path = (
        finish_journal_path(project, "other").with_suffix(".journal.tmp.crash")
        if obstruction == "partial"
        else wrkslots._validate_batch_seal_journal_path(config)
    )
    path.write_text("{}\n", encoding="utf-8")
    active = control_directory(project) / "ACTIVE.testhost.json"
    active_before = active.read_bytes()

    assert remove_completed_validation(project) == 3
    assert expected in capsys.readouterr().err
    assert path.is_file()
    assert active.read_bytes() == active_before
    assert checkout(project, slot="target", slot_type="validate").is_dir()


def test_remove_ignores_unrelated_unavailable_repository(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, _repository, remote = make_project(tmp_path)
    made = create(project, slot="target", slot_type="validate", branch=None)
    assert made.returncode == 0, made.stderr
    unrelated_repository = project / "repo-unrelated"
    subprocess.run(
        ["git", "clone", str(remote), str(unrelated_repository)],
        check=True,
        capture_output=True,
        text=True,
    )
    unrelated = create(
        project,
        slot="unrelated",
        agent="other",
        branch="other/task",
        repository_name="repo-unrelated",
    )
    assert unrelated.returncode == 0, unrelated.stderr
    config = wrkslots._load_config(str(project), "testhost")
    state = wrkslots._load_active(config)
    target = next(record for record in state.slots if record.slot == "target")
    assert target.owner is not None
    wrkslots._write_active_state(
        config,
        wrkslots._replace_record(
            state,
            replace(target, owner=replace(target.owner, boot_id="finished-boot")),
        ),
        action="test-owner-exited",
        slot="target",
    )
    set_liveness(project, "dead")
    shutil.rmtree(unrelated_repository)
    monkeypatch.setattr(
        wrkslots, "_assert_slot_unused", lambda *_args, **_kwargs: None
    )

    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "remove",
            "target",
            "--validate-complete",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--expected-generation",
            "1",
        ]
    )

    assert rc == 0, capsys.readouterr().err
    state = wrkslots._load_active(config, require_repository=False)
    assert {record.slot for record in state.slots} == {"unrelated"}


def test_remove_recovery_ignores_unrelated_unavailable_repository(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project, _repository, remote = make_project(tmp_path)
    made = create(project, slot="target", slot_type="validate", branch=None)
    assert made.returncode == 0, made.stderr
    unrelated_repository = project / "repo-unrelated"
    subprocess.run(
        ["git", "clone", str(remote), str(unrelated_repository)],
        check=True,
        capture_output=True,
        text=True,
    )
    unrelated = create(
        project,
        slot="unrelated",
        agent="other",
        branch="other/recovery",
        repository_name="repo-unrelated",
    )
    assert unrelated.returncode == 0, unrelated.stderr
    config = wrkslots._load_config(str(project), "testhost")
    state = wrkslots._load_active(config)
    target = next(record for record in state.slots if record.slot == "target")
    assert target.owner is not None
    wrkslots._write_active_state(
        config,
        wrkslots._replace_record(
            state,
            replace(target, owner=replace(target.owner, boot_id="finished-boot")),
        ),
        action="test-owner-exited",
        slot="target",
    )
    set_liveness(project, "dead")
    shutil.rmtree(unrelated_repository)
    monkeypatch.setattr(
        wrkslots, "_assert_slot_unused", lambda *_args, **_kwargs: None
    )

    class Interrupted(RuntimeError):
        pass

    def interrupt_after_finish_journal(point: str) -> None:
        if point == "after-finish-journal":
            raise Interrupted

    monkeypatch.setattr(
        wrkslots, "_interrupt_for_test", interrupt_after_finish_journal
    )
    with pytest.raises(Interrupted):
        wrkslots.main(
            [
                "--project-root",
                str(project),
                "remove",
                "target",
                "--validate-complete",
                "--coordinator-authorized",
                "--coordinator-pid",
                str(os.getpid()),
                "--expected-generation",
                "1",
            ]
        )
    monkeypatch.setattr(wrkslots, "_interrupt_for_test", lambda _point: None)

    assert (
        wrkslots.main(
            [
                "--project-root",
                str(project),
                "recover",
                "--coordinator-pid",
                str(os.getpid()),
            ]
        )
        == 0
    )
    state = wrkslots._load_active(config, require_repository=False)
    assert {record.slot for record in state.slots} == {"unrelated"}


def test_remove_still_refuses_when_the_target_storage_is_missing(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    made = create(project, slot="target", slot_type="validate", branch=None)
    assert made.returncode == 0, made.stderr
    mark_owner_dead(project)
    set_liveness(project, "dead")
    target = checkout(project, slot="target", slot_type="validate")
    git(repository, "worktree", "remove", "--force", str(target))

    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "remove",
            "target",
            "--validate-complete",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--expected-generation",
            "1",
        ]
    )

    assert rc == 3
    assert "record names checkout 'product' but it is not in" in capsys.readouterr().err
    assert any(
        isinstance(row, dict) and row.get("slot") == "target"
        for row in active_slots(project)
    )


def test_remove_refuses_exact_target_extra_registration_at_selection(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    made = create(project, slot="target", slot_type="validate", branch=None)
    assert made.returncode == 0, made.stderr
    mark_owner_dead(project)
    set_liveness(project, "dead")
    target = checkout(project, slot="target", slot_type="validate")
    extra = target.parent / "extra-registration"
    git(repository, "worktree", "add", "-b", "extra/selection", str(extra), "main")
    shutil.rmtree(extra)
    monkeypatch.setattr(
        wrkslots, "_assert_slot_unused", lambda *_args, **_kwargs: None
    )

    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "remove",
            "target",
            "--validate-complete",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--expected-generation",
            "1",
        ]
    )

    assert rc == 3
    assert "registers" in capsys.readouterr().err
    assert target.is_dir()
    assert any(
        isinstance(row, dict) and row.get("slot") == "target"
        for row in active_slots(project)
    )


def test_remove_rechecks_exact_target_extra_registration_at_delete_boundary(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    made = create(project, slot="target", slot_type="validate", branch=None)
    assert made.returncode == 0, made.stderr
    mark_owner_dead(project)
    set_liveness(project, "dead")
    target = checkout(project, slot="target", slot_type="validate")
    extra = target.parent / "extra-registration"
    original_check = wrkslots._assert_target_record_storage_consistent
    calls = 0

    def check_then_inject(
        config: wrkslots.Config,
        states: list[wrkslots.ActiveState],
        record: wrkslots.ActiveRecord,
    ) -> None:
        nonlocal calls
        calls += 1
        original_check(config, states, record)
        if calls == 1:
            git(repository, "worktree", "add", "-b", "extra/boundary", str(extra), "main")
            shutil.rmtree(extra)

    monkeypatch.setattr(
        wrkslots, "_assert_target_record_storage_consistent", check_then_inject
    )
    monkeypatch.setattr(
        wrkslots, "_assert_slot_unused", lambda *_args, **_kwargs: None
    )

    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "remove",
            "target",
            "--validate-complete",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--expected-generation",
            "1",
        ]
    )

    assert rc == 3
    assert calls == 2
    assert "registers" in capsys.readouterr().err
    assert target.is_dir()
    assert any(
        isinstance(row, dict) and row.get("slot") == "target"
        for row in active_slots(project)
    )


def test_remove_refuses_cross_repository_target_registration_at_selection(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, other, target = prepare_cross_repository_target(tmp_path)
    extra = target.parent / "foreign-registration"
    git(other, "worktree", "add", "-b", "foreign/selection", str(extra), "main")
    shutil.rmtree(extra)
    monkeypatch.setattr(
        wrkslots, "_assert_slot_unused", lambda *_args, **_kwargs: None
    )

    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "remove",
            "target",
            "--validate-complete",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--expected-generation",
            "1",
        ]
    )

    assert rc == 3
    assert "registers unexpected path" in capsys.readouterr().err
    assert target.is_dir()


def test_remove_rechecks_cross_repository_registration_at_delete_boundary(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, other, target = prepare_cross_repository_target(tmp_path)
    extra = target.parent / "foreign-registration"
    original_check = wrkslots._assert_target_record_storage_consistent
    calls = 0

    def check_then_inject(
        config: wrkslots.Config,
        states: Sequence[wrkslots.ActiveState],
        record: wrkslots.ActiveRecord,
    ) -> None:
        nonlocal calls
        calls += 1
        original_check(config, states, record)
        if calls == 1:
            git(
                other,
                "worktree",
                "add",
                "-b",
                "foreign/boundary",
                str(extra),
                "main",
            )
            shutil.rmtree(extra)

    monkeypatch.setattr(
        wrkslots, "_assert_target_record_storage_consistent", check_then_inject
    )
    monkeypatch.setattr(
        wrkslots, "_assert_slot_unused", lambda *_args, **_kwargs: None
    )

    rc = wrkslots.main(
        [
            "--project-root",
            str(project),
            "remove",
            "target",
            "--validate-complete",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--expected-generation",
            "1",
        ]
    )

    assert rc == 3
    assert calls == 2
    assert "registers unexpected path" in capsys.readouterr().err
    assert target.is_dir()


def test_unpushed_distinguishes_absent_and_rewritten_same_named_remote(
    tmp_path: Path,
) -> None:
    project, _repository, remote = make_project(tmp_path)
    assert create(
        project,
        slot="slot01",
        agent="codex-1",
        branch="codex/unpublished",
    ).returncode == 0
    unpublished = checkout(project, "slot01")
    unpublished_head = commit_local(
        unpublished,
        "unpublished.txt",
        "local only\n",
        "same-looking subject",
    )

    assert create(
        project,
        slot="slot02",
        agent="codex-2",
        branch="codex/rewritten",
    ).returncode == 0
    rewritten = checkout(project, "slot02")
    literal_pathspec_name = ":(exclude)*"
    (rewritten / literal_pathspec_name).write_text(
        "old local history\n", encoding="utf-8"
    )
    git(rewritten, "--literal-pathspecs", "add", literal_pathspec_name)
    git(rewritten, "commit", "-m", "same-looking subject")
    rewritten_head = git(rewritten, "rev-parse", "HEAD").stdout.strip()
    git(rewritten, "push", "-u", "origin", "codex/rewritten")

    updater = tmp_path / "updater"
    subprocess.run(
        ["git", "clone", str(remote), str(updater)],
        check=True,
        capture_output=True,
        text=True,
    )
    git(updater, "config", "user.name", "Wrkslots Test")
    git(updater, "config", "user.email", "wrkslots@example.invalid")
    git(updater, "checkout", "-b", "replacement", "origin/main")
    (updater / literal_pathspec_name).write_text(
        "new remote history\n", encoding="utf-8"
    )
    git(updater, "--literal-pathspecs", "add", literal_pathspec_name)
    git(updater, "commit", "-m", "same-looking subject")
    git(updater, "push", "--force", "origin", "HEAD:codex/rewritten")
    state_before = (control_directory(project) / "ACTIVE.testhost.json").read_bytes()

    report = command(project, "unpushed", "--format", "json")

    assert report.returncode == 0, report.stderr
    rows = {row["slot"]: row for row in json.loads(report.stdout)["slots"]}
    unpublished_row = rows["slot01"]
    assert unpublished_row["checkout"] == "product"
    assert unpublished_row["head"] == unpublished_head
    assert unpublished_row["containing_remote_refs"] == []
    assert unpublished_row["same_named_remote_ref"] is None
    assert unpublished_row["diagnostic_command"] is None

    rewritten_row = rows["slot02"]
    assert rewritten_row["checkout"] == "product"
    assert rewritten_row["head"] == rewritten_head
    assert rewritten_row["containing_remote_refs"] == []
    assert rewritten_row["same_named_remote_ref"] == (
        "refs/remotes/origin/codex/rewritten"
    )
    expected_command = shlex.join(
        [
            "git",
            "--literal-pathspecs",
            "-C",
            str(rewritten),
            "diff",
            rewritten_head,
            "refs/remotes/origin/codex/rewritten",
            "--",
            literal_pathspec_name,
        ]
    )
    assert rewritten_row["diagnostic_command"] == expected_command
    assert rewritten_row["touched_files"] == [literal_pathspec_name]
    comparison = subprocess.run(
        shlex.split(expected_command),
        text=True,
        capture_output=True,
        check=False,
    )
    assert comparison.returncode == 0, comparison.stderr
    assert comparison.stdout
    assert (control_directory(project) / "ACTIVE.testhost.json").read_bytes() == state_before

    unpublished_refusal = finish(project, slot="slot01", agent="codex-1")
    assert unpublished_refusal.returncode == 3
    assert "same-named remote ref" in unpublished_refusal.stderr
    assert "does not exist" in unpublished_refusal.stderr

    rewritten_refusal = finish(project, slot="slot02", agent="codex-2")
    assert rewritten_refusal.returncode == 3
    assert expected_command in rewritten_refusal.stderr

    git(unpublished, "checkout", "-b", "scratch/wrong-branch")
    wrong_branch = command(
        project, "unpushed", "--slot", "slot01", "--format", "json"
    )
    assert wrong_branch.returncode == 3
    assert "branch changed" in wrong_branch.stderr


def mark_recorded_owner_dead(project: Path, slot: str) -> wrkslots.ActiveRecord:
    config = wrkslots._load_config(str(project), "testhost")
    state = wrkslots._load_active(config)
    record = wrkslots._find_record(state, slot)
    assert record.owner is not None
    updated = replace(record, owner=replace(record.owner, pid=2_147_483_647))
    wrkslots._write_active_state(
        config,
        wrkslots._replace_record(state, updated),
        action="test-owner-exited",
        slot=slot,
    )
    return updated


def prepare_absent_validate_row(
    project: Path,
    repository: Path,
    *,
    slot: str,
    agent: str,
    unregister: bool = True,
) -> wrkslots.ActiveRecord:
    made = create(
        project,
        slot=slot,
        agent=agent,
        branch=None,
        slot_type="validate",
    )
    assert made.returncode == 0, made.stderr
    record = mark_recorded_owner_dead(project, slot)
    make_validate_row_absent(project, repository, record, unregister=unregister)
    return record


def make_validate_row_absent(
    project: Path,
    repository: Path,
    record: wrkslots.ActiveRecord,
    *,
    unregister: bool = True,
) -> None:
    config = wrkslots._load_config(str(project), "testhost")
    checkout_path = wrkslots._stored_path(config, record.checkouts[0].path, "test checkout")
    if unregister:
        git(repository, "worktree", "remove", "--force", "--", str(checkout_path))
    else:
        shutil.rmtree(checkout_path)
    slot_path = wrkslots._slot_directory(config, record.slot, "validate")
    if slot_path.exists():
        slot_path.rmdir()


def write_absent_validate_input(project: Path, records: list[wrkslots.ActiveRecord]) -> Path:
    path = project / "absent-validate-rows.json"
    path.write_text(
        json.dumps(
            {
                "schema": 1,
                "rows": [
                    {
                        "machine": record.machine,
                        "slot": record.slot,
                        "generation": record.generation,
                        "record_sha256": wrkslots._record_sha256(record),
                    }
                    for record in records
                ],
            },
            indent=2,
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )
    return path


def run_absent_validate_recovery(
    project: Path,
    input_path: Path,
    *,
    apply: bool,
    output_format: str = "human",
) -> int:
    args = [
        "--project-root",
        str(project),
        "recover-absent-validate-rows",
        "--input",
        str(input_path),
        "--format",
        output_format,
    ]
    if apply:
        args.extend(
            (
                "--apply",
                "--coordinator-authorized",
                "--coordinator-pid",
                str(os.getpid()),
            )
        )
    return wrkslots.main(args)


def allow_test_host_for_absent_validate_recovery(
    project: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    set_liveness(project, "dead")
    monkeypatch.setattr(wrkslots, "_short_hostname", lambda: "testhost")
    monkeypatch.setattr(wrkslots, "_absent_validate_process_snapshot", lambda: ())
    monkeypatch.setattr(
        wrkslots,
        "_absent_validate_privileged_path_match",
        lambda _processes, _targets, _budget=None: None,
    )
    monkeypatch.setattr(wrkslots, "_user_systemd_snapshot", lambda: ())


def prepare_absent_agent_row(
    project: Path, repository: Path, *, slot: str = "gone-agent"
) -> wrkslots.ActiveRecord:
    made = create(project, slot=slot, agent=f"agent-{slot}", branch=f"agent/{slot}")
    assert made.returncode == 0, made.stderr
    record = mark_recorded_owner_dead(project, slot)
    slot_path = wrkslots._slot_directory(
        wrkslots._load_config(str(project), "testhost"), slot, "agent"
    )
    checkout_path = wrkslots._stored_path(
        wrkslots._load_config(str(project), "testhost"),
        record.checkouts[0].path,
        "test checkout",
    )
    shutil.rmtree(slot_path)
    assert checkout_path.absolute() in wrkslots._GitVcs().listed_worktrees(repository)
    return record


def prepare_absent_agent_row_with_sidecar(
    project: Path,
    repository: Path,
    tmp_path: Path,
    *,
    read: bool,
) -> tuple[wrkslots.ActiveRecord, Path]:
    made = create(
        project,
        slot="gone-agent",
        agent="agent-gone-agent",
        branch="agent/gone-agent",
    )
    assert made.returncode == 0, made.stderr
    source = tmp_path / "external-handoff.md"
    source.write_text("preserve this handoff\n", encoding="utf-8")
    written = raw_command(
        project,
        "write-handoff",
        "gone-agent",
        "--agent",
        "agent-gone-agent",
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        "--from-file",
        str(source),
    )
    assert written.returncode == 0, written.stderr
    if read:
        acknowledged = raw_command(
            project,
            "read-handoff",
            "gone-agent",
            "--coordinator-pid",
            str(os.getpid()),
        )
        assert acknowledged.returncode == 0, acknowledged.stderr
    record = mark_recorded_owner_dead(project, "gone-agent")
    config = wrkslots._load_config(str(project), "testhost")
    sidecar = wrkslots._handoff_sidecar_path(config, record.slot, record.generation)
    assert sidecar.is_file()
    shutil.rmtree(wrkslots._slot_directory(config, record.slot, "agent"))
    assert wrkslots._GitVcs().worktree_registration(
        repository,
        wrkslots._stored_path(config, record.checkouts[0].path, "test checkout"),
    ) is not None
    return record, sidecar


def run_absent_agent_recovery(
    project: Path, record: wrkslots.ActiveRecord, *, apply: bool
) -> int:
    args = [
        "--project-root",
        str(project),
        "recover-absent-agent-row",
        record.slot,
        "--expected-generation",
        str(record.generation),
        "--record-sha256",
        wrkslots._record_sha256(record),
    ]
    if apply:
        args.extend(
            (
                "--apply",
                "--coordinator-authorized",
                "--coordinator-pid",
                str(os.getpid()),
            )
        )
    return wrkslots.main(args)


def test_recover_absent_agent_row_refuses_unread_external_sidecar(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    record, sidecar = prepare_absent_agent_row_with_sidecar(
        project, repository, tmp_path, read=False
    )
    set_liveness(project, "dead")
    allow_test_host_for_absent_validate_recovery(project, monkeypatch)

    assert run_absent_agent_recovery(project, record, apply=True) == 3
    assert "unread HANDOFF.md or handoff sidecar" in capsys.readouterr().err
    config = wrkslots._load_config(str(project), "testhost")
    assert wrkslots._find_record(wrkslots._load_active(config), record.slot) == record
    assert sidecar.is_file()
    assert not (config.control / "ACTIVE.testhost.journal").exists()


def test_recover_absent_agent_row_cleans_read_sidecar_only_after_archive(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    record, sidecar = prepare_absent_agent_row_with_sidecar(
        project, repository, tmp_path, read=True
    )
    set_liveness(project, "dead")
    allow_test_host_for_absent_validate_recovery(project, monkeypatch)

    class Interrupted(RuntimeError):
        pass

    def interrupt(point: str) -> None:
        if point == "after-absent-agent-archive":
            raise Interrupted

    monkeypatch.setattr(wrkslots, "_interrupt_for_test", interrupt)
    with pytest.raises(Interrupted):
        run_absent_agent_recovery(project, record, apply=True)
    config = wrkslots._load_config(str(project), "testhost")
    assert wrkslots._find_record(wrkslots._load_active(config), record.slot) == record
    assert sidecar.is_file()
    assert wrkslots._load_archive(config).records[-1]["slot"] == record.slot

    monkeypatch.setattr(wrkslots, "_interrupt_for_test", lambda _point: None)
    assert (
        wrkslots.main(
            [
                "--project-root",
                str(project),
                "recover",
                "--coordinator-authorized",
                "--coordinator-pid",
                str(os.getpid()),
            ]
        )
        == 0
    )
    assert not wrkslots._load_active(config).slots
    assert not sidecar.exists()


def test_recover_absent_agent_row_preserves_commit_before_registry_repair(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, repository, remote = make_project(tmp_path)
    record = prepare_absent_agent_row(project, repository)
    set_liveness(project, "dead")
    allow_test_host_for_absent_validate_recovery(project, monkeypatch)

    assert run_absent_agent_recovery(project, record, apply=False) == 0
    assert run_absent_agent_recovery(project, record, apply=True) == 0

    config = wrkslots._load_config(str(project), "testhost")
    assert not wrkslots._load_active(config).slots
    archived = wrkslots._load_archive(config).records[-1]
    assert archived["slot"] == record.slot
    assert archived["limitations"] == [
        "working-tree contents were absent before recovery; tracked and ordinary "
        "untracked content and HANDOFF could not be inspected or salvaged; ignored "
        "content could not be inspected and would not have been uploaded"
    ]
    rescue_ref = wrkslots._absent_agent_rescue_ref(record, record.checkouts[0])
    assert git(remote, "rev-parse", rescue_ref).stdout.strip() == record.checkouts[0].head
    path = wrkslots._stored_path(config, record.checkouts[0].path, "test checkout")
    assert wrkslots._GitVcs().worktree_registration(repository, path) is None
    assert run_absent_agent_recovery(project, record, apply=True) == 0


def test_recover_absent_agent_row_accepts_unchanged_unregistered_branch(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, repository, _remote = make_project(tmp_path)
    record = prepare_absent_agent_row(project, repository)
    git(repository, "worktree", "prune", "--expire=now")
    set_liveness(project, "dead")
    allow_test_host_for_absent_validate_recovery(project, monkeypatch)
    config = wrkslots._load_config(str(project), "testhost")
    path = wrkslots._stored_path(config, record.checkouts[0].path, "test checkout")
    assert wrkslots._GitVcs().worktree_registration(repository, path) is None
    assert (
        git(
            repository,
            "rev-parse",
            f"refs/heads/{record.checkouts[0].branch}",
        ).stdout.strip()
        == record.checkouts[0].head
    )

    assert run_absent_agent_recovery(project, record, apply=True) == 0
    assert not wrkslots._load_active(config).slots


def test_recover_absent_agent_row_refuses_missing_recorded_local_branch(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, remote = make_project(tmp_path)
    record = prepare_absent_agent_row(project, repository)
    git(repository, "worktree", "prune", "--expire=now")
    checkout_record = record.checkouts[0]
    branch_ref = f"refs/heads/{checkout_record.branch}"
    git(repository, "update-ref", "-d", branch_ref)
    set_liveness(project, "dead")
    allow_test_host_for_absent_validate_recovery(project, monkeypatch)

    assert run_absent_agent_recovery(project, record, apply=True) == 3
    assert "recorded checkout local branch" in capsys.readouterr().err
    config = wrkslots._load_config(str(project), "testhost")
    assert wrkslots._find_record(wrkslots._load_active(config), record.slot) == record
    assert not (config.control / "ACTIVE.testhost.journal").exists()
    rescue_ref = wrkslots._absent_agent_rescue_ref(record, checkout_record)
    assert git(remote, "rev-parse", rescue_ref, check=False).returncode != 0


def test_recover_absent_agent_row_refuses_branch_ahead_before_journal(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, remote = make_project(tmp_path)
    record = prepare_absent_agent_row(project, repository)
    git(repository, "worktree", "prune", "--expire=now")
    checkout_record = record.checkouts[0]
    tree = git(repository, "rev-parse", f"{checkout_record.head}^{{tree}}").stdout.strip()
    ahead = git(
        repository,
        "commit-tree",
        tree,
        "-p",
        checkout_record.head,
        "-m",
        "unrecorded branch work",
    ).stdout.strip()
    branch_ref = f"refs/heads/{checkout_record.branch}"
    git(repository, "update-ref", branch_ref, ahead, checkout_record.head)
    set_liveness(project, "dead")
    allow_test_host_for_absent_validate_recovery(project, monkeypatch)

    assert run_absent_agent_recovery(project, record, apply=True) == 3
    assert "moved from recorded HEAD" in capsys.readouterr().err
    config = wrkslots._load_config(str(project), "testhost")
    assert wrkslots._find_record(wrkslots._load_active(config), record.slot) == record
    assert git(repository, "rev-parse", branch_ref).stdout.strip() == ahead
    assert not (config.control / "ACTIVE.testhost.journal").exists()
    rescue_ref = wrkslots._absent_agent_rescue_ref(record, checkout_record)
    assert git(remote, "rev-parse", rescue_ref, check=False).returncode != 0


def test_recover_absent_agent_row_rechecks_branch_before_archive(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, remote = make_project(tmp_path)
    record = prepare_absent_agent_row(project, repository)
    git(repository, "worktree", "prune", "--expire=now")
    checkout_record = record.checkouts[0]
    tree = git(repository, "rev-parse", f"{checkout_record.head}^{{tree}}").stdout.strip()
    ahead = git(
        repository,
        "commit-tree",
        tree,
        "-p",
        checkout_record.head,
        "-m",
        "branch advances during recovery",
    ).stdout.strip()
    branch_ref = f"refs/heads/{checkout_record.branch}"
    original_push = wrkslots._GitVcs.push_salvage
    advanced = False

    def push_then_advance(
        self: wrkslots._GitVcs,
        checkout_path: Path,
        remote_name: str,
        commit: str,
        rescue_ref: str,
    ) -> None:
        nonlocal advanced
        original_push(self, checkout_path, remote_name, commit, rescue_ref)
        if not advanced:
            git(repository, "update-ref", branch_ref, ahead, checkout_record.head)
            advanced = True

    monkeypatch.setattr(wrkslots._GitVcs, "push_salvage", push_then_advance)
    set_liveness(project, "dead")
    allow_test_host_for_absent_validate_recovery(project, monkeypatch)

    assert run_absent_agent_recovery(project, record, apply=True) == 3
    assert "moved from recorded HEAD" in capsys.readouterr().err
    config = wrkslots._load_config(str(project), "testhost")
    assert wrkslots._find_record(wrkslots._load_active(config), record.slot) == record
    assert git(repository, "rev-parse", branch_ref).stdout.strip() == ahead
    assert not wrkslots._load_archive(config).records
    rescue_ref = wrkslots._absent_agent_rescue_ref(record, checkout_record)
    assert git(remote, "rev-parse", rescue_ref).stdout.strip() == checkout_record.head


def test_recover_absent_agent_row_refuses_live_or_changed_evidence(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, repository, _remote = make_project(tmp_path)
    record = prepare_absent_agent_row(project, repository)
    allow_test_host_for_absent_validate_recovery(project, monkeypatch)

    set_liveness(project, "alive")
    assert run_absent_agent_recovery(project, record, apply=False) == 3
    assert "reports owner alive" in capsys.readouterr().err

    set_liveness(project, "dead")
    changed = replace(record, generation=record.generation + 1)
    assert run_absent_agent_recovery(project, changed, apply=False) == 3
    assert "generation changed" in capsys.readouterr().err


def test_recover_absent_agent_row_accepts_no_owner_only_with_full_dead_evidence(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, repository, _remote = make_project(tmp_path)
    made = create(
        project,
        slot="unbound-gone",
        agent="agent-unbound-gone",
        branch="agent/unbound-gone",
        bind_owner=False,
    )
    assert made.returncode == 0, made.stderr
    config = wrkslots._load_config(str(project), "testhost")
    record = wrkslots._find_record(wrkslots._load_active(config), "unbound-gone")
    assert record.owner is None
    shutil.rmtree(wrkslots._slot_directory(config, record.slot, "agent"))
    set_liveness(project, "dead")
    allow_test_host_for_absent_validate_recovery(project, monkeypatch)

    assert run_absent_agent_recovery(project, record, apply=True) == 0
    assert not wrkslots._load_active(config).slots


def test_recover_absent_agent_row_remote_failure_changes_no_registry_state(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, repository, _remote = make_project(tmp_path)
    record = prepare_absent_agent_row(project, repository)
    set_liveness(project, "dead")
    allow_test_host_for_absent_validate_recovery(project, monkeypatch)
    config = wrkslots._load_config(str(project), "testhost")
    before = wrkslots._record_to_obj(wrkslots._load_active(config).slots[0])

    def refuse_push(*_args: object, **_kwargs: object) -> None:
        raise wrkslots.Refusal("remote unavailable")

    monkeypatch.setattr(wrkslots._GitVcs, "push_salvage", refuse_push)
    assert run_absent_agent_recovery(project, record, apply=True) == 3
    after = wrkslots._record_to_obj(wrkslots._load_active(config).slots[0])
    assert after == before
    assert wrkslots._GitVcs().worktree_registration(
        repository,
        wrkslots._stored_path(config, record.checkouts[0].path, "test checkout"),
    ) is not None


@pytest.mark.parametrize(
    "point",
    (
        "after-absent-agent-journal",
        "after-absent-agent-rescue-ref",
        "after-absent-agent-registration-remove",
        "after-absent-agent-archive",
        "after-absent-agent-active-delete",
    ),
)
def test_recover_absent_agent_row_resumes_each_durable_boundary(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    point: str,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    record = prepare_absent_agent_row(project, repository)
    set_liveness(project, "dead")
    allow_test_host_for_absent_validate_recovery(project, monkeypatch)

    class Interrupted(RuntimeError):
        pass

    def interrupt(observed: str) -> None:
        if observed == point:
            raise Interrupted

    monkeypatch.setattr(wrkslots, "_interrupt_for_test", interrupt)
    with pytest.raises(Interrupted):
        run_absent_agent_recovery(project, record, apply=True)
    monkeypatch.setattr(wrkslots, "_interrupt_for_test", lambda _point: None)
    assert (
        wrkslots.main(
            [
                "--project-root",
                str(project),
                "recover",
                "--coordinator-authorized",
                "--coordinator-pid",
                str(os.getpid()),
            ]
        )
        == 0
    )
    config = wrkslots._load_config(str(project), "testhost")
    assert not wrkslots._load_active(config).slots


def prepare_ownerless_agent_worktree(
    project: Path, repository: Path, *, slot: str = "ownerless"
) -> tuple[Path, str, str]:
    target = slots_directory(project) / slot
    branch = f"agent/{slot}"
    git(repository, "worktree", "add", "-b", branch, str(target), "HEAD")
    head = git(target, "rev-parse", "HEAD").stdout.strip()
    remote_digest = wrkslots._GitVcs().remote_url_sha256(target, "origin")
    return target, head, remote_digest


def allow_test_host_for_ownerless_agent_recovery(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(wrkslots, "_short_hostname", lambda: "testhost")
    monkeypatch.setattr(wrkslots, "_assert_slot_unused", lambda *_args, **_kwargs: None)
    monkeypatch.setattr(wrkslots, "_user_systemd_snapshot", lambda: ())


def write_pre_handoff_ownerless_agent_journal(
    project: Path,
    target: Path,
    head: str,
    remote_digest: str,
) -> dict[str, object]:
    """Write the exact schema-2 journal shape produced before handoff digests."""

    config = wrkslots._load_config(str(project), "testhost")
    authorization: dict[str, object] = {
        "path": target.relative_to(project).as_posix(),
        "identity": list(
            wrkslots._open_directory_identity(target, "ownerless agent worktree")
        ),
        "actor": wrkslots._identity_to_obj(
            wrkslots._read_process_identity(os.getpid())
        ),
        "repository": "repo",
        "head": head,
        "branch": f"agent/{target.name}",
        "remote": "origin",
        "remote_url_sha256": remote_digest,
        "recorded_at": wrkslots._utc_now(),
    }
    assert set(authorization) == wrkslots._OWNERLESS_AGENT_AUTHORIZATION_PRE_HANDOFF_REQUIRED
    journal: dict[str, object] = {
        "schema": 2,
        "kind": "ownerless-agent-remove",
        "machine": "testhost",
        "slot": target.name,
        "phase": "prepared",
        "fenced": (
            target.with_name(f".{target.name}.ownerless-agent.{'a' * 32}")
            .relative_to(project)
            .as_posix()
        ),
        "authorization": authorization,
        "salvage": [],
    }
    wrkslots._write_journal(config, journal)
    return journal


def run_ownerless_agent_recovery(
    project: Path,
    target: Path,
    head: str,
    remote_digest: str,
    *,
    apply: bool,
    handoff_sha256: str | None = None,
) -> int:
    args = [
        "--project-root",
        str(project),
        "recover-ownerless-agent-worktree",
        target.relative_to(project).as_posix(),
        "--repository",
        "repo",
        "--head",
        head,
        "--branch",
        f"agent/{target.name}",
        "--remote",
        "origin",
        "--remote-url-sha256",
        remote_digest,
    ]
    if handoff_sha256 is not None:
        args.extend(("--handoff-sha256", handoff_sha256))
    if apply:
        args.extend(
            (
                "--apply",
                "--coordinator-authorized",
                "--coordinator-pid",
                str(os.getpid()),
            )
        )
    return wrkslots.main(args)


def test_recover_ownerless_agent_worktree_salvages_dirty_tree_without_owner(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, repository, remote = make_project(tmp_path)
    target, head, remote_digest = prepare_ownerless_agent_worktree(project, repository)
    (target / "uncommitted.txt").write_text("preserve me\n", encoding="utf-8")
    allow_test_host_for_ownerless_agent_recovery(monkeypatch)

    assert run_ownerless_agent_recovery(
        project, target, head, remote_digest, apply=False
    ) == 0
    assert run_ownerless_agent_recovery(
        project, target, head, remote_digest, apply=True
    ) == 0
    assert not target.exists()
    assert target.absolute() not in wrkslots._GitVcs().listed_worktrees(repository)

    config = wrkslots._load_config(str(project), "testhost")
    events = wrkslots._load_events(config)
    event = next(
        value for value in reversed(events) if value["kind"] == "ownerless-agent-worktree-removed"
    )
    payload = wrkslots._as_mapping(event["payload"], "ownerless event payload")
    assert "owner" not in payload and "task" not in payload and "handoff" not in payload
    receipt = wrkslots._as_mapping(
        wrkslots._as_list(payload["salvage"], "ownerless salvage")[0],
        "ownerless salvage receipt",
    )
    assert receipt["disposition"] == "salvaged"
    rescue_ref = wrkslots._as_str(receipt["remote_ref"], "ownerless rescue ref")
    salvage_commit = git(remote, "rev-parse", rescue_ref).stdout.strip()
    assert salvage_commit == receipt["salvage_commit"]
    assert git(remote, "show", f"{salvage_commit}:uncommitted.txt").stdout == "preserve me\n"


def test_recover_ownerless_agent_worktree_resumes_literal_pre_handoff_journal(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, repository, _remote = make_project(tmp_path)
    target, head, remote_digest = prepare_ownerless_agent_worktree(project, repository)
    allow_test_host_for_ownerless_agent_recovery(monkeypatch)
    journal = write_pre_handoff_ownerless_agent_journal(
        project, target, head, remote_digest
    )

    with pytest.raises(wrkslots.StateError, match="missing handoff_sha256"):
        wrkslots._ownerless_agent_authorization_from_obj(journal["authorization"])
    assert (
        wrkslots.main(
            [
                "--project-root",
                str(project),
                "recover",
                "--coordinator-authorized",
                "--coordinator-pid",
                str(os.getpid()),
            ]
        )
        == 0
    )
    assert not target.exists()
    event = next(
        value
        for value in reversed(
            wrkslots._load_events(wrkslots._load_config(str(project), "testhost"))
        )
        if value["kind"] == "ownerless-agent-worktree-removed"
    )
    payload = wrkslots._as_mapping(event["payload"], "ownerless event payload")
    authorization = wrkslots._as_mapping(
        payload["authorization"], "ownerless authorization"
    )
    assert authorization["handoff_sha256"] is None


@pytest.mark.parametrize(
    ("conflict", "message"),
    (("handoff", "unread HANDOFF.md"), ("head", "HEAD or branch changed")),
)
def test_pre_handoff_ownerless_agent_journal_still_refuses_changed_facts(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    conflict: str,
    message: str,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    target, head, remote_digest = prepare_ownerless_agent_worktree(project, repository)
    allow_test_host_for_ownerless_agent_recovery(monkeypatch)
    if conflict == "handoff":
        (target / "HANDOFF.md").write_text("unread\n", encoding="utf-8")
    elif conflict == "head":
        head = "0" * 40
    write_pre_handoff_ownerless_agent_journal(project, target, head, remote_digest)

    assert (
        wrkslots.main(
            [
                "--project-root",
                str(project),
                "recover",
                "--coordinator-authorized",
                "--coordinator-pid",
                str(os.getpid()),
            ]
        )
        == 3
    )
    assert message in capsys.readouterr().err
    assert target.exists()


def test_recover_ownerless_agent_worktree_requires_read_handoff_and_preserves_it(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, repository, remote = make_project(tmp_path)
    target, head, remote_digest = prepare_ownerless_agent_worktree(project, repository)
    allow_test_host_for_ownerless_agent_recovery(monkeypatch)
    (target / "HANDOFF.md").write_text("unread\n", encoding="utf-8")
    assert run_ownerless_agent_recovery(
        project, target, head, remote_digest, apply=True
    ) == 3
    error = capsys.readouterr().err
    assert "unread HANDOFF.md" in error
    assert "--handoff-sha256" in error
    assert target.exists()

    handoff_digest = hashlib.sha256(b"unread\n").hexdigest()
    assert run_ownerless_agent_recovery(
        project,
        target,
        head,
        remote_digest,
        apply=True,
        handoff_sha256=handoff_digest,
    ) == 0
    config = wrkslots._load_config(str(project), "testhost")
    event = next(
        value
        for value in reversed(wrkslots._load_events(config))
        if value["kind"] == "ownerless-agent-worktree-removed"
    )
    payload = wrkslots._as_mapping(event["payload"], "ownerless event payload")
    authorization = wrkslots._as_mapping(
        payload["authorization"], "ownerless authorization"
    )
    assert authorization["handoff_sha256"] == handoff_digest
    receipt = wrkslots._as_mapping(
        wrkslots._as_list(payload["salvage"], "ownerless salvage")[0],
        "ownerless salvage receipt",
    )
    salvage_commit = git(
        remote,
        "rev-parse",
        wrkslots._as_str(receipt["remote_ref"], "ownerless rescue ref"),
    ).stdout.strip()
    assert git(remote, "show", f"{salvage_commit}:HANDOFF.md").stdout == "unread\n"


def test_recover_ownerless_agent_worktree_refuses_handoff_change(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, repository, _remote = make_project(tmp_path)
    target, head, remote_digest = prepare_ownerless_agent_worktree(project, repository)
    allow_test_host_for_ownerless_agent_recovery(monkeypatch)
    (target / "HANDOFF.md").write_text("read me\n", encoding="utf-8")
    digest = hashlib.sha256(b"read me\n").hexdigest()

    class Interrupted(RuntimeError):
        pass

    def interrupt(point: str) -> None:
        if point == "after-ownerless-agent-journal":
            raise Interrupted

    monkeypatch.setattr(wrkslots, "_interrupt_for_test", interrupt)
    with pytest.raises(Interrupted):
        run_ownerless_agent_recovery(
            project,
            target,
            head,
            remote_digest,
            apply=True,
            handoff_sha256=digest,
        )
    (target / "HANDOFF.md").write_text("changed\n", encoding="utf-8")
    monkeypatch.setattr(wrkslots, "_interrupt_for_test", lambda _point: None)
    assert run_ownerless_agent_recovery(
        project,
        target,
        head,
        remote_digest,
        apply=True,
        handoff_sha256=digest,
    ) == 3
    assert "HANDOFF.md changed" in capsys.readouterr().err
    assert target.exists()


def test_recover_ownerless_agent_worktree_refuses_identity_change(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, repository, _remote = make_project(tmp_path)
    target, _head, remote_digest = prepare_ownerless_agent_worktree(project, repository)
    allow_test_host_for_ownerless_agent_recovery(monkeypatch)
    assert run_ownerless_agent_recovery(
        project, target, "0" * 40, remote_digest, apply=False
    ) == 3
    assert "HEAD or branch changed" in capsys.readouterr().err


def test_recover_ownerless_agent_worktree_refuses_active_user_unit(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, repository, _remote = make_project(tmp_path)
    target, head, remote_digest = prepare_ownerless_agent_worktree(project, repository)
    monkeypatch.setattr(wrkslots, "_short_hostname", lambda: "testhost")
    monkeypatch.setattr(wrkslots, "_assert_slot_unused", lambda *_args, **_kwargs: None)
    monkeypatch.setattr(
        wrkslots,
        "_user_systemd_snapshot",
        lambda: (
            {
                "Id": "agent.service",
                "ActiveState": "active",
                "PendingJob": "no",
                "WorkingDirectory": str(target),
            },
        ),
    )

    assert run_ownerless_agent_recovery(
        project, target, head, remote_digest, apply=False
    ) == 3
    assert "user-systemd unit agent.service" in capsys.readouterr().err
    assert target.exists()


def test_recover_ownerless_agent_worktree_refuses_inode_change_after_journal(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, repository, _remote = make_project(tmp_path)
    target, head, remote_digest = prepare_ownerless_agent_worktree(project, repository)
    allow_test_host_for_ownerless_agent_recovery(monkeypatch)

    class Interrupted(RuntimeError):
        pass

    monkeypatch.setattr(
        wrkslots,
        "_interrupt_for_test",
        lambda point: (_ for _ in ()).throw(Interrupted())
        if point == "after-ownerless-agent-journal"
        else None,
    )
    with pytest.raises(Interrupted):
        run_ownerless_agent_recovery(project, target, head, remote_digest, apply=True)
    original = target.with_name("ownerless-original")
    target.rename(original)
    target.mkdir()
    monkeypatch.setattr(wrkslots, "_interrupt_for_test", lambda _point: None)
    assert (
        wrkslots.main(
            [
                "--project-root",
                str(project),
                "recover",
                "--coordinator-authorized",
                "--coordinator-pid",
                str(os.getpid()),
            ]
        )
        == 3
    )
    assert "identity changed" in capsys.readouterr().err
    assert original.exists() and target.exists()


def test_recover_ownerless_agent_worktree_salvages_initialized_nested_repositories(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, repository, _remote = make_project(tmp_path)
    add_recursive_submodules(tmp_path, project, repository)
    target, head, remote_digest = prepare_ownerless_agent_worktree(project, repository)
    git(
        target,
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "update",
        "--init",
        "--recursive",
    )
    (target / "component" / "uncommitted.txt").write_text(
        "nested component\n", encoding="utf-8"
    )
    (target / "component" / "leaf" / "uncommitted.txt").write_text(
        "nested leaf\n", encoding="utf-8"
    )
    allow_test_host_for_ownerless_agent_recovery(monkeypatch)

    assert run_ownerless_agent_recovery(
        project, target, head, remote_digest, apply=True
    ) == 0
    events = wrkslots._load_events(wrkslots._load_config(str(project), "testhost"))
    event = next(
        value for value in reversed(events) if value["kind"] == "ownerless-agent-worktree-removed"
    )
    payload = wrkslots._as_mapping(event["payload"], "ownerless event payload")
    receipts = [
        wrkslots._as_mapping(value, "ownerless salvage receipt")
        for value in wrkslots._as_list(payload["salvage"], "ownerless salvage")
    ]
    assert {receipt["checkout"] for receipt in receipts} == {
        "worktree",
        "worktree/component",
        "worktree/component/leaf",
    }
    for receipt in receipts[1:]:
        assert receipt["disposition"] == "salvaged"
        assert receipt["remote_ref"] is not None


@pytest.mark.parametrize(
    "point",
    (
        "after-ownerless-agent-journal",
        "after-ownerless-agent-salvage",
        "after-ownerless-agent-fence-before-journal",
        "after-ownerless-agent-fence",
        "after-ownerless-agent-remove",
    ),
)
def test_recover_ownerless_agent_worktree_resumes_each_durable_boundary(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, point: str
) -> None:
    project, repository, _remote = make_project(tmp_path)
    target, head, remote_digest = prepare_ownerless_agent_worktree(project, repository)
    (target / "dirty.txt").write_text("durable\n", encoding="utf-8")
    allow_test_host_for_ownerless_agent_recovery(monkeypatch)

    class Interrupted(RuntimeError):
        pass

    def interrupt(observed: str) -> None:
        if observed == point:
            raise Interrupted

    monkeypatch.setattr(wrkslots, "_interrupt_for_test", interrupt)
    with pytest.raises(Interrupted):
        run_ownerless_agent_recovery(project, target, head, remote_digest, apply=True)
    monkeypatch.setattr(wrkslots, "_interrupt_for_test", lambda _point: None)
    assert (
        wrkslots.main(
            [
                "--project-root",
                str(project),
                "recover",
                "--coordinator-authorized",
                "--coordinator-pid",
                str(os.getpid()),
            ]
        )
        == 0
    )
    assert not target.exists()


def prepare_ownerless_agent_cache(project: Path) -> Path:
    source = slots_directory(project) / "ignored"
    payload = source / "validate" / "cache" / "rust-script" / "projects" / "one"
    payload.mkdir(parents=True)
    (payload / "Cargo.toml").write_text("[package]\nname='cached'\n", encoding="utf-8")
    return source


def run_ownerless_agent_cache_recovery(project: Path, *, apply: bool) -> int:
    args = ["--project-root", str(project), "recover-ownerless-agent-cache"]
    if apply:
        args.extend(
            (
                "--apply",
                "--coordinator-authorized",
                "--coordinator-pid",
                str(os.getpid()),
            )
        )
    return wrkslots.main(args)


def test_recover_ownerless_agent_cache_relocates_exact_tree(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    source = prepare_ownerless_agent_cache(project)
    allow_test_host_for_ownerless_agent_recovery(monkeypatch)

    assert run_ownerless_agent_cache_recovery(project, apply=False) == 0
    assert run_ownerless_agent_cache_recovery(project, apply=True) == 0
    destination = project / "ignored" / "validate" / "cache" / "wrkslots-agent-ignored"
    assert not source.exists()
    assert (destination / "validate/cache/rust-script/projects/one/Cargo.toml").is_file()


def test_recover_ownerless_agent_cache_refuses_arbitrary_contents(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    source = prepare_ownerless_agent_cache(project)
    (source / "authored.txt").write_text("not cache\n", encoding="utf-8")
    allow_test_host_for_ownerless_agent_recovery(monkeypatch)

    assert run_ownerless_agent_cache_recovery(project, apply=True) == 3
    assert "unexpected entry" in capsys.readouterr().err
    assert source.exists()


@pytest.mark.parametrize(
    "point",
    (
        "after-ownerless-agent-cache-journal",
        "after-ownerless-agent-cache-fence",
        "after-ownerless-agent-cache-relocate",
    ),
)
def test_recover_ownerless_agent_cache_resumes_each_durable_boundary(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, point: str
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    source = prepare_ownerless_agent_cache(project)
    allow_test_host_for_ownerless_agent_recovery(monkeypatch)

    class Interrupted(RuntimeError):
        pass

    def interrupt(observed: str) -> None:
        if observed == point:
            raise Interrupted

    monkeypatch.setattr(wrkslots, "_interrupt_for_test", interrupt)
    with pytest.raises(Interrupted):
        run_ownerless_agent_cache_recovery(project, apply=True)
    monkeypatch.setattr(wrkslots, "_interrupt_for_test", lambda _point: None)
    assert (
        wrkslots.main(
            [
                "--project-root",
                str(project),
                "recover",
                "--coordinator-authorized",
                "--coordinator-pid",
                str(os.getpid()),
            ]
        )
        == 0
    )
    destination = project / "ignored" / "validate" / "cache" / "wrkslots-agent-ignored"
    assert not source.exists() and destination.is_dir()


def test_recover_absent_validate_rows_handles_terminal_and_recordless_rows(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, repository, _remote = make_project(tmp_path)
    terminal_made = create(
        project,
        slot="terminal",
        agent="validate-a",
        branch=None,
        slot_type="validate",
    )
    assert terminal_made.returncode == 0, terminal_made.stderr
    recordless_made = create(
        project,
        slot="recordless",
        agent="validate-b",
        branch=None,
        slot_type="validate",
    )
    assert recordless_made.returncode == 0, recordless_made.stderr
    kept = create(
        project,
        slot="kept",
        agent="validate-c",
        branch=None,
        slot_type="validate",
    )
    assert kept.returncode == 0, kept.stderr
    config = wrkslots._load_config(str(project), "testhost")
    terminal = mark_recorded_owner_dead(project, "terminal")
    recordless = mark_recorded_owner_dead(project, "recordless")
    make_validate_row_absent(project, repository, terminal)
    make_validate_row_absent(project, repository, recordless)
    kept_before = wrkslots._record_to_obj(
        wrkslots._find_record(wrkslots._load_active(config), "kept")
    )
    terminal_checkout = wrkslots._stored_path(
        config, terminal.checkouts[0].path, "terminal checkout"
    )
    prepare_terminal_validation_record(
        project, terminal_checkout, field="checkout", name="terminal"
    )
    input_path = write_absent_validate_input(project, [terminal, recordless])
    allow_test_host_for_absent_validate_recovery(project, monkeypatch)

    assert run_absent_validate_recovery(project, input_path, apply=False, output_format="json") == 0
    planned = json.loads(capsys.readouterr().out)
    assert [row["outcome"] for row in planned["rows"]] == ["planned", "planned"]
    assert run_absent_validate_recovery(project, input_path, apply=True, output_format="json") == 0
    recovered_outcomes = json.loads(capsys.readouterr().out)
    assert [row["outcome"] for row in recovered_outcomes["rows"]] == [
        "recovered",
        "recovered",
    ]

    state = wrkslots._load_active(config)
    assert [record.slot for record in state.slots] == ["kept"]
    assert wrkslots._record_to_obj(state.slots[0]) == kept_before
    archive = wrkslots._load_archive(config)
    recovered = {str(record["slot"]): record for record in archive.records}
    assert set(recovered) == {"terminal", "recordless"}
    assert all(record["physical_storage"] == "removed" for record in recovered.values())
    assert all(
        "validation outcome unknown"
        in wrkslots._as_list(record["limitations"], "archive limitations")
        for record in recovered.values()
    )
    assert all("recovery" not in record for record in recovered.values())
    assert not (control_directory(project) / "ACTIVE.testhost.journal").exists()

    assert run_absent_validate_recovery(project, input_path, apply=True, output_format="json") == 0
    replay = json.loads(capsys.readouterr().out)
    assert [row["outcome"] for row in replay["rows"]] == [
        "already-recovered",
        "already-recovered",
    ]


def test_audit_row_round_trips_as_absent_validate_input(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project, slot="audited", agent="validate-a", branch=None, slot_type="validate")
    assert made.returncode == 0, made.stderr
    assert wrkslots.main(["--project-root", str(project), "audit", "--format", "json"]) == 0
    audit = json.loads(capsys.readouterr().out)
    row = next(item for item in audit["slots"] if item["slot"] == "audited")
    assert isinstance(row["generation"], int)
    assert len(row["record_sha256"]) == 64
    input_path = project / "round-trip.json"
    input_path.write_text(
        json.dumps(
            {
                "schema": 1,
                "rows": [
                    {
                        key: row[key]
                        for key in (
                            "machine",
                            "slot",
                            "generation",
                            "record_sha256",
                        )
                    }
                ],
            }
        ),
        encoding="utf-8",
    )
    config = wrkslots._load_config(str(project), "testhost")
    _digest, parsed = wrkslots._read_absent_validate_rows_input(config, str(input_path))
    assert parsed == (
        wrkslots.AbsentValidateRow("testhost", "audited", row["generation"], row["record_sha256"]),
    )

    input_path.write_bytes(b" " * (1024 * 1024 + 1))
    with pytest.raises(wrkslots.Refusal, match="exceeds the 1 MiB safety bound"):
        wrkslots._read_absent_validate_rows_input(config, str(input_path))


def test_bounded_regular_file_refuses_fifo_without_blocking(tmp_path: Path) -> None:
    fifo = tmp_path / "retained-handle.json"
    os.mkfifo(fifo)
    script = (
        "from pathlib import Path\n"
        "from wrkslots import cli\n"
        "try:\n"
        f"    cli._read_bounded_regular_file(Path({str(fifo)!r}), 'test FIFO', 1024)\n"
        "except cli.Refusal:\n"
        "    raise SystemExit(0)\n"
        "raise SystemExit(1)\n"
    )
    completed = subprocess.run(
        [sys.executable, "-c", script],
        env=source_environment(),
        capture_output=True,
        text=True,
        timeout=2,
        check=False,
    )
    assert completed.returncode == 0, completed.stderr


def test_bounded_regular_file_refuses_path_swapped_to_fifo(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    target = tmp_path / "retained-handle.json"
    target.write_text("{}", encoding="utf-8")
    original_open = os.open
    observed_flags: list[int] = []

    def swap_then_open(path: os.PathLike[str] | str, flags: int, mode: int = 0o777) -> int:
        observed_flags.append(flags)
        target.unlink()
        os.mkfifo(target)
        monkeypatch.setattr(os, "open", original_open)
        return original_open(path, flags, mode)

    monkeypatch.setattr(os, "open", swap_then_open)
    with pytest.raises(wrkslots.Refusal, match="not a regular file"):
        wrkslots._read_bounded_regular_file(target, "swapped handle", 1024)
    assert observed_flags[0] & os.O_NONBLOCK


def test_absent_storage_archive_preserves_schema_two_shape(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(
        project,
        slot="validate-row",
        agent="validate-a",
        branch=None,
        slot_type="validate",
    )
    assert made.returncode == 0, made.stderr
    config = wrkslots._load_config(str(project), "testhost")
    record = wrkslots._find_record(wrkslots._load_active(config), "validate-row")
    item = wrkslots.AbsentValidateRow(
        record.machine, record.slot, record.generation, wrkslots._record_sha256(record)
    )
    entry = wrkslots._absent_validate_archive_entry(record, item, "2026-09-02T00:00:00+00:00")
    assert set(entry) == {
        "archive_id",
        "slot",
        "agent",
        "task",
        "purpose",
        "slot_type",
        "machine",
        "generation",
        "created_at",
        "finished_at",
        "mode",
        "actor",
        "physical_storage",
        "validation",
        "limitations",
        "continuation",
        "salvage",
        "checkouts",
    }
    assert entry["slot_type"] == "validate"
    assert entry["physical_storage"] == "removed"
    assert "recovery" not in entry
    assert (
        f"source ACTIVE record SHA-256: {item.record_sha256}"
        in wrkslots._as_list(entry["validation"], "archive validation")
    )
    parsed = wrkslots._archive_from_obj(
        config,
        {
            "schema": wrkslots.SCHEMA,
            "machine": "testhost",
            "revision": 1,
            "records": [entry],
        },
        "testhost",
        "test archive",
    )
    assert parsed.records == (entry,)


def test_recovery_replay_rejects_archive_with_only_mimicking_prose(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    record = prepare_absent_validate_row(
        project, repository, slot="prose-only", agent="validate-a"
    )
    config = wrkslots._load_config(str(project), "testhost")
    item = wrkslots.AbsentValidateRow(
        record.machine,
        record.slot,
        record.generation,
        wrkslots._record_sha256(record),
    )
    entry = wrkslots._absent_validate_archive_entry(
        record, item, "2026-09-02T00:00:00+00:00"
    )
    archive = wrkslots._load_archive(config)
    wrkslots._write_archive_state(
        config,
        wrkslots.ArchiveState(
            archive.machine,
            archive.revision + 1,
            (*archive.records, entry),
        ),
        action="slot-archived",
        slot=record.slot,
        evidence={"archive_id": entry["archive_id"]},
    )
    active = wrkslots._load_active(config)
    wrkslots._write_active_state(
        config,
        wrkslots._delete_record(active, record.slot),
        action="test-remove-prose-only-row",
        slot=record.slot,
    )
    input_path = write_absent_validate_input(project, [record])
    allow_test_host_for_absent_validate_recovery(project, monkeypatch)

    assert run_absent_validate_recovery(project, input_path, apply=False) == 3
    assert "exact prior absent-row recovery" in capsys.readouterr().err


@pytest.mark.parametrize(
    "point",
    (
        "after-absent-validate-batch-journal",
        "after-absent-validate-archive",
        "after-absent-validate-row",
        "after-absent-validate-active-row-1",
        "after-absent-validate-active-event",
        "after-absent-validate-active-delete",
    ),
)
def test_absent_validate_batch_resumes_each_durable_boundary(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, point: str
) -> None:
    project, repository, _remote = make_project(tmp_path)
    first_made = create(
        project,
        slot="gone-a",
        agent="validate-a",
        branch=None,
        slot_type="validate",
    )
    assert first_made.returncode == 0, first_made.stderr
    second_made = create(
        project,
        slot="gone-b",
        agent="validate-b",
        branch=None,
        slot_type="validate",
    )
    assert second_made.returncode == 0, second_made.stderr
    config = wrkslots._load_config(str(project), "testhost")
    records = [
        mark_recorded_owner_dead(project, "gone-a"),
        mark_recorded_owner_dead(project, "gone-b"),
    ]
    for record in records:
        make_validate_row_absent(project, repository, record)
    input_path = write_absent_validate_input(project, records)
    allow_test_host_for_absent_validate_recovery(project, monkeypatch)

    class Interrupted(RuntimeError):
        pass

    def interrupt(observed: str) -> None:
        if observed == point:
            raise Interrupted

    monkeypatch.setattr(wrkslots, "_interrupt_for_test", interrupt)
    with pytest.raises(Interrupted):
        run_absent_validate_recovery(project, input_path, apply=True)
    assert (control_directory(project) / "ACTIVE.testhost.journal").is_file()
    if point == "after-absent-validate-active-event":
        assert len(wrkslots._load_active_snapshot(config).slots) == 2
        assert wrkslots._load_active(config).slots == ()

    monkeypatch.setattr(wrkslots, "_interrupt_for_test", lambda _point: None)
    assert run_absent_validate_recovery(project, input_path, apply=True) == 0
    assert active_slots(project) == []
    assert wrkslots._load_active_snapshot(config).slots == ()
    assert not (control_directory(project) / "ACTIVE.testhost.journal").exists()


def test_absent_validate_recovery_parses_event_history_in_bounded_passes_at_127_rows(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, repository, _remote = make_project(tmp_path)
    seed = prepare_absent_validate_row(
        project, repository, slot="scale-seed", agent="validate-scale"
    )
    config = wrkslots._load_config(str(project), "testhost")
    records: list[wrkslots.ActiveRecord] = []
    for index in range(127):
        slot = f"scale-{index:03d}"
        checkout = replace(
            seed.checkouts[0],
            path=seed.checkouts[0].path.replace(seed.slot, slot),
            branch=seed.checkouts[0].branch.replace(seed.slot, slot),
        )
        records.append(
            replace(
                seed,
                slot=slot,
                agent=f"validate-{index:03d}",
                task=f"scale-{index:03d}",
                purpose="bounded recovery scale control",
                checkouts=(checkout,),
            )
        )
    event_directory = wrkslots._event_directory(config)
    shutil.rmtree(event_directory)
    wrkslots._atomic_write_json(
        wrkslots._active_path(config),
        wrkslots._active_to_obj(wrkslots.ActiveState("testhost", 1, tuple(records))),
    )
    wrkslots._atomic_write_json(
        wrkslots._archive_path(config),
        wrkslots._archive_to_obj(wrkslots.ArchiveState("testhost", 0, ())),
    )
    input_path = write_absent_validate_input(project, records)
    allow_test_host_for_absent_validate_recovery(project, monkeypatch)
    original_load_events = wrkslots._load_events
    original_listed_worktrees = wrkslots._GitVcs.listed_worktrees
    original_archive_from_obj = wrkslots._archive_from_obj
    original_active_from_obj = wrkslots._active_from_obj
    loads = 0
    worktree_listings = 0
    archive_rows_parsed = 0
    active_rows_parsed = 0

    def counted_load_events(
        cfg: wrkslots.Config, machine: str | None = None
    ) -> tuple[dict[str, object], ...]:
        nonlocal loads
        loads += 1
        return original_load_events(cfg, machine)

    monkeypatch.setattr(wrkslots, "_load_events", counted_load_events)

    def counted_listed_worktrees(
        vcs: wrkslots._GitVcs, repository_path: Path
    ) -> set[Path]:
        nonlocal worktree_listings
        worktree_listings += 1
        return original_listed_worktrees(vcs, repository_path)

    monkeypatch.setattr(wrkslots._GitVcs, "listed_worktrees", counted_listed_worktrees)

    def counted_archive_from_obj(
        cfg: wrkslots.Config, value: object, selected: str, label: str
    ) -> wrkslots.ArchiveState:
        nonlocal archive_rows_parsed
        raw = wrkslots._as_mapping(value, "counted archive")
        archive_rows_parsed += len(wrkslots._as_list(raw["records"], "counted records"))
        return original_archive_from_obj(cfg, value, selected, label)

    def counted_active_from_obj(
        cfg: wrkslots.Config, value: object, selected: str, label: str
    ) -> wrkslots.ActiveState:
        nonlocal active_rows_parsed
        raw = wrkslots._as_mapping(value, "counted active")
        active_rows_parsed += len(wrkslots._as_list(raw["slots"], "counted slots"))
        return original_active_from_obj(cfg, value, selected, label)

    monkeypatch.setattr(wrkslots, "_archive_from_obj", counted_archive_from_obj)
    monkeypatch.setattr(wrkslots, "_active_from_obj", counted_active_from_obj)

    assert run_absent_validate_recovery(project, input_path, apply=True, output_format="json") == 0
    outcomes = json.loads(capsys.readouterr().out)["rows"]
    assert len(outcomes) == 127
    assert {row["outcome"] for row in outcomes} == {"recovered"}
    assert wrkslots._load_active_snapshot(config).slots == ()
    assert len(wrkslots._load_archive_snapshot(config).records) == 127
    assert loads <= 16
    assert worktree_listings <= 2
    assert archive_rows_parsed <= 24 * len(records)
    assert active_rows_parsed <= 24 * len(records)
    events = original_load_events(config)
    assert sum(event["kind"] == "operation-progress-recorded" for event in events) == 2
    assert sum(event["kind"] == "archive-state-recorded" for event in events) == len(records)
    assert sum(event["kind"] == "active-state-recorded" for event in events) == len(records)
    archive_events = [
        wrkslots._as_mapping(event["payload"], "archive event")
        for event in events
        if event["kind"] == "archive-state-recorded"
    ]
    assert all(event["action"] == "absent-validation-row-recovered" for event in archive_events)
    assert all(
        set(wrkslots._as_mapping(event["evidence"], "archive evidence"))
        == {"archive_id", "source_record_sha256", "validation_outcome"}
        for event in archive_events
    )
    removal_events = [
        wrkslots._as_mapping(event["payload"], "removal event")
        for event in events
        if event["kind"] == "active-state-recorded"
    ]
    assert all(
        set(wrkslots._as_mapping(event["evidence"], "removal evidence"))
        == {"archive_id", "source_record_sha256", "validation_outcome"}
        for event in removal_events
    )


@pytest.mark.parametrize("change", ("generation", "digest"))
def test_absent_validate_recovery_refuses_wrong_row_identity(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    change: str,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    record = prepare_absent_validate_row(project, repository, slot="gone", agent="validate-a")
    input_path = write_absent_validate_input(project, [record])
    value = json.loads(input_path.read_text(encoding="utf-8"))
    row = value["rows"][0]
    if change == "generation":
        row["generation"] += 1
    else:
        row["record_sha256"] = "0" * 64
    input_path.write_text(json.dumps(value), encoding="utf-8")
    allow_test_host_for_absent_validate_recovery(project, monkeypatch)

    assert run_absent_validate_recovery(project, input_path, apply=False, output_format="json") == 3
    captured = capsys.readouterr()
    outcomes = json.loads(captured.out)
    assert [row["outcome"] for row in outcomes["rows"]] == ["refused"]
    assert "changed" in captured.err
    assert len(active_slots(project)) == 1


def test_absent_validate_mixed_batch_refusal_writes_nothing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, repository, _remote = make_project(tmp_path)
    first_made = create(
        project,
        slot="valid-first",
        agent="validate-a",
        branch=None,
        slot_type="validate",
        bind_owner=False,
    )
    assert first_made.returncode == 0, first_made.stderr
    second_made = create(
        project,
        slot="invalid-last",
        agent="validate-b",
        branch=None,
        slot_type="validate",
        bind_owner=False,
    )
    assert second_made.returncode == 0, second_made.stderr
    config = wrkslots._load_config(str(project), "testhost")
    state = wrkslots._load_active(config)
    first = wrkslots._find_record(state, "valid-first")
    second = wrkslots._find_record(state, "invalid-last")
    make_validate_row_absent(project, repository, first)
    make_validate_row_absent(project, repository, second)
    input_path = write_absent_validate_input(project, [first, second])
    value = json.loads(input_path.read_text(encoding="utf-8"))
    value["rows"][1]["record_sha256"] = "0" * 64
    input_path.write_text(json.dumps(value), encoding="utf-8")
    allow_test_host_for_absent_validate_recovery(project, monkeypatch)
    control = control_directory(project)
    before = {
        path.relative_to(control): path.read_bytes()
        for path in control.rglob("*")
        if path.is_file()
    }

    assert run_absent_validate_recovery(project, input_path, apply=True, output_format="json") == 3
    outcomes = json.loads(capsys.readouterr().out)["rows"]
    assert [row["outcome"] for row in outcomes] == ["refused", "refused"]
    after = {
        path.relative_to(control): path.read_bytes()
        for path in control.rglob("*")
        if path.is_file()
    }
    assert after == before


def test_absent_validate_mixed_prior_recovery_and_invalid_active_row_writes_nothing(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    recovered_made = create(
        project,
        slot="already",
        agent="validate-a",
        branch=None,
        slot_type="validate",
    )
    assert recovered_made.returncode == 0, recovered_made.stderr
    invalid_made = create(
        project,
        slot="invalid",
        agent="validate-b",
        branch=None,
        slot_type="validate",
    )
    assert invalid_made.returncode == 0, invalid_made.stderr
    recovered = mark_recorded_owner_dead(project, "already")
    invalid = mark_recorded_owner_dead(project, "invalid")
    make_validate_row_absent(project, repository, recovered)
    make_validate_row_absent(project, repository, invalid)
    allow_test_host_for_absent_validate_recovery(project, monkeypatch)
    recovered_input = write_absent_validate_input(project, [recovered])
    assert run_absent_validate_recovery(project, recovered_input, apply=True) == 0
    capsys.readouterr()

    mixed_input = write_absent_validate_input(project, [recovered, invalid])
    value = json.loads(mixed_input.read_text(encoding="utf-8"))
    value["rows"][1]["record_sha256"] = "0" * 64
    mixed_input.write_text(json.dumps(value), encoding="utf-8")
    control = control_directory(project)
    before = {
        path.relative_to(control): path.read_bytes()
        for path in control.rglob("*")
        if path.is_file()
    }

    assert run_absent_validate_recovery(project, mixed_input, apply=True, output_format="json") == 3
    outcomes = json.loads(capsys.readouterr().out)["rows"]
    assert [row["outcome"] for row in outcomes] == [
        "already-recovered",
        "refused",
    ]
    after = {
        path.relative_to(control): path.read_bytes()
        for path in control.rglob("*")
        if path.is_file()
    }
    assert after == before


def test_absent_validate_recovery_refuses_agent_row_and_existing_path(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, repository, _remote = make_project(tmp_path)
    agent = create(project, slot="agent-row", agent="agent-a")
    assert agent.returncode == 0, agent.stderr
    config = wrkslots._load_config(str(project), "testhost")
    agent_record = wrkslots._find_record(wrkslots._load_active(config), "agent-row")
    input_path = write_absent_validate_input(project, [agent_record])
    allow_test_host_for_absent_validate_recovery(project, monkeypatch)
    assert run_absent_validate_recovery(project, input_path, apply=False) == 3
    assert "not a validation row" in capsys.readouterr().err

    validate = create(
        project,
        slot="present",
        agent="validate-a",
        branch=None,
        slot_type="validate",
    )
    assert validate.returncode == 0, validate.stderr
    present = wrkslots._find_record(wrkslots._load_active(config), "present")
    input_path = write_absent_validate_input(project, [present])
    assert run_absent_validate_recovery(project, input_path, apply=False) == 3
    assert "still has physical storage" in capsys.readouterr().err


@pytest.mark.parametrize(
    "name",
    (
        ".gone.fenced.{generation}.junk.fenced.x",
        ".gone.ownerless-validate.deadbeef.junk.ownerless-validate.x",
    ),
)
def test_absent_validate_recovery_refuses_multi_marker_fenced_sibling(
    tmp_path: Path, name: str
) -> None:
    project, repository, _remote = make_project(tmp_path)
    record = prepare_absent_validate_row(
        project, repository, slot="gone", agent="validate-a"
    )
    config = wrkslots._load_config(str(project), "testhost")
    sibling = wrkslots._validate_slots_directory(config) / name.format(
        generation=record.generation
    )
    sibling.mkdir()

    with pytest.raises(wrkslots.Refusal, match="possible fenced path"):
        wrkslots._assert_absent_validate_rows_storage(config, (record,))


def test_absent_validate_recovery_refuses_remote_machine(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, repository, _remote = make_project(tmp_path)
    record = prepare_absent_validate_row(project, repository, slot="gone", agent="validate-a")
    input_path = write_absent_validate_input(project, [record])
    monkeypatch.setattr(wrkslots, "_short_hostname", lambda: "another-host")

    assert run_absent_validate_recovery(project, input_path, apply=False, output_format="json") == 3
    captured = capsys.readouterr()
    assert json.loads(captured.out)["rows"][0]["outcome"] == "refused"
    assert "liveness evidence is local to another-host" in captured.err


def test_absent_validate_recovery_binds_stable_host_identity(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(
        project,
        slot="host-bound",
        agent="validate-a",
        branch=None,
        slot_type="validate",
    )
    assert made.returncode == 0, made.stderr
    config = wrkslots._load_config(str(project), "testhost")
    record = wrkslots._find_record(wrkslots._load_active(config), "host-bound")
    foreign = replace(
        record,
        owner=None,
        coordinator_lease=replace(record.coordinator_lease, host_id="foreign-host-id"),
    )
    with pytest.raises(wrkslots.Refusal, match="belongs to stable host"):
        wrkslots._assert_absent_validate_row_is_local(foreign)

    assert record.owner is not None
    mixed = replace(
        record,
        owner=replace(record.owner, host_id="foreign-host-id"),
    )
    with pytest.raises(wrkslots.Refusal, match="missing or mixed"):
        wrkslots._assert_absent_validate_row_is_local(mixed)

    with pytest.raises(wrkslots.Refusal, match="no exact owner process generation"):
        wrkslots._assert_absent_validate_owners_dead((replace(record, owner=None),))

    live = wrkslots._read_process_identity(os.getpid())
    dead_owner = replace(
        record,
        owner=replace(live, pid=2_147_483_647),
    )
    wrkslots._assert_absent_validate_owners_dead((dead_owner,))
    moved_cgroup = replace(
        record,
        owner=replace(live, cgroup_path="/different/cgroup"),
    )
    with pytest.raises(wrkslots.Refusal, match="generation is still live"):
        wrkslots._assert_absent_validate_owners_dead((moved_cgroup,))


def test_absent_validate_recovery_allows_live_agent_without_resource_use(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, repository, _remote = make_project(tmp_path)
    record = prepare_absent_validate_row(project, repository, slot="gone", agent="validate-a")
    config = wrkslots._load_config(str(project), "testhost")
    set_liveness(project, "alive")
    allow_test_host_for_absent_validate_recovery(project, monkeypatch)
    monkeypatch.setattr(
        wrkslots,
        "_registered_liveness_state",
        lambda *_args: (_ for _ in ()).throw(
            AssertionError("agent-name liveness is not resource ownership")
        ),
    )
    wrkslots._assert_absent_validate_rows_safe(config, (record,))


def test_generic_recovery_refuses_on_another_stable_host(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, repository, _remote = make_project(tmp_path)
    record = prepare_absent_validate_row(project, repository, slot="gone", agent="validate-a")
    input_path = write_absent_validate_input(project, [record])
    allow_test_host_for_absent_validate_recovery(project, monkeypatch)

    class Interrupted(RuntimeError):
        pass

    def interrupt(point: str) -> None:
        if point == "after-absent-validate-batch-journal":
            raise Interrupted

    monkeypatch.setattr(wrkslots, "_interrupt_for_test", interrupt)
    with pytest.raises(Interrupted):
        run_absent_validate_recovery(project, input_path, apply=True)
    monkeypatch.setattr(wrkslots, "_interrupt_for_test", lambda _point: None)
    monkeypatch.setattr(wrkslots, "_host_id", lambda: "another-stable-host")

    assert (
        wrkslots.main(
            [
                "--project-root",
                str(project),
                "recover",
                "--coordinator-authorized",
                "--coordinator-pid",
                str(os.getpid()),
            ]
        )
        == 3
    )
    assert "journal belongs to another host identity" in capsys.readouterr().err


def test_generic_recovery_resumes_without_original_input(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, repository, _remote = make_project(tmp_path)
    record = prepare_absent_validate_row(project, repository, slot="gone", agent="validate-a")
    input_path = write_absent_validate_input(project, [record])
    allow_test_host_for_absent_validate_recovery(project, monkeypatch)

    class Interrupted(RuntimeError):
        pass

    def interrupt(point: str) -> None:
        if point == "after-absent-validate-batch-journal":
            raise Interrupted

    monkeypatch.setattr(wrkslots, "_interrupt_for_test", interrupt)
    with pytest.raises(Interrupted):
        run_absent_validate_recovery(project, input_path, apply=True)
    input_path.unlink()
    monkeypatch.setattr(wrkslots, "_interrupt_for_test", lambda _point: None)

    assert (
        wrkslots.main(
            [
                "--project-root",
                str(project),
                "recover",
                "--coordinator-authorized",
                "--coordinator-pid",
                str(os.getpid()),
            ]
        )
        == 0
    )
    assert "outcome=recovered" in capsys.readouterr().out
    assert (
        wrkslots._load_active_snapshot(wrkslots._load_config(str(project), "testhost")).slots == ()
    )


def test_absent_validate_recovery_refuses_git_registration_and_hold(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, repository, _remote = make_project(tmp_path)
    registered_made = create(
        project,
        slot="registered",
        agent="validate-a",
        branch=None,
        slot_type="validate",
    )
    assert registered_made.returncode == 0, registered_made.stderr
    held_made = create(
        project,
        slot="held",
        agent="validate-b",
        branch=None,
        slot_type="validate",
    )
    assert held_made.returncode == 0, held_made.stderr
    config = wrkslots._load_config(str(project), "testhost")
    state = wrkslots._load_active(config)
    registered = wrkslots._find_record(state, "registered")
    held = wrkslots._find_record(state, "held")
    held_result = raw_command(project, "hold", "held", "--reason", "operator hold")
    assert held_result.returncode == 0, held_result.stderr
    make_validate_row_absent(project, repository, registered, unregister=False)
    input_path = write_absent_validate_input(project, [registered])
    allow_test_host_for_absent_validate_recovery(project, monkeypatch)
    assert run_absent_validate_recovery(project, input_path, apply=False) == 3
    assert "Git still registers" in capsys.readouterr().err

    git(repository, "worktree", "prune")
    held_checkout = wrkslots._stored_path(config, held.checkouts[0].path, "held checkout")
    git(repository, "worktree", "remove", "--force", "--", str(held_checkout))
    wrkslots._slot_directory(config, "held", "validate").rmdir()
    input_path = write_absent_validate_input(project, [held])
    assert run_absent_validate_recovery(project, input_path, apply=False) == 3
    assert "is held" in capsys.readouterr().err


@pytest.mark.parametrize("handle_kind", ("unit-mismatch", "malformed"))
def test_absent_validate_recovery_refuses_unsafe_retained_handle(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    handle_kind: str,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    record = prepare_absent_validate_row(project, repository, slot="gone", agent="validate-a")
    config = wrkslots._load_config(str(project), "testhost")
    handle = project / "ignored" / "validate" / "runs" / "handle.json"
    handle.parent.mkdir(parents=True, exist_ok=True)
    if handle_kind == "unit-mismatch":
        handle.write_text(
            json.dumps(
                {
                    "checkout": str(
                        wrkslots._stored_path(config, record.checkouts[0].path, "checkout")
                    ),
                    "state": "running",
                    "unit": "different.service",
                }
            ),
            encoding="utf-8",
        )
    else:
        handle.write_text("{", encoding="utf-8")
    input_path = write_absent_validate_input(project, [record])
    allow_test_host_for_absent_validate_recovery(project, monkeypatch)

    assert run_absent_validate_recovery(project, input_path, apply=False) == 3
    assert "retained validation handle" in capsys.readouterr().err


def test_absent_validate_handle_uses_exact_process_and_unit_liveness(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    pid = os.getpid()
    start_ticks = wrkslots._process_start_ticks(Path("/proc") / str(pid))
    assert start_ticks is not None
    live = wrkslots._RetainedValidationHandle(
        Path("/runs/exact.json"),
        "exact.service",
        pid,
        start_ticks,
        wrkslots._boot_id(Path("/proc")),
    )
    with pytest.raises(wrkslots.Refusal, match="live exact process generation"):
        wrkslots._assert_retained_handle_processes_dead({"gone": (live,)})

    dead = replace(live, pid=2_147_483_647)
    wrkslots._assert_retained_handle_processes_dead({"gone": (dead,)})
    with pytest.raises(wrkslots.Refusal, match="live exact process generation"):
        wrkslots._assert_retained_handle_processes_dead({"gone": (dead, live)})
    record = object.__new__(wrkslots.ActiveRecord)
    object.__setattr__(record, "slot", "gone")
    rows = ((record, (Path("/absent/gone"),)),)
    active_unit = {
        "Id": "exact.service",
        "LoadState": "loaded",
        "ActiveState": "active",
        "SubState": "running",
        "MainPID": "0",
        "ControlGroup": "/user.slice/exact.service",
        "WorkingDirectory": "",
        "ExecStart": "",
        "Environment": "",
        "PendingJob": "no",
    }
    monkeypatch.setattr(wrkslots, "_user_systemd_snapshot", lambda: (active_unit,))
    with pytest.raises(wrkslots.Refusal, match="exact.service may still use"):
        wrkslots._assert_absent_validate_systemd_unrelated(
            rows, {"gone": (dead,)}, ()
        )

    inactive_unit = {**active_unit, "ActiveState": "inactive", "SubState": "dead"}
    monkeypatch.setattr(wrkslots, "_user_systemd_snapshot", lambda: (inactive_unit,))
    exact_process = wrkslots._AbsentProcessObservation(
        123, 17, "/user.slice/exact.service/child", "mnt:[123]"
    )
    with pytest.raises(wrkslots.Refusal, match="live cgroup process"):
        wrkslots._assert_absent_validate_systemd_unrelated(
            rows, {"gone": (dead,)}, (exact_process,)
        )

    unrelated = replace(exact_process, cgroup_path="/shared/launcher.scope")
    wrkslots._assert_absent_validate_systemd_unrelated(
        rows, {"gone": (dead,)}, (unrelated,)
    )


def test_retained_handle_binds_checkout_and_source_checkout_even_when_nonterminal(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    first_made = create(
        project,
        slot="checkout-row",
        agent="validate-a",
        branch=None,
        slot_type="validate",
    )
    assert first_made.returncode == 0, first_made.stderr
    second_made = create(
        project,
        slot="source-row",
        agent="validate-b",
        branch=None,
        slot_type="validate",
    )
    assert second_made.returncode == 0, second_made.stderr
    first = mark_recorded_owner_dead(project, "checkout-row")
    second = mark_recorded_owner_dead(project, "source-row")
    make_validate_row_absent(project, repository, first)
    make_validate_row_absent(project, repository, second)
    config = wrkslots._load_config(str(project), "testhost")
    first_paths = wrkslots._absent_validate_row_paths(config, first)
    second_paths = wrkslots._absent_validate_row_paths(config, second)
    handle_path = project / "ignored" / "validate" / "runs" / "exact-run.json"
    handle_path.parent.mkdir(parents=True, exist_ok=True)
    handle_path.write_text(
        json.dumps(
            {
                "checkout": str(first_paths[-1]),
                "source_checkout": str(second_paths[-1]),
                "unit": "exact-run.service",
                "state": "running",
                "process_identity": {
                    "pid": 2_147_483_647,
                    "start_ticks": 17,
                    "boot_id": wrkslots._boot_id(Path("/proc")),
                },
            }
        ),
        encoding="utf-8",
    )

    bindings = wrkslots._retained_handles_for_absent_rows(
        config,
        ((first, first_paths), (second, second_paths)),
    )
    assert [handle.path for handle in bindings["checkout-row"]] == [handle_path]
    assert [handle.path for handle in bindings["source-row"]] == [handle_path]
    wrkslots._assert_retained_handle_processes_dead(bindings)


@pytest.mark.parametrize("bound", ("count", "bytes", "deadline"))
def test_retained_handle_enumeration_is_bounded_before_sorting(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, bound: str
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    config = wrkslots._load_config(str(project), "testhost")
    handles = project / "ignored" / "validate" / "runs"
    handles.mkdir(parents=True, exist_ok=True)
    (handles / "one.json").write_text("{}", encoding="utf-8")
    if bound == "count":
        (handles / "two.json").write_text("{}", encoding="utf-8")
        monkeypatch.setattr(wrkslots, "_RETAINED_HANDLE_COUNT_LIMIT", 1)
        message = "file-count census bound"
    elif bound == "bytes":
        monkeypatch.setattr(wrkslots, "_RETAINED_HANDLE_BYTES_LIMIT", 1)
        message = "64 MiB census bound"
    else:
        ticks = iter((0.0, 2.0))
        monkeypatch.setattr(wrkslots, "_RETAINED_HANDLE_CENSUS_SECONDS", 1.0)
        monkeypatch.setattr(time, "monotonic", lambda: next(ticks))
        message = "time bound"

    with pytest.raises(wrkslots.Refusal, match=message):
        wrkslots._retained_handles_for_absent_rows(config, ())


def test_absent_validate_recovery_refuses_row_mutation_after_journal(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, repository, _remote = make_project(tmp_path)
    record = prepare_absent_validate_row(project, repository, slot="gone", agent="validate-a")
    input_path = write_absent_validate_input(project, [record])
    allow_test_host_for_absent_validate_recovery(project, monkeypatch)

    class Interrupted(RuntimeError):
        pass

    def interrupt(point: str) -> None:
        if point == "after-absent-validate-batch-journal":
            raise Interrupted

    monkeypatch.setattr(wrkslots, "_interrupt_for_test", interrupt)
    with pytest.raises(Interrupted):
        run_absent_validate_recovery(project, input_path, apply=True)
    config = wrkslots._load_config(str(project), "testhost")
    state = wrkslots._load_active(config)
    current = wrkslots._find_record(state, "gone")
    wrkslots._write_active_state(
        config,
        wrkslots._replace_record(state, replace(current, purpose="changed concurrently")),
        action="test-concurrent-change",
        slot="gone",
    )
    monkeypatch.setattr(wrkslots, "_interrupt_for_test", lambda _point: None)

    assert run_absent_validate_recovery(project, input_path, apply=True) == 3
    assert "changed" in capsys.readouterr().err


def test_absent_validate_process_and_systemd_checks_fail_closed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, repository, _remote = make_project(tmp_path)
    record = prepare_absent_validate_row(project, repository, slot="gone", agent="validate-a")
    target = Path("/absent/validate/gone")
    rows = ((record, (target,)),)
    proc_root = tmp_path / "proc"
    (proc_root / "123").mkdir(parents=True)
    monkeypatch.setattr(
        wrkslots,
        "_read_process_stat",
        lambda _path: wrkslots._ProcessStat(start_ticks=17, flags=0),
    )
    monkeypatch.setattr(
        wrkslots,
        "_process_uid",
        lambda _path: os.getuid() + 1,
    )
    cgroup_reads = 0

    def read_cgroup(_path: Path) -> str:
        nonlocal cgroup_reads
        cgroup_reads += 1
        return "/other"

    monkeypatch.setattr(wrkslots, "_read_process_cgroup", read_cgroup)
    monkeypatch.setattr(wrkslots, "_mount_namespace", lambda _path: "mnt:[123]")
    snapshot = wrkslots._absent_validate_process_snapshot(proc_root)
    assert snapshot == (wrkslots._AbsentProcessObservation(123, 17, "/other", "mnt:[123]"),)
    assert cgroup_reads == 1
    cgroup_reads = 0
    cgroupless = wrkslots._absent_validate_process_snapshot(
        proc_root, include_owner_cgroups=False
    )
    assert cgroupless == (wrkslots._AbsentProcessObservation(123, 17, "", "mnt:[123]"),)
    assert cgroup_reads == 0

    monkeypatch.setattr(
        wrkslots, "_mountinfo_path_references", lambda _pid, _budget, _cache=None: ()
    )
    shared_launcher_cgroup = wrkslots._absent_validate_mount_match(
        (wrkslots._AbsentProcessObservation(123, 17, "/recorded/owner/child", "mnt:[123]"),),
        {target: "gone"},
    )
    assert shared_launcher_cgroup is None

    monkeypatch.setattr(
        wrkslots,
        "_root_owned_executable",
        lambda path, _label: wrkslots._TrustedExecutablePath(path, ()),
    )
    monkeypatch.setattr(
        wrkslots,
        "_run_bounded_read_only_command",
        lambda _command, **_kwargs: (1, b"", b"sudo denied"),
    )
    with pytest.raises(wrkslots.Refusal, match="find census produced diagnostics"):
        wrkslots._absent_validate_find_match(snapshot, {target: "gone"})

    monkeypatch.setattr(
        wrkslots,
        "_user_systemd_snapshot",
        lambda: (
            {
                "Id": "validate.service",
                "LoadState": "loaded",
                "ActiveState": "active",
                "SubState": "running",
                "MainPID": "0",
                "ControlGroup": "/unrelated",
                "WorkingDirectory": str(target),
                "ExecStart": "",
                "Environment": "",
                "PendingJob": "no",
            },
        ),
    )
    with pytest.raises(wrkslots.Refusal, match="names validation row"):
        wrkslots._assert_absent_validate_systemd_unrelated(rows, {"gone": ()}, snapshot)

    monkeypatch.setattr(
        wrkslots,
        "_user_systemd_snapshot",
        lambda: (
            {
                "Id": "queued.scope",
                "LoadState": "loaded",
                "ActiveState": "inactive",
                "SubState": "dead",
                "MainPID": "0",
                "ControlGroup": "/unrelated",
                "WorkingDirectory": "",
                "ExecStart": f"/bin/true {target}",
                "Environment": "",
                "PendingJob": "yes",
            },
        ),
    )
    with pytest.raises(wrkslots.Refusal, match="names validation row"):
        wrkslots._assert_absent_validate_systemd_unrelated(rows, {"gone": ()}, snapshot)


def test_final_slot_scan_uses_filesystem_uid_for_private_fence(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    proc_root = tmp_path / "proc"
    pid_dir = proc_root / "123"
    pid_dir.mkdir(parents=True)
    filesystem_uid = os.getuid()
    inspected: list[Path] = []
    monkeypatch.setattr(
        wrkslots,
        "_process_uids",
        lambda _path: (
            os.getuid() + 1,
            os.getuid() + 1,
            os.getuid() + 1,
            filesystem_uid,
        ),
    )

    def uses(process: Path, _slot: Path) -> list[str]:
        inspected.append(process)
        return ["cwd=/private/fence"]

    monkeypatch.setattr(wrkslots, "_process_uses_slot", uses)
    fallback = wrkslots._ProcessPathCensus((), ())
    with pytest.raises(wrkslots.Refusal, match="live process 123"):
        wrkslots._assert_slot_unused(
            Path("/private/fence"),
            use_lsof=False,
            fallback_census=fallback,
            proc_root=proc_root,
        )
    assert inspected == [pid_dir]

    filesystem_uid = os.getuid() + 1
    inspected.clear()
    wrkslots._assert_slot_unused(
        Path("/private/fence"),
        use_lsof=False,
        fallback_census=fallback,
        proc_root=proc_root,
    )
    assert inspected == []


def test_final_slot_scan_does_not_use_stale_census_for_indeterminate_current_uid(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    proc_root = tmp_path / "proc"
    (proc_root / "123").mkdir(parents=True)
    monkeypatch.setattr(
        wrkslots,
        "_process_uids",
        lambda _path: (os.getuid(), os.getuid(), os.getuid(), os.getuid()),
    )
    monkeypatch.setattr(
        wrkslots,
        "_process_uses_slot",
        lambda _process, _slot: (_ for _ in ()).throw(
            wrkslots.Refusal("fresh process evidence is unreadable")
        ),
    )
    with pytest.raises(wrkslots.Refusal, match="fresh process evidence is unreadable"):
        wrkslots._assert_slot_unused(
            Path("/private/fence"),
            use_lsof=False,
            fallback_census=wrkslots._ProcessPathCensus((), ()),
            proc_root=proc_root,
        )


def test_mount_namespace_reselects_after_representative_generation_changes(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    target = Path("/absent/validate/gone")
    processes = (
        wrkslots._AbsentProcessObservation(123, 17, "/other", "mnt:[same]"),
        wrkslots._AbsentProcessObservation(124, 18, "/other", "mnt:[same]"),
    )
    inspected: list[int] = []

    def mountinfo(
        pid: int,
        _budget: wrkslots._ReadOnlyCommandBudget,
        _cache: dict[str, tuple[tuple[Path, str], ...]] | None = None,
    ) -> tuple[tuple[Path, str], ...]:
        inspected.append(pid)
        return ((target, str(target)),) if pid == 123 else ()

    monkeypatch.setattr(wrkslots, "_mountinfo_path_references", mountinfo)
    monkeypatch.setattr(
        wrkslots,
        "_process_start_ticks",
        lambda path: None if path.name == "123" else 18,
    )
    assert wrkslots._absent_validate_mount_match(processes, {target: "gone"}) is None
    assert inspected == [123, 124]


@pytest.mark.parametrize("persistent", (False, True))
def test_mountinfo_census_retries_only_transient_stable_einval(
    monkeypatch: pytest.MonkeyPatch,
    persistent: bool,
) -> None:
    target = Path("/absent/validate/gone")
    process = wrkslots._AbsentProcessObservation(123, 17, "/other", "mnt:[123]")
    calls = 0

    def mountinfo(
        _pid: int,
        _budget: wrkslots._ReadOnlyCommandBudget,
        _cache: dict[str, tuple[tuple[Path, str], ...]] | None = None,
    ) -> tuple[tuple[Path, str], ...]:
        nonlocal calls
        calls += 1
        if persistent or calls == 1:
            try:
                raise OSError(errno.EINVAL, "transient procfs state")
            except OSError as cause:
                raise wrkslots.Refusal("cannot read mountinfo") from cause
        return ((target, str(target)),)

    monkeypatch.setattr(wrkslots, "_mountinfo_path_references", mountinfo)
    monkeypatch.setattr(wrkslots, "_process_start_ticks", lambda _path: 17)
    if persistent:
        with pytest.raises(wrkslots.Refusal, match="cannot read mountinfo"):
            wrkslots._absent_validate_mount_matches((process,), {target: "gone"})
    else:
        assert wrkslots._absent_validate_mount_matches(
            (process,), {target: "gone"}
        ) == ((123, "gone", "mount", str(target)),)
    assert calls == 2


def test_mountinfo_census_enforces_file_aggregate_and_deadline_bounds(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    observed_limits: list[int] = []

    def bounded_read(_path: Path, _label: str, limit: int) -> bytes:
        observed_limits.append(limit)
        return b"abc"

    monkeypatch.setattr(wrkslots, "_read_bounded_regular_file", bounded_read)
    monkeypatch.setattr(
        wrkslots, "_parse_mountinfo_paths", lambda _text, _label, _cache=None: ()
    )
    budget = wrkslots._ReadOnlyCommandBudget.start(
        timeout_seconds=30,
        stdout_limit=1,
        stderr_limit=1,
        input_limit=5,
    )
    assert wrkslots._mountinfo_path_references(1, budget) == ()
    with pytest.raises(wrkslots.Refusal, match="input bound"):
        wrkslots._mountinfo_path_references(2, budget)
    assert observed_limits == [5, 2]

    expired = wrkslots._ReadOnlyCommandBudget.start(
        timeout_seconds=30,
        stdout_limit=1,
        stderr_limit=1,
        input_limit=wrkslots._MOUNTINFO_CENSUS_BYTES_LIMIT,
    )
    expired.deadline = time.monotonic() - 1
    with pytest.raises(wrkslots.Refusal, match="time bound"):
        wrkslots._mountinfo_path_references(3, expired)


@pytest.mark.parametrize(
    ("evidence_path", "observed"),
    (
        ("/proc/123/cwd", "/absent/validate/gone"),
        ("/proc/123/root", "/absent/validate/gone/root"),
        ("/proc/123/fd/42", "/absent/validate/gone/open"),
    ),
)
def test_privileged_find_census_reports_deleted_process_links(
    monkeypatch: pytest.MonkeyPatch, evidence_path: str, observed: str
) -> None:
    process = wrkslots._AbsentProcessObservation(123, 17, "/other", "mnt:[123]")
    target = Path("/absent/validate/gone")
    monkeypatch.setattr(
        wrkslots,
        "_process_symlink_roots",
        lambda _processes: ("/proc/123/fd",),
    )
    monkeypatch.setattr(wrkslots, "_process_start_ticks", lambda _path: 17)
    monkeypatch.setattr(
        wrkslots,
        "_run_root_owned_command",
        lambda *_args, **_kwargs: (
            0,
            evidence_path.encode() + b"\0" + observed.encode() + b" (deleted)\0",
            b"",
        ),
    )
    assert wrkslots._absent_validate_find_match((process,), {target: "gone"}) == (
        123,
        "gone",
        "link",
        f"{observed} (deleted)",
    )


def test_privileged_grep_census_reports_deleted_mapping(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    process = wrkslots._AbsentProcessObservation(123, 17, "/other", "mnt:[123]")
    target = Path("/absent/validate/gone")
    monkeypatch.setattr(wrkslots, "_process_start_ticks", lambda _path: 17)
    monkeypatch.setattr(
        wrkslots,
        "_run_root_owned_command",
        lambda *_args, **_kwargs: (
            0,
            b"/proc/123/maps:7f00-7f10 r--p 00000000 00:00 0 "
            b"/absent/validate/gone/mapped (deleted)\n",
            b"",
        ),
    )
    assert wrkslots._absent_validate_maps_match((process,), {target: "gone"}) == (
        123,
        "gone",
        "map",
        "/absent/validate/gone/mapped",
    )


def test_process_path_census_collects_every_target_once(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    first = Path("/absent/validate/first")
    second = Path("/absent/validate/second")
    process = wrkslots._AbsentProcessObservation(123, 17, "/other", "mnt:[123]")
    calls: list[str] = []
    monkeypatch.setattr(
        wrkslots, "_absent_validate_process_snapshot", lambda: (process,)
    )

    def mount_matches(
        _processes: object, targets: object, _budget: object
    ) -> tuple[tuple[int, str, str, str], ...]:
        calls.append("mount")
        assert targets == {first: str(first), second: str(second)}
        return ((123, str(first), "mount", str(first)),)

    def find_matches(
        _processes: object, _targets: object, _budget: object
    ) -> tuple[tuple[int, str, str, str], ...]:
        calls.append("find")
        return ((123, str(second), "link", str(second / "open")),)

    def maps_matches(
        _processes: object, _targets: object, _budget: object
    ) -> tuple[tuple[int, str, str, str], ...]:
        calls.append("maps")
        return ()

    monkeypatch.setattr(wrkslots, "_absent_validate_mount_matches", mount_matches)
    monkeypatch.setattr(wrkslots, "_absent_validate_find_matches", find_matches)
    monkeypatch.setattr(wrkslots, "_absent_validate_maps_matches", maps_matches)

    census = wrkslots._capture_process_path_census((first, second))

    assert calls == ["mount", "find", "maps"]
    assert census.processes == (process,)
    assert census.matches == (
        (123, str(first), "mount", str(first)),
        (123, str(second), "link", str(second / "open")),
    )


def test_privileged_process_arguments_batch_and_refuse_oversized_item() -> None:
    batches = wrkslots._argument_batches(
        ("prefix",), ("a" * 20, "b" * 20, "c" * 20), ("suffix",), budget=55
    )
    assert len(batches) > 1
    assert [item for batch in batches for item in batch[1:-1]] == [
        "a" * 20,
        "b" * 20,
        "c" * 20,
    ]
    with pytest.raises(wrkslots.Refusal, match="one .* argument exceeds"):
        wrkslots._argument_batches(("p",), ("x" * 100,), ("s",), budget=32)


def test_privileged_process_diagnostics_allow_only_vanished_generations(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    process = wrkslots._AbsentProcessObservation(123, 17, "/other", "mnt:[123]")
    find_error = b"/usr/bin/find: '/proc/123/fd': No such file or directory\n"
    grep_error = b"/usr/bin/grep: /proc/123/maps: No such file or directory\n"
    monkeypatch.setattr(wrkslots, "_process_start_ticks", lambda _path: None)
    wrkslots._allow_only_vanished_process_diagnostics("find", 1, find_error, {123: process})
    wrkslots._allow_only_vanished_process_diagnostics("grep", 2, grep_error, {123: process})

    monkeypatch.setattr(wrkslots, "_process_start_ticks", lambda _path: 17)
    with pytest.raises(wrkslots.Refusal, match="find census produced diagnostics"):
        wrkslots._allow_only_vanished_process_diagnostics("find", 1, find_error, {123: process})
    with pytest.raises(wrkslots.Refusal, match="grep census produced diagnostics"):
        wrkslots._allow_only_vanished_process_diagnostics("grep", 2, grep_error, {123: process})


def test_kernel_thread_process_scan_omits_only_exe() -> None:
    process = wrkslots._AbsentProcessObservation(123, 17, "/other", "mnt:[123]", kernel_thread=True)
    roots = wrkslots._process_symlink_roots((process,))
    assert roots == ("/proc/123/cwd", "/proc/123/root", "/proc/123/fd")


def test_integrated_process_liveness_clear_and_in_use(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    target = Path("/absent/validate/gone")
    record = object.__new__(wrkslots.ActiveRecord)
    object.__setattr__(record, "slot", "gone")
    object.__setattr__(record, "owner", None)
    rows = ((record, (target,)),)
    process = wrkslots._AbsentProcessObservation(123, 17, "/other", "mnt:[123]")
    monkeypatch.setattr(wrkslots, "_absent_validate_process_snapshot", lambda: (process,))
    monkeypatch.setattr(
        wrkslots,
        "_absent_validate_mount_match",
        lambda *_args: None,
    )
    monkeypatch.setattr(
        wrkslots,
        "_absent_validate_privileged_path_match",
        lambda *_args: None,
    )
    monkeypatch.setattr(
        wrkslots,
        "_process_start_ticks",
        lambda _path: (_ for _ in ()).throw(
            AssertionError("unrelated PID generations must not be globally rechecked")
        ),
    )
    wrkslots._assert_absent_validate_processes_unrelated(rows)

    monkeypatch.setattr(
        wrkslots,
        "_absent_validate_privileged_path_match",
        lambda *_args: (123, "gone", "cwd", str(target)),
    )
    monkeypatch.setattr(wrkslots, "_process_start_ticks", lambda _path: 17)
    with pytest.raises(wrkslots.Refusal, match="live process 123"):
        wrkslots._assert_absent_validate_processes_unrelated(rows)


def test_user_systemd_uses_validated_bus_and_absolute_binary(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("XDG_RUNTIME_DIR", "/tmp/shadowed-user-bus")
    monkeypatch.setenv("DBUS_SESSION_BUS_ADDRESS", "unix:path=/tmp/shadowed-user-bus/bus")
    with pytest.raises(wrkslots.Refusal, match="XDG_RUNTIME_DIR must be"):
        wrkslots._user_bus_environment()

    seen: dict[str, object] = {}
    monkeypatch.setattr(
        wrkslots,
        "_user_bus_environment",
        lambda: {"XDG_RUNTIME_DIR": "/run/user/123"},
    )
    monkeypatch.setattr(
        wrkslots,
        "_root_owned_executable",
        lambda path, _label: wrkslots._TrustedExecutablePath(path, ()),
    )

    def bounded(command: list[str], **kwargs: object) -> tuple[int, bytes, bytes]:
        seen["command"] = command
        seen["kwargs"] = kwargs
        return 0, b"unit.service loaded active running\n", b""

    monkeypatch.setattr(wrkslots, "_run_bounded_read_only_command", bounded)
    budget = wrkslots._ReadOnlyCommandBudget.start(
        timeout_seconds=30,
        stdout_limit=1024 * 1024,
        stderr_limit=64 * 1024,
    )
    assert "unit.service" in wrkslots._run_user_systemctl(("list-units",), budget)
    assert seen["command"] == ["/usr/bin/systemctl", "--user", "list-units"]
    kwargs = seen["kwargs"]
    assert isinstance(kwargs, dict)
    assert 0 < kwargs["timeout_seconds"] <= 30
    assert kwargs["stdout_limit"] == 1024 * 1024
    assert kwargs["stderr_limit"] == 64 * 1024
    assert kwargs["env_overrides"] == {"XDG_RUNTIME_DIR": "/run/user/123"}


def test_bounded_read_only_command_refuses_output_and_time_overruns() -> None:
    with pytest.raises(wrkslots.Refusal, match="stdout exceeded"):
        wrkslots._run_bounded_read_only_command(["/usr/bin/printf", "12345"], stdout_limit=4)
    with pytest.raises(wrkslots.Refusal, match="exceeded"):
        wrkslots._run_bounded_read_only_command(["/usr/bin/sleep", "1"], timeout_seconds=0.01)
    with pytest.raises(wrkslots.Refusal, match="exceeded"):
        wrkslots._run_bounded_read_only_command(
            ["/usr/bin/sleep", "1"],
            timeout_seconds=0.01,
            input_data=b"x" * (256 * 1024),
        )


def test_same_uid_command_preserves_shared_deadline_and_byte_accounting(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    clock = [10.0]
    monkeypatch.setattr(time, "monotonic", lambda: clock[0])
    budget = wrkslots._ReadOnlyCommandBudget.start(
        timeout_seconds=22, stdout_limit=100, stderr_limit=100, input_limit=100
    )
    deadline = budget.deadline
    clock[0] = 13.0

    def bounded(_command: list[str], **kwargs: object) -> tuple[int, bytes, bytes]:
        assert kwargs["timeout_seconds"] == 19
        assert kwargs["stdout_limit"] == kwargs["stderr_limit"] == 100
        return 0, b"out", b"err"

    # Executable trust has separate native controls; this unit exercises the
    # shared budget and must work in the mapped test partition as well.
    monkeypatch.setattr(
        wrkslots, "_root_owned_executable",
        lambda path, _label: wrkslots._TrustedExecutablePath(path, ()),
    )
    monkeypatch.setattr(wrkslots, "_run_bounded_read_only_command", bounded)
    result = wrkslots._run_same_uid_command(
        Path("/usr/bin/printf"), ("out",), budget=budget,
        input_data=b"input",
    )
    assert result == (0, b"out", b"err")
    assert budget.deadline == deadline
    assert budget.stdout_remaining == budget.stderr_remaining == 97
    assert budget.input_remaining == 95


@pytest.mark.parametrize("delay_at", ["spawn", "wait"])
def test_bounded_read_only_command_refuses_late_success(
    monkeypatch: pytest.MonkeyPatch, delay_at: str
) -> None:
    clock = [0.0]
    original_killpg = os.killpg
    signals: list[tuple[int, int]] = []

    def killpg(pgid: int, signum: int) -> None:
        signals.append((pgid, signum))
        original_killpg(pgid, signum)

    class DelayedPopen(subprocess.Popen[bytes]):
        def __init__(
            self, args: Sequence[str], *, stdin: int, stdout: int, stderr: int,
            cwd: str, env: Mapping[str, str], start_new_session: bool,
        ) -> None:
            super().__init__(
                args, stdin=stdin, stdout=stdout, stderr=stderr,
                cwd=cwd, env=env, start_new_session=start_new_session,
            )
            if delay_at == "spawn":
                clock[0] = 2

        def wait(self, timeout: float | None = None) -> int:
            assert timeout is not None, "every wait must be bounded"
            result = super().wait(timeout=timeout)
            if delay_at == "wait":
                clock[0] = 2
            return result

    monkeypatch.setattr(subprocess, "Popen", DelayedPopen)
    monkeypatch.setattr(time, "monotonic", lambda: clock[0])
    monkeypatch.setattr(os, "killpg", killpg)
    with pytest.raises(wrkslots.Refusal, match="exceeded 1s"):
        wrkslots._run_bounded_read_only_command(
            ["/usr/bin/printf", "complete-looking output"], timeout_seconds=1
        )
    if delay_at == "wait":
        assert signals == [], "a reaped leader's PID/PGID must never be signalled"
    else:
        assert len(signals) == 1 and signals[0][1] == signal.SIGKILL


@pytest.mark.ordinary_environment
def test_bounded_read_only_command_discards_output_before_timeout(tmp_path: Path) -> None:
    ready = tmp_path / "ready"
    script = (
        "import os, pathlib, sys, time; "
        "pathlib.Path(sys.argv[1]).write_text(str(os.getpid())); "
        "os.write(1, b'p1234\\nfcwd\\nn/selected\\n'); time.sleep(60)"
    )
    with pytest.raises(wrkslots.Refusal, match="exceeded"):
        wrkslots._run_bounded_read_only_command(
            [sys.executable, "-c", script, str(ready)], timeout_seconds=1
        )
    pid = int(ready.read_text())
    assert not Path(f"/proc/{pid}").exists(), "direct child was not reaped"


@pytest.mark.ordinary_environment
def test_bounded_read_only_command_kills_descendant_holding_output(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    ready = tmp_path / "descendant"
    script = textwrap.dedent('''
        import os, pathlib, signal, sys, time
        if os.fork():
            sys.exit(0)
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        pathlib.Path(sys.argv[1]).write_text(str(os.getpid()))
        os.write(1, b"partial output")
        time.sleep(60)
    ''')
    original_killpg = os.killpg
    pidfds: list[int] = []

    def killpg(pgid: int, signum: int) -> None:
        try:
            assert signum == signal.SIGKILL
            assert not pidfds
            # Hold this exact descendant's identity before it is killed. A
            # later /proc read can race with exit or observe a reused PID.
            pidfds.append(os.pidfd_open(int(ready.read_text())))
        finally:
            original_killpg(pgid, signum)

    monkeypatch.setattr(os, "killpg", killpg)
    terminated = False
    try:
        with pytest.raises(wrkslots.Refusal, match="exceeded"):
            wrkslots._run_bounded_read_only_command(
                [sys.executable, "-c", script, str(ready)], timeout_seconds=1
            )
        assert len(pidfds) == 1
        assert select.select(pidfds, [], [], 2)[0] == pidfds, (
            "descendant survived process-group SIGKILL"
        )
        terminated = True
    finally:
        for pidfd in pidfds:
            try:
                if not terminated:
                    try:
                        signal.pidfd_send_signal(pidfd, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
            finally:
                os.close(pidfd)


@pytest.mark.ordinary_environment
def test_bounded_read_only_command_refuses_when_killed_child_cannot_be_reaped(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    original_wait = subprocess.Popen.wait
    children: list[subprocess.Popen[bytes]] = []

    def unreapable_wait(process: subprocess.Popen[bytes], timeout: float | None = None) -> int:
        assert timeout == 1, "post-kill wait must use the one-second bound"
        children.append(process)
        raise subprocess.TimeoutExpired(process.args, timeout)

    monkeypatch.setattr(subprocess.Popen, "wait", unreapable_wait)
    try:
        with pytest.raises(wrkslots.Refusal, match="was not reaped within 1s after SIGKILL"):
            wrkslots._run_bounded_read_only_command(
                ["/usr/bin/sleep", "60"], timeout_seconds=0.01
            )
        assert len(children) == 1
    finally:
        # This models an unreapable response, without inducing kernel D-state.
        for process in children:
            original_wait(process, timeout=5)


@pytest.mark.ordinary_environment
@pytest.mark.parametrize("interrupt", [False, True])
def test_bounded_read_only_command_cleans_up_post_spawn_setup_failure(
    monkeypatch: pytest.MonkeyPatch, interrupt: bool,
) -> None:
    failure = KeyboardInterrupt() if interrupt else RuntimeError("selector setup failed")
    original_wait = subprocess.Popen.wait
    children: list[subprocess.Popen[bytes]] = []

    def fail_selector() -> selectors.BaseSelector:
        raise failure

    def reap(process: subprocess.Popen[bytes], timeout: float | None = None) -> int:
        assert timeout == 1
        children.append(process)
        return original_wait(process, timeout=timeout)

    monkeypatch.setattr(selectors, "DefaultSelector", fail_selector)
    monkeypatch.setattr(subprocess.Popen, "wait", reap)
    with pytest.raises(type(failure)) as observed:
        wrkslots._run_bounded_read_only_command(["/usr/bin/sleep", "60"])
    assert observed.value is failure
    assert len(children) == 1
    assert children[0].returncode == -signal.SIGKILL


@pytest.mark.mapped_root_namespace
def test_root_owned_executable_rejects_namespace_root_without_host_root(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    if os.getuid() != 0 or wrkslots._has_identity_user_namespace():
        pytest.skip("requires a nonidentity user namespace with visible UID 0")
    helper = tmp_path / "attacker-owned-helper"
    helper.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
    helper.chmod(0o755)
    assert helper.stat().st_uid == 0
    assert not wrkslots._has_identity_user_namespace()
    monkeypatch.setattr(wrkslots, "_TRUSTED_EXECUTABLE_DIRECTORY", tmp_path)
    with pytest.raises(wrkslots.Refusal, match="not a root-owned"):
        wrkslots._root_owned_executable(helper, "attacker helper")


@pytest.mark.ordinary_environment
def test_root_owned_executable_accepts_host_root_helper() -> None:
    if not wrkslots._has_identity_user_namespace():
        pytest.skip("requires the initial identity user namespace")
    executable = wrkslots._root_owned_executable(Path("/usr/bin/find"), "find")
    assert executable.path == Path("/usr/bin/find").resolve(strict=True)
    wrkslots._recheck_trusted_executable(executable)


def test_root_owned_executable_rejects_nested_identity_map(tmp_path: Path) -> None:
    uid_map = tmp_path / "uid_map"
    uid_map.write_text("0 0 1\n", encoding="ascii")
    assert not wrkslots._has_identity_user_namespace(uid_map)


def test_root_owned_executable_refuses_effectively_writable_ancestor(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    executable = Path("/usr/bin/find")

    def identity(path: Path) -> tuple[Path, int, int, int, int, int]:
        mode = stat.S_IFREG | 0o555 if path == executable else stat.S_IFDIR | 0o555
        return path, 1, len(path.parts), mode, 0, 0

    def access(path: os.PathLike[str], mode: int, *, effective_ids: bool = False) -> bool:
        assert effective_ids
        if mode == os.W_OK:
            return Path(path) == Path("/usr")
        assert mode == os.X_OK
        return True

    monkeypatch.setattr(wrkslots, "_has_identity_user_namespace", lambda: True)
    monkeypatch.setattr(wrkslots, "_trusted_path_identity", identity)
    monkeypatch.setattr(os, "access", access)
    with pytest.raises(wrkslots.Refusal, match=r"component: /usr$"):
        wrkslots._root_owned_executable(executable, "find")


def test_trusted_executable_recheck_refuses_substitution(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    executable = Path("/usr/bin/find")
    expected = (executable, 1, 2, stat.S_IFREG | 0o555, 0, 0)
    authority = wrkslots._TrustedExecutablePath(executable, (expected,))
    monkeypatch.setattr(
        wrkslots,
        "_trusted_path_identity",
        lambda path: (path, 1, 3, stat.S_IFREG | 0o555, 0, 0),
    )
    with pytest.raises(wrkslots.Refusal, match="identity changed before execution"):
        wrkslots._recheck_trusted_executable(authority)


def test_same_uid_snapshot_selects_filesystem_uid(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    proc_root = tmp_path / "proc"
    pid_dir = proc_root / "123"
    pid_dir.mkdir(parents=True)
    filesystem_uid = os.getuid() + 1
    monkeypatch.setattr(
        wrkslots,
        "_read_process_stat",
        lambda _path: wrkslots._ProcessStat(start_ticks=17, flags=0),
    )
    monkeypatch.setattr(
        wrkslots, "_process_filesystem_uid", lambda _path: filesystem_uid
    )
    monkeypatch.setattr(
        wrkslots,
        "_read_process_cgroup",
        lambda _path: pytest.fail("same-UID final snapshot read unused cgroup evidence"),
    )
    monkeypatch.setattr(wrkslots, "_mount_namespace", lambda _path: "mnt:[123]")
    budget = wrkslots._ReadOnlyCommandBudget.start(
        timeout_seconds=30,
        stdout_limit=1024,
        stderr_limit=1024,
    )

    assert wrkslots._same_uid_process_observations(budget, proc_root) == ()
    filesystem_uid = os.getuid()
    observed = wrkslots._same_uid_process_observations(budget, proc_root)
    assert len(observed) == 1
    assert observed[0].pid == 123
    assert observed[0].start_ticks == 17


def test_same_uid_path_census_routes_protected_process_to_fresh_fallback(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    process = wrkslots._AbsentProcessObservation(123, 17, "/other", "mnt:[123]")
    target = Path("/absent/validate/gone")
    monkeypatch.setattr(
        os,
        "readlink",
        lambda _path: (_ for _ in ()).throw(PermissionError("procfs denied")),
    )
    monkeypatch.setattr(wrkslots, "_process_start_ticks", lambda _path: 17)
    monkeypatch.setattr(
        wrkslots,
        "_run_same_uid_command",
        lambda *_args, **_kwargs: pytest.fail(
            "known-protected proc entries reached the unprivileged command"
        ),
    )
    budget = wrkslots._ReadOnlyCommandBudget.start(
        timeout_seconds=30,
        stdout_limit=1024,
        stderr_limit=1024,
    )

    matches, fallback = wrkslots._same_uid_batched_path_matches(
        (process,), {target: str(target)}, budget
    )
    assert matches == ()
    assert fallback == (process,)


@pytest.mark.parametrize("fallback", ("clear", "in-use", "refused"))
def test_same_uid_census_resolves_protected_process_only_with_fresh_privilege(
    monkeypatch: pytest.MonkeyPatch,
    fallback: str,
) -> None:
    process = wrkslots._AbsentProcessObservation(123, 17, "/other", "mnt:[123]")
    target = Path("/absent/validate/gone")
    monkeypatch.setattr(
        wrkslots, "_same_uid_process_observations", lambda _budget: (process,)
    )
    monkeypatch.setattr(
        wrkslots,
        "_same_uid_batched_path_matches",
        lambda _processes, _targets, _budget: ((), (process,)),
    )
    monkeypatch.setattr(
        wrkslots, "_absent_validate_mount_matches", lambda *_args: ()
    )
    privileged_calls: list[str] = []

    def privileged_find(
        _processes: Sequence[wrkslots._AbsentProcessObservation],
        _targets: dict[Path, str],
        _budget: wrkslots._ReadOnlyCommandBudget,
    ) -> tuple[tuple[int, str, str, str], ...]:
        privileged_calls.append("find")
        if fallback == "refused":
            raise wrkslots.Refusal("fresh privileged fallback refused")
        if fallback == "in-use":
            return ((process.pid, str(target), "cwd", str(target)),)
        return ()

    monkeypatch.setattr(wrkslots, "_absent_validate_find_matches", privileged_find)

    def privileged_maps(*_args: object) -> tuple[()]:
        privileged_calls.append("maps")
        return ()

    monkeypatch.setattr(
        wrkslots,
        "_absent_validate_maps_matches",
        privileged_maps,
    )
    budget = wrkslots._ReadOnlyCommandBudget.start(
        timeout_seconds=30,
        stdout_limit=1024,
        stderr_limit=1024,
    )

    if fallback == "refused":
        with pytest.raises(wrkslots.Refusal, match="fresh privileged fallback refused"):
            wrkslots._capture_same_uid_process_path_census((target,), budget=budget)
        assert privileged_calls == ["find"]
        return
    census = wrkslots._capture_same_uid_process_path_census((target,), budget=budget)
    assert privileged_calls == ["find", "maps"]
    if fallback == "in-use":
        with pytest.raises(wrkslots.Refusal, match="live process 123"):
            census.assert_slot_unused(target, None)
    else:
        census.assert_slot_unused(target, None)


def test_same_uid_census_retries_share_one_batch_deadline(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monotonic_now = 0.0

    def monotonic() -> float:
        return monotonic_now

    monkeypatch.setattr(time, "monotonic", monotonic)
    budget = wrkslots._ReadOnlyCommandBudget.start(
        timeout_seconds=20,
        stdout_limit=1024,
        stderr_limit=1024,
    )
    calls = 0

    def changed(_budget: wrkslots._ReadOnlyCommandBudget) -> tuple[object, ...]:
        nonlocal calls, monotonic_now
        assert _budget is budget
        calls += 1
        monotonic_now += 11.0
        raise wrkslots._ProcessEvidenceChanged("generation changed")

    monkeypatch.setattr(wrkslots, "_same_uid_process_observations", changed)
    with pytest.raises(wrkslots.Refusal, match="operation-wide time bound"):
        wrkslots._capture_same_uid_process_path_census(
            (Path("/absent/validate/gone"),), budget=budget
        )
    assert calls == 2


def test_privileged_census_enforces_one_operation_wide_budget(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        wrkslots,
        "_root_owned_executable",
        lambda path, _label: wrkslots._TrustedExecutablePath(path, ()),
    )
    monkeypatch.setattr(
        wrkslots,
        "_run_bounded_read_only_command",
        lambda _command, **_kwargs: (0, b"abc", b""),
    )
    budget = wrkslots._ReadOnlyCommandBudget.start(
        timeout_seconds=30,
        stdout_limit=5,
        stderr_limit=64,
    )
    wrkslots._run_root_owned_command(Path("/usr/bin/find"), ("/proc/1/fd",), budget=budget)
    with pytest.raises(wrkslots.Refusal, match="operation-wide output bound"):
        wrkslots._run_root_owned_command(
            Path("/usr/bin/find"), ("/proc/2/fd",), budget=budget
        )


def test_process_census_retries_only_generation_changes(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    record = object.__new__(wrkslots.ActiveRecord)
    object.__setattr__(record, "slot", "gone")
    rows = ((record, (Path("/absent/gone"),)),)
    calls = 0

    def stable_refusal() -> tuple[wrkslots._AbsentProcessObservation, ...]:
        nonlocal calls
        calls += 1
        raise wrkslots.Refusal("permission denied")

    monkeypatch.setattr(wrkslots, "_absent_validate_process_snapshot", stable_refusal)
    with pytest.raises(wrkslots.Refusal, match="permission denied"):
        wrkslots._assert_absent_validate_processes_unrelated(rows)
    assert calls == 1

    calls = 0

    def changed() -> tuple[wrkslots._AbsentProcessObservation, ...]:
        nonlocal calls
        calls += 1
        raise wrkslots._ProcessEvidenceChanged("generation changed")

    monkeypatch.setattr(wrkslots, "_absent_validate_process_snapshot", changed)
    with pytest.raises(wrkslots.Refusal, match="three liveness attempts"):
        wrkslots._assert_absent_validate_processes_unrelated(rows)
    assert calls == 3


def test_bounded_read_only_command_uses_a_fixed_environment(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("PATH", "/tmp/shadowed-bin")
    monkeypatch.setenv("WRKSLOTS_UNTRUSTED", "must-not-cross-boundary")
    returncode, stdout, stderr = wrkslots._run_bounded_read_only_command(["/usr/bin/env"])
    assert returncode == 0
    assert stderr == b""
    assert set(stdout.decode("ascii").splitlines()) == {
        "LC_ALL=C",
        "PATH=/usr/sbin:/usr/bin:/sbin:/bin",
    }


def test_stale_row_for_another_slot_does_not_refuse_this_command(
    tmp_path: Path,
) -> None:
    """A row whose directory is gone must not refuse a command naming a different slot.

    Measured on devbig014 2026-09-03: 47 of 92 registered rows had lost their
    directories and 13 commands gated on the whole registry, so `wrkslots adopt
    demos-calib` died reporting `lander-3`. The reported name was simply the
    first record the loop reached, so repairing that one directory would have
    cleared the message and left the defect whole. The failure surfaced inside
    the new agent's operation while the cause belonged to a slot it had never
    touched.
    """
    project, _repository, _remote = make_project(tmp_path)
    first = create(project, slot="slot01", agent="codex-1", branch="codex/one")
    assert first.returncode == 0, first.stderr
    second = create(project, slot="slot02", agent="codex-2", branch="codex/two")
    assert second.returncode == 0, second.stderr

    shutil.rmtree(slots_directory(project) / "slot02")

    config = wrkslots._load_config(str(project), "testhost")
    state = wrkslots._load_active(config)

    findings = wrkslots._assert_registry_storage_consistent(
        config, [state], target_slot="slot01"
    )
    assert findings.stale == ("agent:slot02",), findings.stale


def test_stale_row_for_the_named_slot_STILL_REFUSES(tmp_path: Path) -> None:
    """The other direction: a command naming the broken slot must still refuse.

    Identical fixture to the test above with one thing changed -- which slot the
    command names -- so it isolates the scoping decision rather than re-testing
    that a missing directory is detected at all.
    """
    project, _repository, _remote = make_project(tmp_path)
    first = create(project, slot="slot01", agent="codex-1", branch="codex/one")
    assert first.returncode == 0, first.stderr
    second = create(project, slot="slot02", agent="codex-2", branch="codex/two")
    assert second.returncode == 0, second.stderr

    shutil.rmtree(slots_directory(project) / "slot02")

    config = wrkslots._load_config(str(project), "testhost")
    state = wrkslots._load_active(config)

    with pytest.raises(wrkslots.Refusal, match="slot directory is missing or unsafe"):
        wrkslots._assert_registry_storage_consistent(
            config, [state], target_slot="slot02"
        )


def test_stale_row_with_no_named_slot_still_refuses(tmp_path: Path) -> None:
    """`wrkslots recover` names no slot, and its behaviour is deliberately unchanged.

    Widening a recovery path was not measured here, so with no target every
    inconsistent row refuses exactly as before this scoping existed.
    """
    project, _repository, _remote = make_project(tmp_path)
    first = create(project, slot="slot01", agent="codex-1", branch="codex/one")
    assert first.returncode == 0, first.stderr
    second = create(project, slot="slot02", agent="codex-2", branch="codex/two")
    assert second.returncode == 0, second.stderr

    shutil.rmtree(slots_directory(project) / "slot02")

    config = wrkslots._load_config(str(project), "testhost")
    state = wrkslots._load_active(config)

    with pytest.raises(wrkslots.Refusal, match="slot directory is missing or unsafe"):
        wrkslots._assert_registry_storage_consistent(config, [state])


def _reference_glob_matches_path(pattern: str, path: str) -> bool:
    """The straightforward reading of the cache-glob contract, kept for comparison.

    It is deliberately the slow, obvious form: split both sides, walk them
    component by component, and let a ``**`` component consume any number of
    path components. The shipped matcher decides the same question with
    per-pattern work hoisted out of the per-path loop, so the two must agree on
    every input or the faster one has changed which paths count as cache.
    """

    pattern_parts = tuple(part for part in pattern.split("/") if part)
    path_parts = tuple(part for part in path.split("/") if part)

    def matches(pattern_index: int, path_index: int) -> bool:
        if pattern_index == len(pattern_parts):
            return path_index == len(path_parts)
        part = pattern_parts[pattern_index]
        if part == "**":
            return matches(pattern_index + 1, path_index) or (
                path_index < len(path_parts) and matches(pattern_index, path_index + 1)
            )
        return (
            path_index < len(path_parts)
            and fnmatch.fnmatchcase(path_parts[path_index], part)
            and matches(pattern_index + 1, path_index + 1)
        )

    return matches(0, 0)


def _reference_cache_glob_contains_path(pattern: str, path: str) -> bool:
    parts = tuple(part for part in path.split("/") if part)
    return any(
        _reference_glob_matches_path(pattern, "/".join(parts[:length]))
        for length in range(1, len(parts) + 1)
    )


def test_cache_glob_matching_accepts_exactly_the_documented_paths() -> None:
    # A cache glob names a directory, so every path INSIDE it is cache and the
    # sibling that merely starts with the same letters is not.
    assert wrkslots._cache_glob_contains_path("target", "target")
    assert wrkslots._cache_glob_contains_path("target", "target/debug/build/x.o")
    assert not wrkslots._cache_glob_contains_path("target", "targets/debug/x.o")
    assert not wrkslots._cache_glob_contains_path("target", "src/target.rs")

    # A multi-component glob matches at its own depth, not at every depth.
    assert wrkslots._cache_glob_contains_path("crates/target", "crates/target/a")
    assert not wrkslots._cache_glob_contains_path("crates/target", "target/a")
    assert not wrkslots._cache_glob_contains_path("crates/target", "a/crates/target")

    # '**' is the one form that may match at more than one depth.
    assert wrkslots._cache_glob_contains_path("crates/**/target", "crates/target/a")
    assert wrkslots._cache_glob_contains_path("crates/**/target", "crates/x/y/target/a")

    # A wildcard never crosses a slash.
    assert wrkslots._cache_glob_contains_path("tar*", "target/debug")
    assert not wrkslots._cache_glob_contains_path("tar*", "src/target/debug")
    assert not wrkslots._cache_glob_contains_path("t*t/debug", "tat/x/debug")

    # An empty glob selects nothing rather than everything.
    assert not wrkslots._cache_glob_contains_path("", "target/debug")
    assert not wrkslots._cache_glob_contains_path("", "")


def test_cache_glob_matching_agrees_with_the_reference_on_every_combination() -> None:
    patterns = [
        "/".join(combination)
        for length in range(0, 4)
        for combination in itertools.product(
            ("a", "target", "*", "?", "**", "a*", "[ab]", ""), repeat=length
        )
    ]
    paths = [
        "/".join(combination)
        for length in range(0, 5)
        for combination in itertools.product(("a", "b", "target", "ab"), repeat=length)
    ]
    assert len(patterns) * len(paths) > 150_000

    for pattern in patterns:
        for path in paths:
            assert wrkslots._cache_glob_contains_path(
                pattern, path
            ) == _reference_cache_glob_contains_path(pattern, path), (pattern, path)
            assert wrkslots._glob_matches_path(
                pattern, path
            ) == _reference_glob_matches_path(pattern, path), (pattern, path)


def _strand_git_registration(project: Path, repository: Path, slot: str) -> Path:
    """Leave a Git worktree registration inside the managed root with no active row.

    This is the `git-registration-without-row` condition: Git still lists the
    path, wrkslots has no row for it, and the directory is gone.
    """
    stranded = checkout(project, slot)
    git(
        repository,
        "worktree",
        "add",
        "-b",
        f"codex/stranded-{slot}",
        str(stranded),
        "origin/main",
    )
    assert stranded.is_dir()
    shutil.rmtree(stranded.parent)
    assert not stranded.exists()
    assert str(stranded) in git(repository, "worktree", "list", "--porcelain").stdout
    return stranded


def test_hold_ignores_an_unrelated_stranded_git_registration(tmp_path: Path) -> None:
    """One slot's stranded Git registration must not refuse another slot's hold.

    Five agents hit this from the outside: each asked to operate on its OWN slot,
    was refused by a message naming a slot it had never touched, and correctly
    declined to repair another lane's worktree. The refusal is retained for the
    named slot and reported by name for every other, so nothing is swept.
    """
    project, repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    assert (
        create(project, slot="slot02", agent="codex-2", branch="codex/two").returncode
        == 0
    )
    _strand_git_registration(project, repository, "stranded")

    held = command(project, "hold", "slot02", "--reason", "unrelated lane")

    assert held.returncode == 0, held.stderr
    assert "held slot=slot02" in held.stdout
    # Scoping must not hide it: the retained finding is named, not swallowed.
    assert "stranded" in held.stderr
    assert "RETAINED" in held.stderr


def test_hold_still_refuses_a_stranded_git_registration_for_its_own_slot(
    tmp_path: Path,
) -> None:
    """The other direction. Scoping that stopped refusing everything would be worse."""
    project, repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    assert (
        create(project, slot="slot02", agent="codex-2", branch="codex/two").returncode
        == 0
    )
    # Strand a registration underneath slot02's own managed path.
    stranded = checkout(project, "slot02") / "stale-checkout"
    git(
        repository,
        "worktree",
        "add",
        "-b",
        "codex/stranded-own",
        str(stranded),
        "origin/main",
    )
    shutil.rmtree(stranded)
    assert str(stranded) in git(repository, "worktree", "list", "--porcelain").stdout

    refused = command(project, "hold", "slot02", "--reason", "own slot")

    assert refused.returncode == 3, refused.stdout
    assert "no active checkout row for that repository names it" in refused.stderr
    assert "slot02" in refused.stderr


def test_registry_storage_guard_still_refuses_when_no_slot_is_named(
    tmp_path: Path,
) -> None:
    """With no target slot the guard keeps refusing, deliberately.

    `recover` is the only gated command naming no slot, and it refuses earlier
    for its own reasons, so this asserts the invariant where it lives rather
    than through a command that cannot reach it.
    """
    project, repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    _strand_git_registration(project, repository, "stranded")
    config = wrkslots._load_config(str(project), "testhost")
    states = [wrkslots._load_active(config)]

    with pytest.raises(wrkslots.StateError) as unscoped:
        wrkslots._assert_registry_storage_consistent(config, states)
    assert "no active checkout row for that repository names it" in str(
        unscoped.value
    )

    # ...and the same guard, told which slot the command names, does not.
    findings = wrkslots._assert_registry_storage_consistent(
        config, states, target_slot="slot01"
    )
    assert any("stranded" in value for value in findings.registrations)


def test_target_scoped_guard_still_refuses_unattributable_registration(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    config = wrkslots._load_config(str(project), "testhost")
    states = [wrkslots._load_active(config)]
    finding = wrkslots.StorageInconsistency(
        kind="git-registration-without-row",
        scope="directory",
        slot=None,
        slot_type="validate",
        machine=None,
        checkout=None,
        detail="unattributable registration",
        remedy="inspect the registration",
    )
    monkeypatch.setattr(
        wrkslots,
        "_registry_storage_inconsistencies",
        lambda *_args, **_kwargs: (finding,),
    )

    with pytest.raises(wrkslots.StateError, match="unattributable registration"):
        wrkslots._assert_registry_storage_consistent(
            config,
            states,
            target_slot="review-checkout",
            scope_to_target=True,
        )


def test_hold_ignores_an_unrelated_directory_without_a_row(tmp_path: Path) -> None:
    """The same scoping for a bare unregistered directory in the managed root."""
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    assert (
        create(project, slot="slot02", agent="codex-2", branch="codex/two").returncode
        == 0
    )
    (slots_directory(project) / "orphan").mkdir()

    held = command(project, "hold", "slot02", "--reason", "unrelated lane")

    assert held.returncode == 0, held.stderr
    assert "held slot=slot02" in held.stdout
    assert "orphan" in held.stderr
    assert "RETAINED" in held.stderr


def prepare_current_incomplete_frozen_checkout(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    *,
    nested: bool = False,
) -> tuple[Path, Path, Path, Path, Path]:
    project, repository, _remote = make_project(tmp_path, cache_globs=("ignored",))
    (repository / ".gitignore").write_text("ignored/\n", encoding="utf-8")
    git(repository, "add", ".gitignore")
    git(repository, "commit", "-m", "declare ignored output")
    if nested:
        add_recursive_submodules(tmp_path, project, repository)
    commit_validation_removal_schema(repository, schema_version=5)
    checkout, record, target = prepare_frozen_validation_checkout(
        project, repository, monkeypatch=monkeypatch, name="current-incomplete"
    )
    if nested:
        git(checkout, "-c", "protocol.file.allow=always", "submodule", "update", "--init", "--recursive")
    value = dict(wrkslots._strict_json_object(record.read_bytes(), "fixture record"))
    value.update({
        "service_result_schema": 5, "materialized_target": False,
        "state": "completed", "result": "no-result",
        "final_validate_status": "COULD_NOT_RUN", "exit_code": 75,
        "executed_nodes": 0, "executed_tests": None, "passed_tests": None,
        "selection_mode": None, "scorecard_writeback": None,
        "detail": "producer could not prepare validation",
    })
    value.pop("scorecard_handoff")
    value.pop("scorecard_writeback_files")
    record.write_text(json.dumps(value, sort_keys=True) + "\n", encoding="utf-8")
    sidecar = record.with_name(f"{record.stem}{wrkslots._VALIDATION_SERVICE_RESULT_SUFFIX}")
    sidecar.write_text(json.dumps({
        "schema_version": 5, "commit": target, "profile": "only-full",
        "final_validate_status": "COULD_NOT_RUN", "exit_code": 75,
        "executed_nodes": 0, "executed_tests": None, "passed_tests": None,
        "selection_mode": None, "scorecard_writeback": None,
        "detail": ["producer could not prepare validation"],
    }, sort_keys=True) + "\n", encoding="utf-8")
    stub_canonical_frozen_record_parser(monkeypatch, label="current incomplete fixture")
    stub_validate_batch_censuses(monkeypatch)
    return project, repository, checkout, record, sidecar


def run_current_incomplete_batch(
    project: Path, repository: Path, checkout: Path, record: Path, *, action: str
) -> int:
    authorization = (
        ["--coordinator-authorized", "--coordinator-pid", str(os.getpid())]
        if action == "recover" else []
    )
    return wrkslots.main([
        "--project-root", str(project), f"{action}-ownerless-validate-batch",
        *authorization, "--frozen-validate-checkout", str(checkout),
        "--completed-record", record.relative_to(project).as_posix(),
        "--repository", repository.relative_to(project).as_posix(),
        "--format", "json",
    ])


def incomplete_batch_row(report: Mapping[str, object], action: str) -> Mapping[str, object]:
    if action == "recover":
        assert report["removed"] == []
    rows = report["retained" if action == "recover" else "classifications"]
    assert isinstance(rows, list) and len(rows) == 1
    row = rows[0]
    assert isinstance(row, dict)
    return row


@pytest.mark.ordinary_environment
@pytest.mark.parametrize("action", ("recover", "classify"))
@pytest.mark.parametrize("advanced_source", (False, True))
def test_current_incomplete_frozen_retains_evidence_and_allows_stable_source_work(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str], action: str, advanced_source: bool,
) -> None:
    project, source, checkout, record, sidecar = prepare_current_incomplete_frozen_checkout(
        tmp_path, monkeypatch, nested=True
    )
    if advanced_source:
        (source / "seed.txt").write_text("later source commit\n")
        git(source, "commit", "-am", "advance surviving source")
        assert git(source, "rev-parse", "HEAD").stdout != git(checkout, "rev-parse", "HEAD").stdout
    (source / "HANDOFF.md").write_text("preserve active work\n")
    (source / "ignored").mkdir()
    (source / "ignored/report.txt").write_text("retained investigation\n")
    files = [record, sidecar, checkout / wrkslots._VALIDATION_SERVICE_RESULT_SCHEMA_PATH,
             source / "HANDOFF.md", source / "ignored/report.txt"]
    contents = {path: (path.read_bytes(), path.stat().st_mode) for path in files}
    clone_before, source_before = git_metadata_snapshot(checkout), git_metadata_snapshot(source)
    rc = run_current_incomplete_batch(project, source, checkout, record, action=action)
    assert rc == 0
    report = json.loads(capsys.readouterr().out)
    row = incomplete_batch_row(report, action)
    assert row["blocks_entry"] is False, row
    if action == "classify":
        assert row["state"] == "current-incomplete-retained"
    assert "no-result remain retained" in str(row["reason"])
    assert checkout.is_dir()
    assert git_metadata_snapshot(checkout) == clone_before
    assert git_metadata_snapshot(source) == source_before
    assert {path: (path.read_bytes(), path.stat().st_mode) for path in files} == contents
    config = wrkslots._load_config(str(project), "testhost")
    assert not wrkslots._journal_path(config).exists()
    assert not any(item["kind"] == "ownerless-validate-path-removed" for item in wrkslots._load_events(config))
    assert not (record.parent / "scorecard-handoffs").exists()
    assert json.loads(record.read_text())["result"] == "no-result"


@pytest.mark.ordinary_environment
@pytest.mark.parametrize("action", ("recover", "classify"))
@pytest.mark.parametrize(("field", "replacement"), (
    ("service_result_schema", 4), ("service_result_schema", True),
    ("materialized_target", True), ("qualifying_receipt", True),
    ("temporary_checkout", False), ("executed_nodes", 1), ("executed_nodes", False),
    ("executed_tests", 0), ("passed_tests", 0), ("selection_mode", "only"),
    ("scorecard_writeback", {"status": "completed"}),
    ("scorecard_handoff", None), ("scorecard_writeback_files", []),
    ("admission_result", {"state": "admitted"}), ("process_identity", None),
))
def test_current_incomplete_frozen_requires_exact_current_shape(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str], action: str, field: str, replacement: object,
) -> None:
    project, source, checkout, record, sidecar = prepare_current_incomplete_frozen_checkout(tmp_path, monkeypatch)
    value = dict(wrkslots._strict_json_object(record.read_bytes(), "fixture record"))
    value[field] = replacement
    record.write_text(json.dumps(value) + "\n")
    before = (record.read_bytes(), sidecar.read_bytes(), git_metadata_snapshot(checkout))
    assert run_current_incomplete_batch(project, source, checkout, record, action=action) == 0
    row = incomplete_batch_row(json.loads(capsys.readouterr().out), action)
    assert row["blocks_entry"] is True, row
    assert checkout.is_dir()
    assert (record.read_bytes(), sidecar.read_bytes(), git_metadata_snapshot(checkout)) == before


@pytest.mark.ordinary_environment
@pytest.mark.parametrize("action", ("recover", "classify"))
@pytest.mark.parametrize("relative", (".", "component", "component/leaf"))
@pytest.mark.parametrize("mutation", ("tracked", "staged", "untracked", "ignored", "handoff", "assume", "skip", "operation", "history"))
def test_current_incomplete_frozen_requires_pristine_recursive_checkout(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str], action: str, relative: str, mutation: str,
) -> None:
    project, source, checkout, record, _sidecar = prepare_current_incomplete_frozen_checkout(tmp_path, monkeypatch, nested=True)
    repository = checkout / relative
    tracked = {".": "seed.txt", "component": "component.txt", "component/leaf": "leaf.txt"}[relative]
    if mutation in {"tracked", "staged"}:
        (repository / tracked).write_text("authored modification\n")
        if mutation == "staged":
            git(repository, "add", tracked)
    elif mutation in {"untracked", "ignored", "handoff"}:
        name = "HANDOFF.md" if mutation == "handoff" else "new-file.txt"
        if mutation == "ignored":
            exclude = Path(git(repository, "rev-parse", "--path-format=absolute", "--git-path", "info/exclude").stdout.strip())
            with exclude.open("a") as stream:
                stream.write(f"\n{name}\n")
        (repository / name).write_text("authored file\n")
    elif mutation in {"assume", "skip"}:
        git(repository, "update-index", "--assume-unchanged" if mutation == "assume" else "--skip-worktree", tracked)
    else:
        name = "MERGE_HEAD" if mutation == "operation" else "info/grafts"
        path = Path(git(repository, "rev-parse", "--path-format=absolute", "--git-path", name).stdout.strip())
        path.write_text(git(repository, "rev-parse", "HEAD").stdout)
    # Even a configured submodule-ignore rule must not hide a nested violation.
    git(checkout, "config", "submodule.component.ignore", "all")
    before = git_metadata_snapshot(checkout)
    assert run_current_incomplete_batch(project, source, checkout, record, action=action) == 0
    row = incomplete_batch_row(json.loads(capsys.readouterr().out), action)
    assert row["blocks_entry"] is True, row
    assert checkout.is_dir()
    assert git_metadata_snapshot(checkout) == before


@pytest.mark.ordinary_environment
@pytest.mark.parametrize("action", ("recover", "classify"))
@pytest.mark.parametrize("mutation", ("sidecar", "sidecar-replace", "schema", "record", "child-index", "child-file", "child-head", "source-head", "source-index", "source-remote"))
def test_current_incomplete_frozen_rechecks_after_fresh_census(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str], action: str, mutation: str,
) -> None:
    project, source, checkout, record, sidecar = prepare_current_incomplete_frozen_checkout(tmp_path, monkeypatch, nested=True)
    calls = 0
    def fresh(_paths: Sequence[Path], *, budget: wrkslots._ReadOnlyCommandBudget) -> wrkslots._ProcessPathCensus:
        nonlocal calls
        calls += 1
        budget.remaining_seconds()
        if mutation in {"sidecar", "schema", "record"}:
            path = {"sidecar": sidecar, "schema": checkout / wrkslots._VALIDATION_SERVICE_RESULT_SCHEMA_PATH, "record": record}[mutation]
            with path.open("a") as stream:
                stream.write(" ")
        elif mutation == "sidecar-replace":
            replacement = sidecar.with_suffix(".replacement")
            replacement.write_bytes(sidecar.read_bytes())
            replacement.replace(sidecar)
        elif mutation == "child-head":
            git(checkout / "component/leaf", "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "--allow-empty", "-m", "child advanced during census")
        elif mutation == "child-file":
            (checkout / "component/leaf/new-file.txt").write_text("changed after scan\n")
        elif mutation == "source-head":
            git(source, "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "--allow-empty", "-m", "source advanced during census")
        elif mutation == "source-remote":
            git(source, "remote", "set-url", "origin", str(tmp_path / "changed.git"))
        else:
            repo, tracked = (source, "seed.txt") if mutation == "source-index" else (checkout / "component/leaf", "leaf.txt")
            git(repo, "update-index", "--assume-unchanged", tracked)
        return wrkslots._ProcessPathCensus((), ())
    monkeypatch.setattr(wrkslots, "_capture_same_uid_process_path_census", fresh)
    assert run_current_incomplete_batch(project, source, checkout, record, action=action) == 0
    row = incomplete_batch_row(json.loads(capsys.readouterr().out), action)
    assert row["blocks_entry"] is True, row
    assert calls == 1
    assert checkout.is_dir()
    assert not wrkslots._journal_path(wrkslots._load_config(str(project), "testhost")).exists()


@pytest.mark.ordinary_environment
@pytest.mark.parametrize("action", ("recover", "classify"))
@pytest.mark.parametrize("mutation", ("sidecar-missing", "sidecar-mismatch", "sidecar-duplicate", "live-generation", "unknown-generation", "live-path", "live-mount", "unknown-census", "wrong-source", "wrong-target", "wrong-origin"))
def test_current_incomplete_frozen_refuses_untrusted_or_live_evidence(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str], action: str, mutation: str,
) -> None:
    project, source, checkout, record, sidecar = prepare_current_incomplete_frozen_checkout(tmp_path, monkeypatch)
    if mutation == "sidecar-missing":
        sidecar.unlink()
    elif mutation in {"sidecar-mismatch", "sidecar-duplicate"}:
        if mutation == "sidecar-duplicate":
            sidecar.write_text(sidecar.read_text().replace('"exit_code": 75', '"exit_code": 75, "exit_code": 75'))
        else:
            value = dict(wrkslots._strict_json_object(sidecar.read_bytes(), "fixture sidecar"))
            value["detail"] = ["contradictory retained result"]
            sidecar.write_text(json.dumps(value))
    elif mutation in {"live-generation", "unknown-generation", "wrong-source", "wrong-target"}:
        value = dict(wrkslots._strict_json_object(record.read_bytes(), "fixture record"))
        if mutation == "live-generation":
            identity = wrkslots._read_process_identity(os.getpid())
            value["process_identity"] = {"pid": identity.pid, "start_ticks": identity.start_ticks, "boot_id": identity.boot_id}
        elif mutation == "unknown-generation":
            value["process_identity"] = {"pid": 123, "start_ticks": True, "boot_id": "unknown"}
        else:
            value["source_checkout" if mutation == "wrong-source" else "target"] = str(tmp_path / "wrong") if mutation == "wrong-source" else "0" * 40
        record.write_text(json.dumps(value))
    elif mutation == "wrong-origin":
        git(checkout, "remote", "set-url", "origin", str(tmp_path / "other.git"))
    elif mutation == "unknown-census":
        def unknown(_paths: Sequence[Path], **_kwargs: object) -> wrkslots._ProcessPathCensus:
            raise wrkslots.Refusal("test process census is incomplete")
        monkeypatch.setattr(wrkslots, "_capture_lsof_process_path_census", unknown)
    else:
        kind = "mount" if mutation == "live-mount" else "cwd"
        stub_validate_batch_censuses(monkeypatch, lambda _paths: wrkslots._ProcessPathCensus((), ((os.getpid(), str(checkout), kind, str(checkout)),)))
    assert run_current_incomplete_batch(project, source, checkout, record, action=action) == 0
    row = incomplete_batch_row(json.loads(capsys.readouterr().out), action)
    assert row["blocks_entry"] is True, row
    assert checkout.is_dir()
    assert not wrkslots._journal_path(wrkslots._load_config(str(project), "testhost")).exists()


@pytest.mark.ordinary_environment
def test_current_incomplete_frozen_retention_does_not_authorize_direct_recovery(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str],
) -> None:
    project, source, checkout, record, sidecar = prepare_current_incomplete_frozen_checkout(tmp_path, monkeypatch)
    before = (record.read_bytes(), sidecar.read_bytes(), git_metadata_snapshot(checkout))
    rc = wrkslots.main([
        "--project-root", str(project), "recover", "--coordinator-authorized",
        "--coordinator-pid", str(os.getpid()), "--frozen-validate-checkout", str(checkout),
        "--completed-record", record.relative_to(project).as_posix(),
        "--repository", source.relative_to(project).as_posix(),
    ])
    assert rc == 3
    assert "requires --validation-proof-manifest" in capsys.readouterr().err
    assert checkout.is_dir()
    assert (record.read_bytes(), sidecar.read_bytes(), git_metadata_snapshot(checkout)) == before


@pytest.mark.ordinary_environment
@pytest.mark.parametrize("action", ("recover", "classify"))
@pytest.mark.parametrize("mutation", ("changed-gitlink", "uninitialized-files", "ignored-cache", "replaced-clone"))
def test_current_incomplete_frozen_binds_checkout_and_gitlink_identity(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str], action: str, mutation: str,
) -> None:
    project, source, checkout, record, sidecar = prepare_current_incomplete_frozen_checkout(
        tmp_path, monkeypatch, nested=mutation != "uninitialized-files"
    )
    if mutation == "changed-gitlink":
        git(checkout / "component/leaf", "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "--allow-empty", "-m", "different child commit")
        git(checkout, "config", "submodule.component.ignore", "all")
    elif mutation == "uninitialized-files":
        # Advance the fixture target to a commit with a real gitlink, without
        # initializing that submodule. Hidden content must not count as an
        # empty uninitialized gitlink.
        add_recursive_submodules(tmp_path, project, source)
        target = git(source, "rev-parse", "HEAD").stdout.strip()
        git(checkout, "fetch", str(source), target)
        git(checkout, "checkout", "--detach", target)
        value = dict(wrkslots._strict_json_object(record.read_bytes(), "fixture record"))
        value["target"] = target
        record.write_text(json.dumps(value))
        result = dict(wrkslots._strict_json_object(sidecar.read_bytes(), "fixture result"))
        result["commit"] = target
        sidecar.write_text(json.dumps(result))
        (checkout / "component/authored.txt").write_text("must remain retained\n")
    elif mutation == "ignored-cache":
        (checkout / "ignored").mkdir()
        (checkout / "ignored/result.txt").write_text("never discard retained output\n")
    else:
        calls = 0
        def fresh(_paths: Sequence[Path], *, budget: wrkslots._ReadOnlyCommandBudget) -> wrkslots._ProcessPathCensus:
            nonlocal calls
            calls += 1
            budget.remaining_seconds()
            preserved = checkout.with_name(checkout.name + "-preserved")
            checkout.rename(preserved)
            shutil.copytree(preserved, checkout, symlinks=True)
            return wrkslots._ProcessPathCensus((), ())
        monkeypatch.setattr(wrkslots, "_capture_same_uid_process_path_census", fresh)
    before = (record.read_bytes(), sidecar.read_bytes())
    assert run_current_incomplete_batch(project, source, checkout, record, action=action) == 0
    row = incomplete_batch_row(json.loads(capsys.readouterr().out), action)
    assert row["blocks_entry"] is True, row
    assert checkout.is_dir()
    assert (record.read_bytes(), sidecar.read_bytes()) == before
    if mutation == "replaced-clone":
        assert calls == 1
        assert checkout.with_name(checkout.name + "-preserved").is_dir()


@pytest.mark.ordinary_environment
@pytest.mark.parametrize("action", ("recover", "classify"))
@pytest.mark.parametrize("mutation", ("root-head", "both-origins"))
def test_current_incomplete_frozen_binds_initial_facts_to_recursive_snapshot(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str], action: str, mutation: str,
) -> None:
    project, source, checkout, record, _sidecar = prepare_current_incomplete_frozen_checkout(
        tmp_path, monkeypatch
    )
    original = wrkslots._retained_validation_git_state
    changed = False
    def observe(
        repository: Path, *, pristine: bool, cache_globs: Sequence[str] = (),
    ) -> wrkslots._RetainedValidationGitState:
        nonlocal changed
        if repository == checkout and not changed:
            changed = True
            if mutation == "root-head":
                git(checkout, "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "--allow-empty", "-m", "changed before first snapshot")
            else:
                for path in (checkout, source):
                    git(path, "remote", "set-url", "origin", str(tmp_path / "changed.git"))
        return original(repository, pristine=pristine, cache_globs=cache_globs)
    monkeypatch.setattr(wrkslots, "_retained_validation_git_state", observe)
    assert run_current_incomplete_batch(project, source, checkout, record, action=action) == 0
    row = incomplete_batch_row(json.loads(capsys.readouterr().out), action)
    assert changed
    assert row["blocks_entry"] is True, row
    assert checkout.is_dir()
