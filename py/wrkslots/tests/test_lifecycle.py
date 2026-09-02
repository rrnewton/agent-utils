"""Focused safety and lifecycle tests for the wrkslots distribution."""

from __future__ import annotations

import hashlib
import datetime as dt
import json
import os
import signal
import shlex
import shutil
import subprocess
import sys
import time
from collections.abc import Iterator
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
    project: Path, machine: str = "testhost", *, expire: bool = True
) -> None:
    config = wrkslots._load_config(str(project), machine)
    state = wrkslots._load_active(config)
    record = state.slots[0]
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
) -> tuple[Path, Path]:
    checkout_path = project / "ignored" / "validate-fresh-legacy"
    checkout_path.parent.mkdir(parents=True, exist_ok=True)
    git(repository, "worktree", "add", "--detach", str(checkout_path), "HEAD")
    record_path = project / "ignored" / "validate" / "runs" / "validate-legacy.json"
    record_path.parent.mkdir(parents=True, exist_ok=True)
    record_path.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "unit": "validate-legacy.service",
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
    assert "worktree creation is coordinator-owned" in refused.stderr
    assert "no worktree or lifecycle record was changed" in refused.stderr
    assert "ask the coordinator" in refused.stderr
    assert refused.stdout == ""
    assert not checkout(project).exists()
    assert active_slots(project) == []
    assert sorted(path.read_bytes() for path in events.glob("*.json")) == before


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


def test_recover_resumes_literal_preupgrade_legacy_journal_without_new_authority(
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

    recovered = raw_command(
        project, "recover", "--coordinator-pid", str(os.getpid())
    )

    assert recovered.returncode == 0, recovered.stderr
    assert not checkout_path.exists()
    assert checkout_path.absolute() not in wrkslots._GitVcs().listed_worktrees(repository)


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
    assert "remote changed during recovery fetch" in capsys.readouterr().err
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
    recovered = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-cargo-home",
        target.relative_to(project).as_posix(),
        "--completed-record",
        record.relative_to(project).as_posix(),
    )
    assert recovered.returncode == 0, recovered.stderr
    assert not target.exists()
    assert active_slots(project) == []


def test_ownerless_cargo_home_allows_nested_git_cache_and_records_determination(
    tmp_path: Path,
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

    recovered = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-cargo-home",
        target.relative_to(project).as_posix(),
        "--recovery-note",
        "the historical run handle is absent",
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
    tmp_path: Path, state: str
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

    recovered = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-cargo-home",
        target.relative_to(project).as_posix(),
        "--completed-record",
        record.relative_to(project).as_posix(),
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


def test_ownerless_cleanup_accepts_current_pass_with_failed_writeback(
    tmp_path: Path,
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

    recovered = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-cargo-home",
        target.relative_to(project).as_posix(),
        "--completed-record",
        record.relative_to(project).as_posix(),
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
    interrupted = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-cargo-home",
        target.relative_to(project).as_posix(),
        "--completed-record",
        record.relative_to(project).as_posix(),
        env={
            "WRKSLOTS_TEST_INTERRUPT": "after-ownerless-validate-path-fence-before-journal"
        },
    )
    assert interrupted.returncode == 86
    journal = json.loads(
        (control_directory(project) / "ACTIVE.testhost.journal").read_text()
    )
    fenced = project / journal["fenced"]
    assert not target.exists()
    assert fenced.is_dir()

    resumed = raw_command(project, "recover", "--coordinator-pid", str(os.getpid()))

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

    interrupted = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-cargo-home",
        target.relative_to(project).as_posix(),
        "--completed-record",
        record.relative_to(project).as_posix(),
        env={"WRKSLOTS_TEST_INTERRUPT": "after-ownerless-validate-remove"},
    )
    assert interrupted.returncode == 86
    assert not target.exists()
    journal = control_directory(project) / "ACTIVE.testhost.journal"
    assert journal.is_file()
    # The durable authorization already captured the record digest before the
    # path was deleted.  Bookkeeping recovery must not become impossible if a
    # later retention pass removes that external record.
    record.unlink()

    resumed = raw_command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
    )
    assert resumed.returncode == 0, resumed.stderr
    assert not journal.exists()


