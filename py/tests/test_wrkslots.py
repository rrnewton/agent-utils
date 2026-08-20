"""Focused safety and lifecycle tests for the standalone wrkslots command."""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

import pytest


PY_ROOT = Path(__file__).resolve().parents[1]
WRKSLOTS = PY_ROOT / "wrkslots.py"
sys.path.insert(0, str(PY_ROOT))
import wrkslots  # noqa: E402


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
    argv = [sys.executable, str(WRKSLOTS), "--project-root", str(project)]
    if machine is not None:
        argv.extend(("--machine", machine))
    argv.extend(args)
    process_env = os.environ.copy()
    if env:
        process_env.update(env)
    return subprocess.run(
        argv,
        text=True,
        capture_output=True,
        check=False,
        env=process_env,
    )


def initialize(project: Path, *, machine: str = "testhost", directory: str = "worktrees") -> None:
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
    completed = subprocess.run(
        [
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
        ],
        text=True,
        capture_output=True,
        check=False,
    )
    assert completed.returncode == 0, completed.stderr


def make_project(tmp_path: Path, *, machine: str = "testhost") -> tuple[Path, Path, Path]:
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
    initialize(project, machine=machine)
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
    remote_url = git(project / repository_name, "remote", "get-url", "origin").stdout.strip()
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
        "--remote-url",
        f"{checkout_name}={remote_url}",
        "--branch",
        f"{checkout_name}={branch}",
        machine=machine,
        env=env,
    )


def active(project: Path, machine: str = "testhost") -> dict[str, object]:
    path = project / "worktrees" / f"ACTIVE.{machine}.json"
    value: object = json.loads(path.read_text(encoding="utf-8"))
    assert isinstance(value, dict)
    return value


def active_slots(project: Path, machine: str = "testhost") -> list[object]:
    value = active(project, machine)["slots"]
    assert isinstance(value, list)
    return value


def checkout(project: Path, slot: str = "slot01", name: str = "product") -> Path:
    return project / "worktrees" / slot / name


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
    state_path = project / "worktrees" / f"ACTIVE.{machine}.json"
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
    journal = project / "worktrees" / "ACTIVE.testhost.journal"
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
        str(WRKSLOTS),
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
        "--remote-url",
        f"product={git(repository, 'remote', 'get-url', 'origin').stdout.strip()}",
    ]
    first = subprocess.Popen(
        [*base, "--agent", "codex-1", "--branch", "product=codex/race-one"],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    second = subprocess.Popen(
        [*base, "--agent", "codex-2", "--branch", "product=codex/race-two"],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
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
            "import wrkslots",
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
            "--remote-url",
            f"product={git(project / 'repo', 'remote', 'get-url', 'origin').stdout.strip()}",
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
    project, repository, _remote = make_project(tmp_path)
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
        "--remote-url",
        f"product={git(repository, 'remote', 'get-url', 'origin').stdout.strip()}",
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
    assert len(active_slots(project)) == 1
    assert tree.is_dir()


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
            "--remote-url",
            f"product={git(repository, 'remote', 'get-url', 'origin').stdout.strip()}",
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
        "--remote-url",
        "product=file:///outside",
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
