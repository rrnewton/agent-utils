"""Focused safety and lifecycle tests for the wrkslots distribution."""

from __future__ import annotations

import json
import os
import shlex
import subprocess
import sys
from pathlib import Path

import pytest


PACKAGE_ROOT = Path(__file__).resolve().parents[1]
PY_ROOT = PACKAGE_ROOT.parent
WRKSLOTS = PACKAGE_ROOT / "__main__.py"
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


def create(
    project: Path,
    *,
    slot: str = "slot01",
    agent: str = "codex-1",
    branch: str = "codex/task",
    repository_name: str = "repo",
    checkout_name: str = "product",
    machine: str | None = None,
    owner_pid: int | None = None,
    bind_owner: bool = True,
    env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    pid = os.getpid() if owner_pid is None else owner_pid
    owner_args = ["--owner-pid", str(pid)] if bind_owner else []
    return command(
        project,
        "create",
        slot,
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
        "--branch",
        f"{checkout_name}={branch}",
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


def checkout(project: Path, slot: str = "slot01", name: str = "product") -> Path:
    slots = slots_directory(project)
    if configuration(project).get("layout", "nested") == "flat":
        return slots / slot
    return slots / slot / name


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


def mark_owner_dead(project: Path, machine: str = "testhost") -> None:
    state_path = control_directory(project) / f"ACTIVE.{machine}.json"
    state = json.loads(state_path.read_text(encoding="utf-8"))
    state["slots"][0]["owner"]["boot_id"] = "finished-boot"
    state["slots"][0]["owner"]["cgroup_path"] = "/wrkslots-test-finished"
    state_path.write_text(json.dumps(state, indent=2) + "\n", encoding="utf-8")


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
        assert (project / "worktrees" / "ACTIVE.testhost.json").read_bytes() == before
    finally:
        holder.terminate()
        holder.wait(timeout=10)


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
        slot_path: Path, record: wrkslots.ActiveRecord | None = None
    ) -> None:
        nonlocal calls, entrant
        original_assert(slot_path, record)
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
            entrant.terminate()
            entrant.wait(timeout=10)


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
    refused = command(project, "recover", "--coordinator-pid", str(os.getpid()))
    assert refused.returncode == 3
    assert "active on testhost and archived on testhost" in refused.stderr
    assert len(active_slots(project)) == 1
    assert (project / "worktrees" / "ACTIVE.testhost.journal").is_file()
    archive_path.write_text(original_archive, encoding="utf-8")

    recovered = command(project, "recover", "--coordinator-pid", str(os.getpid()))

    assert recovered.returncode == 0, recovered.stderr
    assert active_slots(project) == []
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
    assert "changed after finish was prepared" in refused.stderr
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


def test_forged_local_remote_tracking_refs_do_not_prove_publication(tmp_path: Path) -> None:
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
    assert "differs from configured authority" in refused.stderr
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


def test_create_defaults_to_origin_and_accepts_a_configured_remote_name(tmp_path: Path) -> None:
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


def test_finish_refuses_unfinished_git_operation_without_deleting(tmp_path: Path) -> None:
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
    if owner == "other-machine":
        state_path = project / "worktrees" / "ACTIVE.testhost.json"
        state = json.loads(state_path.read_text(encoding="utf-8"))
        state["slots"][0]["owner"]["host_id"] = "a-different-machine"
        state_path.write_text(json.dumps(state, indent=2) + "\n", encoding="utf-8")
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
    # Strip the recovery evidence, leaving the handoff: a row an older build could write.
    state_path = project / "worktrees" / "ACTIVE.testhost.json"
    state = json.loads(state_path.read_text(encoding="utf-8"))
    assert state["slots"][0]["owner"] is None
    state["slots"][0]["coordinator_recovery_note"] = None
    state_path.write_text(json.dumps(state, indent=2) + "\n", encoding="utf-8")

    refused = remove(project)

    assert refused.returncode == 3
    assert "no owner lease and no coordinator recovery evidence" in refused.stderr
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
            {"schema": wrkslots.SCHEMA, "machine": "machine-b", "revision": 0, "records": []},
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
def test_status_refuses_registry_directory_mismatch(
    tmp_path: Path, mismatch: str
) -> None:
    project, repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    if mismatch == "row-without-directory":
        git(repository, "worktree", "remove", "--force", str(checkout(project)))
        (project / "worktrees" / "slot01").rmdir()
    else:
        (project / "worktrees" / "orphan").mkdir()

    refused = command(project, "status", "--all-machines")

    assert refused.returncode == 3
    expected = "slot directory is missing" if mismatch == "row-without-directory" else "directory without an active row"
    assert expected in refused.stderr


def test_unbound_owner_requires_coordinator_recovery_and_preserves_history(
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
    removed = remove(project)
    assert removed.returncode == 0, removed.stderr


def test_adopt_refuses_pid_outside_invoking_process_ancestry(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project, bind_owner=False)
    assert made.returncode == 0, made.stderr
    sleeper = subprocess.Popen(["sleep", "60"])
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
        sleeper.terminate()
        sleeper.wait(timeout=10)


def test_remove_refuses_live_process_using_slot(tmp_path: Path) -> None:
    project, repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    tree = checkout(project)
    sleeper = subprocess.Popen(["sleep", "60"], cwd=tree)
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
        sleeper.terminate()
        sleeper.wait(timeout=10)


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
    for slot, branch in (("slot01", "codex/import-one"), ("slot02", "codex/import-two")):
        git(repository, "worktree", "add", "-b", branch, str(slots / slot), "origin/main")

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
    monkeypatch.setattr(wrkslots, "_assert_caller_process", lambda _identity, _label: None)
    monkeypatch.setattr(wrkslots, "_read_process_identity", lambda _pid: reused)
    returncode = wrkslots.main(
        [
            "--project-root",
            str(project),
            "register",
            "slot01",
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


def test_path_escape_and_symlink_are_refused_without_touching_target(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)
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
        "product=../outside",
        "--branch",
        "product=codex/escape",
    )

    assert symlink_refusal.returncode == 3
    assert escape_refusal.returncode == 3
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
    assert "filename does not match machine" in refused.stderr
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

    refused = command(
        project,
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
        "--discard-partial",
    )

    assert refused.returncode == 3
    assert "active state has invalid fields" in refused.stderr
    assert partial.is_file()
    assert json.loads(partial.read_text(encoding="utf-8"))["slots"] == []


def test_corrupt_archive_refuses_before_any_worktree_deletion(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    archive = project / "worktrees" / "ARCHIVED.testhost.json"
    archive.write_text('{"schema": 1, "machine": "testhost"}\n', encoding="utf-8")

    refused = finish(project)

    assert refused.returncode == 3
    assert "archive state has invalid fields" in refused.stderr
    assert checkout(project).is_dir()
    assert len(active_slots(project)) == 1
    assert not (project / "worktrees" / "ACTIVE.testhost.journal").exists()


def test_malformed_active_timestamp_refuses_before_worktree_deletion(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    active_path = project / "worktrees" / "ACTIVE.testhost.json"
    state = json.loads(active_path.read_text(encoding="utf-8"))
    state["slots"][0]["created_at"] = "not-a-timestamp"
    active_path.write_text(json.dumps(state, indent=2) + "\n", encoding="utf-8")

    refused = finish(project)

    assert refused.returncode == 3
    assert "invalid timestamp" in refused.stderr
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

    refused = command(
        project, "recover", "--coordinator-pid", str(os.getpid())
    )

    assert refused.returncode == 3
    assert "differs from the finish journal" in refused.stderr
    assert journal_path.is_file()
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


def test_configuration_from_an_older_build_is_dead_until_repaired(tmp_path: Path) -> None:
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


def test_doctor_reports_every_disagreement_at_once_and_status_still_refuses(
    tmp_path: Path,
) -> None:
    """Diagnosis must show the shape of the drift; status keeps its refusal."""
    project, repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    git(repository, "worktree", "remove", "--force", str(checkout(project)))
    (project / "worktrees" / "slot01").rmdir()
    (project / "worktrees" / "orphan-a").mkdir()
    (project / "worktrees" / "orphan-b").mkdir()

    # The pre-existing contract is unchanged: status refuses, naming one thing.
    refused = command(project, "status", "--all-machines")
    assert refused.returncode == 3

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
    monkeypatch.setattr(wrkslots, "_assert_slot_unused", lambda *_args: None)
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
    monkeypatch.setattr(wrkslots, "_assert_slot_unused", lambda *_args: None)
    remove_code = wrkslots.main(
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
    monkeypatch.setattr(wrkslots, "_assert_slot_unused", lambda *_args: None)

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

    def scan_failed(*_args: object) -> None:
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

    monkeypatch.setattr(wrkslots, "_assert_slot_unused", lambda *_args: None)
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