def test_ownerless_recovery_keeps_unrelated_drift_visible_in_status(
    tmp_path: Path,
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
    recovered = raw_command(
        project,
        "--allow-existing-unregistered-worktrees",
        "recover",
        "--coordinator-authorized",
        "--coordinator-pid",
        str(os.getpid()),
        "--ownerless-validate-cargo-home",
        target.relative_to(project).as_posix(),
        "--recovery-note",
        "no retained run handle",
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

    gated = raw_command(project, "audit", "--gate")
    assert gated.returncode == 1
    assert "state=actionable" in gated.stdout
    assert (
        "summary=1 worktree slot(s) need coordinator attention: historical"
        in gated.stdout
    )
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
    journal = project / "worktrees" / "ACTIVE.testhost.journal"
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


def test_interrupted_create_requires_and_supports_recovery(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)
    interrupted = create(
        project,
        env={"WRKSLOTS_TEST_INTERRUPT": "after-create-worktree"},
    )
    assert interrupted.returncode == 86
    assert checkout(project).is_dir()
    assert not active_slots(project)
    assert (project / "worktrees" / "ACTIVE.testhost.journal").is_file()
    refused = create(
        project,
        slot="slot02",
        agent="codex-2",
        branch="codex/other",
    )
    assert refused.returncode == 3
    assert "recover" in refused.stderr

    recovered = command(
        project, "recover", "--coordinator-pid", str(os.getpid())
    )

    assert recovered.returncode == 0, recovered.stderr
    assert len(active_slots(project)) == 1
    assert not (project / "worktrees" / "ACTIVE.testhost.journal").exists()


def test_create_refuses_owner_generation_change_before_publication(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, _repository, _remote = make_project(tmp_path)
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
    monkeypatch.setattr(wrkslots, "_assert_caller_process", lambda _identity, _label: None)
    monkeypatch.setattr(wrkslots, "_read_process_identity", lambda _pid: reused)
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
    assert (project / "worktrees" / "ACTIVE.testhost.journal").is_file()


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
    assert reread.returncode == 0, reread.stderr
    removed = remove(project)
    assert removed.returncode == 0, removed.stderr
    assert not checkout(project).exists()


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


def test_agent_reclaim_pushes_unpushed_commits_and_uncommitted_files(
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
        "ignored.txt",
        "local.txt",
        "seed.txt",
        "untracked.txt",
    } <= names
    assert "target/build-output" not in names
    assert git(remote, "show", f"{salvage_commit}:seed.txt").stdout == "dirty tracked file\n"
    events = wrkslots._load_events(wrkslots._load_config(str(project), "testhost"))
    started = [event for event in events if event["kind"] == "reclaim-started"]
    assert len(started) == 1
    payload = started[0]["payload"]
    assert isinstance(payload, dict)
    assert payload["coordinator_authorized"] is False


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


def test_validate_batch_shares_one_census_and_retains_each_refusal(
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
                replace(record, owner=replace(record.owner, boot_id="finished-boot")),
            ),
            action="test-owner-exited",
            slot=slot,
        )
    set_liveness(project, "dead")
    in_use = checkout(project, slot="in-use", slot_type="validate").parent
    calls = 0

    def census(paths: list[Path]) -> wrkslots._ProcessPathCensus:
        nonlocal calls
        calls += 1
        assert set(paths) == {
            checkout(project, slot="unused", slot_type="validate").parent,
            in_use,
        }
        return wrkslots._ProcessPathCensus(
            (), ((12345, str(in_use), "link", str(in_use)),)
        )

    monkeypatch.setattr(wrkslots, "_capture_process_path_census", census)

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
            "--format",
            "json",
        ]
    )

    assert rc == 0
    report = json.loads(capsys.readouterr().out)
    assert report["process_censuses"] == 1
    assert report["removed"] == [{"generation": 1, "slot": "unused"}]
    assert len(report["retained"]) == 1
    assert report["retained"][0]["slot"] == "in-use"
    assert "live process 12345" in report["retained"][0]["reason"]
    assert calls == 1
    assert not checkout(project, slot="unused", slot_type="validate").exists()
    assert in_use.is_dir()


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


def test_validate_batch_rechecks_use_that_starts_after_shared_census(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project, slot="late-use", slot_type="validate", branch=None)
    assert made.returncode == 0, made.stderr
    mark_owner_dead(project)
    set_liveness(project, "dead")
    tree = checkout(project, slot="late-use", slot_type="validate")
    monkeypatch.setattr(
        wrkslots,
        "_capture_process_path_census",
        lambda _paths: wrkslots._ProcessPathCensus((), ()),
    )
    real_assert_unused = wrkslots._assert_slot_unused
    late_user: subprocess.Popen[str] | None = None

    def enter_after_census(
        slot_path: Path,
        record: wrkslots.ActiveRecord | None = None,
        *,
        use_lsof: bool = True,
        ignore_invoking_ancestry: bool = False,
        census: wrkslots._ProcessPathCensus | None = None,
        fallback_census: wrkslots._ProcessPathCensus | None = None,
    ) -> None:
        nonlocal late_user
        real_assert_unused(
            slot_path,
            record,
            use_lsof=use_lsof,
            ignore_invoking_ancestry=ignore_invoking_ancestry,
            census=census,
            fallback_census=fallback_census,
        )
        if census is not None and late_user is None:
            late_user = subprocess.Popen(["sleep", "60"], cwd=tree, text=True)

    monkeypatch.setattr(wrkslots, "_assert_slot_unused", enter_after_census)
    try:
        rc = wrkslots.main(
            [
                "--project-root",
                str(project),
                "remove-validate-batch",
                "--coordinator-pid",
                str(os.getpid()),
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
    assert report["removed"] == []
    assert len(report["retained"]) == 1
    assert "live process" in report["retained"][0]["reason"]
    assert tree.is_dir()


def test_agent_remove_still_refuses_dead_owner_cgroup_with_live_process(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    tree = checkout(project)
    commit_task(repository, tree, "codex/task")
    handed_off = finish(project)
    assert handed_off.returncode == 0, handed_off.stderr
    mark_owner_dead_in_current_cgroup(project)
    set_liveness(project, "dead")

    refused = remove(project)

    assert refused.returncode == 3
    assert "remains in recorded owner cgroup" in refused.stderr
    assert tree.is_dir()
    assert active_slots(project)


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
    assert len(read_events) == 1
    payload = read_events[0]["payload"]
    assert isinstance(payload, dict)
    assert payload["sha256"] == hashlib.sha256(
        b"Continue from the exact recorded commit.\n"
    ).hexdigest()


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
    assert len(read_events) == 1
    payload = wrkslots._as_mapping(read_events[0]["payload"], "handoff read payload")
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
    assert len(read_events) == 1
    payload = wrkslots._as_mapping(read_events[0]["payload"], "handoff read payload")
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
    assert [event["kind"] for event in events] == ["state-imported", "handoff-read"]
    payload = wrkslots._as_mapping(events[1]["payload"], "handoff read payload")
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


def test_remove_refuses_an_unbound_owner_with_no_coordinator_recovery_evidence(
    tmp_path: Path,
) -> None:
    """A row with neither an owner lease nor recovery evidence has no authority to delete.

    Slots left unbound by an older allocation path are exactly the 31 that became
    unreclaimable. Reclaiming them must go through coordinator recovery, which
    records why the historical owner is gone; a bare unbound row is refused.
    """
    project, _repository, _remote = make_project(tmp_path)
    made = create(project, bind_owner=False)
    assert made.returncode == 0, made.stderr
    set_liveness(project, "dead")
    expire_heartbeat(project)

    refused = remove(project)

    assert refused.returncode == 3
    assert "owner death is unknown" in refused.stderr
    assert checkout(project).is_dir()
    assert len(active_slots(project)) == 1


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


def test_unbound_owner_remains_unreclaimable_after_recovery_note(
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
    assert removed.returncode == 3
    assert "owner death is unknown" in removed.stderr
    assert checkout(project).is_dir()


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
    journal_path = control_directory(project) / "ACTIVE.testhost.journal"
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
    original = project / "worktrees" / "ACTIVE.testhost.journal"
    changed = project / "worktrees" / "ACTIVE.machine-b.journal"
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
    journal_path = project / "worktrees" / "ACTIVE.testhost.journal"
    journal = json.loads(journal_path.read_text(encoding="utf-8"))
    journal["planned"][0]["destination"] = "repo/redirected"
    journal_path.write_text(json.dumps(journal, indent=2) + "\n", encoding="utf-8")

    refused = command(
        project, "recover", "--coordinator-pid", str(os.getpid())
    )

    assert refused.returncode == 3
    assert "does not match slot" in refused.stderr
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
    assert "repository does not exist or cannot be resolved" in refused.stderr
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

    before_events = tuple(
        path.read_bytes()
        for path in sorted((project / "worktrees" / "EVENTS.testhost").glob("*.json"))
    )
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
    assert "Git common directory" in heartbeat.stderr
    assert "no active checkout row" in heartbeat.stderr
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
    assert "interrupted mutation recorded" in refused.stderr
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
    removed_cache_paths: list[Path] = []
    original_remove_cache_directory = wrkslots._remove_cache_directory

    def record_cache_removal(
        config: wrkslots.Config, cache_directory: wrkslots.CacheDirectory
    ) -> int:
        removed_cache_paths.append(cache_directory.path)
        return original_remove_cache_directory(config, cache_directory)

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
    assert len(removed_cache_paths) == 1
    assert removed_cache_paths[0].name == "target"
    assert any(part.startswith(".slot01.fenced.") for part in removed_cache_paths[0].parts)
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
    journal = control_directory(project) / "ACTIVE.testhost.journal"
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
    journal = control_directory(project) / "ACTIVE.testhost.journal"
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
    journal = control_directory(project) / "ACTIVE.testhost.journal"
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
    assert "summary=1 worktree slot(s) need coordinator attention: slot01" in captured.out
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
    assert "slot directory does not match its record" in capsys.readouterr().err
    assert any(
        isinstance(row, dict) and row.get("slot") == "target"
        for row in active_slots(project)
    )


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
        "working-tree contents were absent before recovery; uncommitted, untracked, "
        "ignored, and HANDOFF contents could not be inspected or salvaged"
    ]
    rescue_ref = wrkslots._absent_agent_rescue_ref(record, record.checkouts[0])
    assert git(remote, "rev-parse", rescue_ref).stdout.strip() == record.checkouts[0].head
    path = wrkslots._stored_path(config, record.checkouts[0].path, "test checkout")
    assert wrkslots._GitVcs().worktree_registration(repository, path) is None
    assert run_absent_agent_recovery(project, record, apply=True) == 0


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
    monkeypatch.setattr(wrkslots, "_read_process_cgroup", lambda _path: "/other")
    monkeypatch.setattr(wrkslots, "_mount_namespace", lambda _path: "mnt:[123]")
    snapshot = wrkslots._absent_validate_process_snapshot(proc_root)
    assert snapshot == (wrkslots._AbsentProcessObservation(123, 17, "/other", "mnt:[123]"),)

    monkeypatch.setattr(
        wrkslots, "_mountinfo_path_references", lambda _pid, _budget: ()
    )
    shared_launcher_cgroup = wrkslots._absent_validate_mount_match(
        (wrkslots._AbsentProcessObservation(123, 17, "/recorded/owner/child", "mnt:[123]"),),
        {target: "gone"},
    )
    assert shared_launcher_cgroup is None

    monkeypatch.setattr(wrkslots, "_root_owned_executable", lambda path, _label: path)
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
        pid: int, _budget: wrkslots._ReadOnlyCommandBudget
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


def test_mountinfo_census_enforces_file_aggregate_and_deadline_bounds(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    observed_limits: list[int] = []

    def bounded_read(_path: Path, _label: str, limit: int) -> bytes:
        observed_limits.append(limit)
        return b"abc"

    monkeypatch.setattr(wrkslots, "_read_bounded_regular_file", bounded_read)
    monkeypatch.setattr(wrkslots, "_parse_mountinfo_paths", lambda _text, _label: ())
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
        lambda path, _label: path,
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


def test_privileged_census_enforces_one_operation_wide_budget(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(wrkslots, "_root_owned_executable", lambda path, _label: path)
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
