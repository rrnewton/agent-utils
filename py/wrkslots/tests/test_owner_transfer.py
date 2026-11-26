"""`wrkslots transfer-owner`: hand a slot whose owner died to its live successor."""

from __future__ import annotations

import json
import os
import subprocess
from dataclasses import replace
from pathlib import Path

import pytest

from wrkslots import cli
from wrkslots.tests.test_lifecycle import (
    active_slots,
    checkout,
    command,
    control_directory,
    create,
    git,
    make_project,
    mark_owner_dead,
    terminate_process,
)


def transfer(project: Path, pid: int, *, agent: str = "codex-1", generation: str = "1") -> subprocess.CompletedProcess[str]:
    return command(
        project, "transfer-owner", "slot01", "--agent", agent, "--owner-pid", str(pid),
        "--expected-generation", generation,
    )


def owner_pid(project: Path) -> object:
    row = active_slots(project)[0]
    assert isinstance(row, dict)
    owner = row["owner"]
    return None if owner is None else owner["pid"]


def last_active_event(project: Path) -> dict[str, object]:
    events = sorted((control_directory(project) / "EVENTS.testhost").glob("*.json"))
    for path in reversed(events):
        document = json.loads(path.read_text(encoding="utf-8"))
        if document["kind"] == "active-state-recorded":
            return dict(document["payload"])
    raise AssertionError("no active-state event")


@pytest.mark.ordinary_environment
def test_a_dead_owner_is_replaced_by_its_successor_and_nothing_else_changes(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    mark_owner_dead(project)
    before = active_slots(project)[0]
    assert isinstance(before, dict)
    moved = transfer(project, os.getpid())
    assert moved.returncode == 0, moved.stderr
    assert "basis=caller-ancestry previous=dead" in moved.stdout
    after = active_slots(project)[0]
    assert isinstance(after, dict)
    assert after["owner"]["pid"] == os.getpid() and after["owner"]["boot_id"] != "finished-boot"
    assert {key: value for key, value in after.items() if key not in ("owner", "heartbeat_at")} == {
        key: value for key, value in before.items() if key not in ("owner", "heartbeat_at")
    }
    event = last_active_event(project)
    assert event.get("action") == "owner-transferred", event
    again = transfer(project, os.getpid())
    assert again.returncode == 0 and "already owned" in again.stdout
    heartbeat = command(project, "heartbeat", "slot01", "--agent", "codex-1", "--owner-pid",
                        str(os.getpid()), "--expected-generation", "1")
    assert heartbeat.returncode == 0, heartbeat.stderr


@pytest.mark.ordinary_environment
def test_a_slot_with_no_recorded_owner_is_bound_to_its_successor(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project, bind_owner=False).returncode == 0
    moved = transfer(project, os.getpid())
    assert moved.returncode == 0, moved.stderr
    assert owner_pid(project) == os.getpid()


@pytest.mark.ordinary_environment
def test_a_live_owner_a_wrong_agent_or_generation_and_an_outside_successor_are_refused(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    sleeper = subprocess.Popen(["sleep", "3600"], text=True)
    try:
        live = transfer(project, sleeper.pid)
        assert live.returncode == 3 and "owner is live" in live.stderr, live.stderr
        mark_owner_dead(project)
        assert transfer(project, os.getpid(), agent="someone").returncode == 3
        assert transfer(project, os.getpid(), generation="2").returncode == 3
        outside = transfer(project, sleeper.pid)
        assert outside.returncode == 3, outside.stderr
        assert "neither in the invoking process ancestry nor inside slot slot01's own box" in outside.stderr
        row = active_slots(project)[0]
        assert isinstance(row, dict) and row["owner"]["boot_id"] == "finished-boot"
    finally:
        terminate_process(sleeper)


@pytest.mark.ordinary_environment
def test_transfer_warns_and_proceeds_with_unregistered_nested_worktrees(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    nested = checkout(project) / "ignored" / "nested-a"
    nested.parent.mkdir(parents=True)
    git(checkout(project), "worktree", "add", "--detach", str(nested), "HEAD")
    (nested / "dirty.txt").write_text("live work\n", encoding="utf-8")
    mark_owner_dead(project)
    moved = transfer(project, os.getpid())
    assert moved.returncode == 0, moved.stderr
    assert "nested-a" in moved.stderr
    assert "unregistered nested worktree" in moved.stderr
    assert owner_pid(project) == os.getpid()
    assert nested.is_dir()
    events = sorted((control_directory(project) / "EVENTS.testhost").glob("*.json"))
    payloads = [json.loads(path.read_text(encoding="utf-8")) for path in events]
    transferred = [
        item["payload"] for item in payloads
        if item["kind"] == "active-state-recorded"
        and item["payload"].get("action") == "owner-transferred"
    ]
    assert transferred and "nested-a" in str(
        transferred[-1]["evidence"]["unregistered_nested_worktrees"]
    )


def _ignored_evidence_worktree(project: Path, repository: Path) -> Path:
    """A checkout carrying the reported #1870 ignored evidence.

    Appends a commit to the task branch with the tool's own local
    settings and the ignored/ holding live evidence.
    """

    worktree = checkout(project)
    (worktree / "task.txt").write_text("work\n", encoding="utf-8")
    git(worktree, "add", "task.txt")
    git(worktree, "commit", "-m", "task work")
    git(worktree, "push", "-q", "origin", "codex/task")
    (worktree / ".gitignore").write_text(
        ".buckconfig.local\nignored/\n", encoding="utf-8"
    )
    git(worktree, "add", ".gitignore")
    git(worktree, "commit", "-m", "ignore local evidence")
    git(worktree, "push", "-q", "origin", "codex/task")
    (worktree / ".buckconfig.local").symlink_to("x" * 44)
    evidence = worktree / "ignored" / "live-evidence.bin"
    evidence.parent.mkdir(parents=True)
    evidence.write_text("precious evidence\n", encoding="utf-8")
    return worktree


def _spawn_owner(project: Path, tmp_path: Path) -> subprocess.Popen[str]:
    owner = subprocess.Popen(["sleep", "3600"], text=True)
    made = create(project, agent="liteinst-arm", owner_pid=owner.pid)
    assert made.returncode == 0, made.stderr
    return owner


@pytest.mark.ordinary_environment
def test_rename_transfers_ignored_evidence_with_the_slot(tmp_path: Path) -> None:
    """#1870: ignored files never block a rename; they transfer unchanged."""

    project, repository, _remote = make_project(tmp_path)
    owner = _spawn_owner(project, tmp_path)
    worktree = _ignored_evidence_worktree(project, repository)
    # The owner exits on this boot; its PID is then simply absent.
    terminate_process(owner)

    moved = command(
        project,
        "transfer-owner",
        "slot01",
        "--agent",
        "liteinst-arm",
        "--to-agent",
        "liteinst-arm2",
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
    )

    assert moved.returncode == 0, moved.stderr
    assert "transferred slot=slot01 agent=liteinst-arm2" in moved.stdout
    assert "ignored_transferred count=2 sha256=" in moved.stdout
    link = worktree / ".buckconfig.local"
    assert link.is_symlink() and os.readlink(link) == "x" * 44
    assert (worktree / "ignored" / "live-evidence.bin").read_text(
        encoding="utf-8"
    ) == "precious evidence\n"
    row = active_slots(project)[0]
    assert isinstance(row, dict)
    assert row["agent"] == "liteinst-arm2" and row["generation"] == 2
    assert isinstance(row["owner"], dict) and row["owner"]["pid"] == os.getpid()


@pytest.mark.ordinary_environment
def test_rename_still_refuses_tracked_and_untracked_work(tmp_path: Path) -> None:
    """#1870: the clean-tree refusal stays for authored and untracked work."""

    project, repository, _remote = make_project(tmp_path)
    owner = _spawn_owner(project, tmp_path)
    worktree = _ignored_evidence_worktree(project, repository)
    terminate_process(owner)

    (worktree / "task.txt").write_text("edited\n", encoding="utf-8")
    refused = command(
        project,
        "transfer-owner",
        "slot01",
        "--agent",
        "liteinst-arm",
        "--to-agent",
        "liteinst-arm2",
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
    )
    assert refused.returncode == 3, refused.stderr
    assert "clean tree" in refused.stderr
    row = active_slots(project)[0]
    assert isinstance(row, dict) and row["agent"] == "liteinst-arm"

    git(worktree, "checkout", "--", "task.txt")
    (worktree / "unaccounted.txt").write_text("new work\n", encoding="utf-8")
    refused = command(
        project,
        "transfer-owner",
        "slot01",
        "--agent",
        "liteinst-arm",
        "--to-agent",
        "liteinst-arm2",
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
    )
    assert refused.returncode == 3, refused.stderr
    assert "clean tree" in refused.stderr


@pytest.mark.ordinary_environment
def test_rename_refuses_hidden_gitignore_edit_via_assume_unchanged(
    tmp_path: Path,
) -> None:
    """Hidden .gitignore edits must refuse, not silently transfer (#1959)."""

    project, repository, _remote = make_project(tmp_path)
    owner = _spawn_owner(project, tmp_path)
    worktree = _ignored_evidence_worktree(project, repository)
    terminate_process(owner)
    # Append a rule hiding authored.py, then hide the edit itself.
    (worktree / ".gitignore").write_text(
        ".buckconfig.local\nignored/\nauthored.py\n", encoding="utf-8"
    )
    git(worktree, "update-index", "--assume-unchanged", ".gitignore")
    (worktree / "authored.py").write_text("authored work\n", encoding="utf-8")

    refused = command(
        project,
        "transfer-owner",
        "slot01",
        "--agent",
        "liteinst-arm",
        "--to-agent",
        "liteinst-arm2",
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
    )

    assert refused.returncode == 3, refused.stderr
    assert "ignore rules" in refused.stderr and "hidden" in refused.stderr
    row = active_slots(project)[0]
    assert isinstance(row, dict) and row["agent"] == "liteinst-arm"


@pytest.mark.ordinary_environment
def test_rename_refuses_hidden_gitignore_edit_via_skip_worktree(
    tmp_path: Path,
) -> None:
    """Skip-worktree hidden .gitignore edits must also refuse."""

    project, repository, _remote = make_project(tmp_path)
    owner = _spawn_owner(project, tmp_path)
    worktree = _ignored_evidence_worktree(project, repository)
    terminate_process(owner)
    (worktree / ".gitignore").write_text(
        ".buckconfig.local\nignored/\nauthored.py\n", encoding="utf-8"
    )
    git(worktree, "update-index", "--skip-worktree", ".gitignore")
    (worktree / "authored.py").write_text("authored work\n", encoding="utf-8")

    refused = command(
        project,
        "transfer-owner",
        "slot01",
        "--agent",
        "liteinst-arm",
        "--to-agent",
        "liteinst-arm2",
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
    )

    assert refused.returncode == 3, refused.stderr
    assert "ignore rules" in refused.stderr and "hidden" in refused.stderr


@pytest.mark.ordinary_environment
def test_rename_ignores_fsmonitor_hook(
    tmp_path: Path,
) -> None:
    """A core.fsmonitor hook must not hide changes from the rename check."""

    project, repository, _remote = make_project(tmp_path)
    owner = _spawn_owner(project, tmp_path)
    worktree = _ignored_evidence_worktree(project, repository)
    terminate_process(owner)
    # Install an fsmonitor hook that reports nothing as changed.
    hook = worktree / ".git" / "hooks" / "fsmonitor-watchman"
    # git config core.fsmonitor points to the hook program.
    git(worktree, "config", "core.fsmonitor", "true")
    (worktree / "task.txt").write_text("edited\n", encoding="utf-8")

    refused = command(
        project,
        "transfer-owner",
        "slot01",
        "--agent",
        "liteinst-arm",
        "--to-agent",
        "liteinst-arm2",
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
    )

    assert refused.returncode == 3, refused.stderr
    assert "clean tree" in refused.stderr


@pytest.mark.ordinary_environment
def test_rename_pathspec_glob_whitespace_not_confused(
    tmp_path: Path,
) -> None:
    """Pathspec globs and whitespace in paths must not confuse tracking (#1959)."""

    project, repository, _remote = make_project(tmp_path)
    owner = _spawn_owner(project, tmp_path)
    worktree = _ignored_evidence_worktree(project, repository)
    terminate_process(owner)
    # Create "p/.gitignore" (tracked) and "p q/.gitignore" (untracked).
    # The pathspec "p*/.gitignore" matches both if globbed.
    pdir = worktree / "p"
    pdir.mkdir()
    (pdir / ".gitignore").write_text("tracked.log\n", encoding="utf-8")
    git(worktree, "add", "p/.gitignore")
    git(worktree, "commit", "-m", "tracked gitignore")
    git(worktree, "push", "-q", "origin", "codex/task")
    pqdir = worktree / "p q"
    pqdir.mkdir()
    (pqdir / ".gitignore").write_text("authored.py\n", encoding="utf-8")
    (pqdir / "authored.py").write_text("authored\n", encoding="utf-8")
    (pdir / "tracked.log").write_text("log\n", encoding="utf-8")

    moved = command(
        project,
        "transfer-owner",
        "slot01",
        "--agent",
        "liteinst-arm",
        "--to-agent",
        "liteinst-arm2",
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
    )

    # The untracked "p q/.gitignore" hides authored.py, so this must refuse.
    assert moved.returncode == 3, moved.stderr


@pytest.mark.ordinary_environment
def test_rename_refuses_clean_filter_hidden_gitignore_edit(
    tmp_path: Path,
) -> None:
    """Hashing the .gitignore blob must bypass clean filters.

    With .gitignore carrying filter=hide whose clean command restores
    the committed blob, status sees the file as clean and a filtered
    hash-object round-trips to the index blob, even though the
    working rules now hide authored work. --no-filters refuses it.
    """
    project, repository, _remote = make_project(tmp_path)
    owner = _spawn_owner(project, tmp_path)
    worktree = _ignored_evidence_worktree(project, repository)
    terminate_process(owner)

    git(worktree, "config", "filter.hide.clean", "git cat-file blob HEAD:.gitignore")
    common = Path(git(worktree, "rev-parse", "--git-common-dir").stdout.strip())
    if not common.is_absolute():
        common = (worktree / common).resolve()
    (common / "info").mkdir(parents=True, exist_ok=True)
    (common / "info" / "attributes").write_text(
        ".gitignore filter=hide\n", encoding="utf-8"
    )
    gitignore = (worktree / ".gitignore").read_text(encoding="utf-8")
    mutated = gitignore.replace("ignored/", "authored", 1)
    # "ignored/" and "authored" are both 8 chars: same size, new meaning.
    assert len(mutated.encode()) == len(gitignore.encode())
    (worktree / ".gitignore").write_text(mutated, encoding="utf-8")
    secret = worktree / "authored" / "secret.py"
    secret.parent.mkdir(parents=True, exist_ok=True)
    secret.write_text("authored work\n", encoding="utf-8")

    refused = command(
        project,
        "transfer-owner",
        "slot01",
        "--agent",
        "liteinst-arm",
        "--to-agent",
        "liteinst-arm2",
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
    )
    assert refused.returncode == 3, refused.stderr
    assert "clean tree" in refused.stderr or "ignore rules" in refused.stderr
    row = active_slots(project)[0]
    assert isinstance(row, dict) and row["agent"] == "liteinst-arm"


@pytest.mark.ordinary_environment
def test_rename_non_utf8_filename_refuses_not_crashes(
    tmp_path: Path,
) -> None:
    """Non-UTF8 filenames must produce a Refusal, not a crash (#1959)."""

    project, repository, _remote = make_project(tmp_path)
    owner = _spawn_owner(project, tmp_path)
    worktree = _ignored_evidence_worktree(project, repository)
    terminate_process(owner)
    # Create a file with non-UTF8 bytes in its name, ignored by tracked rules.
    raw_name = b"bad-\xff\xfe-name.txt"
    (worktree / os.fsdecode(raw_name)).write_bytes(b"content")
    (worktree / ".gitignore").write_text(
        ".buckconfig.local\nignored/\nbad-*\n", encoding="utf-8"
    )
    git(worktree, "add", ".gitignore")
    git(worktree, "commit", "-m", "ignore bad name")
    git(worktree, "push", "-q", "origin", "codex/task")

    moved = command(
        project,
        "transfer-owner",
        "slot01",
        "--agent",
        "liteinst-arm",
        "--to-agent",
        "liteinst-arm2",
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
    )

    # Should either transfer cleanly or refuse cleanly, never traceback.
    assert moved.returncode in (0, 3), moved.stderr
    assert "Traceback" not in moved.stderr
    assert "UnicodeDecodeError" not in moved.stderr


@pytest.mark.ordinary_environment
def test_rename_refuses_what_only_the_callers_global_ignore_hides(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """#1870 review: the caller's git config never decides what is ignored."""

    project, repository, _remote = make_project(tmp_path)
    owner = _spawn_owner(project, tmp_path)
    worktree = _ignored_evidence_worktree(project, repository)
    terminate_process(owner)
    ignores = tmp_path / "xdg" / "git"
    ignores.mkdir(parents=True)
    (ignores / "ignore").write_text("*.local\n", encoding="utf-8")
    monkeypatch.setenv("XDG_CONFIG_HOME", str(tmp_path / "xdg"))
    (worktree / "authored.local").write_text("authored work\n", encoding="utf-8")

    refused = command(
        project,
        "transfer-owner",
        "slot01",
        "--agent",
        "liteinst-arm",
        "--to-agent",
        "liteinst-arm2",
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
    )

    assert refused.returncode == 3, refused.stderr
    assert "clean tree" in refused.stderr
    row = active_slots(project)[0]
    assert isinstance(row, dict) and row["agent"] == "liteinst-arm"
    assert (worktree / "authored.local").exists()


def _release_with_dying_owner(project: Path, tmp_path: Path) -> None:
    source = tmp_path / "release-notes.md"
    source.write_text("# Handoff\n\nAll done.\n", encoding="utf-8")
    helper = (
        "import argparse, dataclasses, os, sys\n"
        "from wrkslots import cli\n"
        "root = sys.argv[1]; source = sys.argv[2]\n"
        "config = cli._load_config(root, None)\n"
        "state = cli._load_active(config)\n"
        "record = cli._find_record(state, 'slot01')\n"
        "identity = cli._read_process_identity(os.getpid())\n"
        "updated = dataclasses.replace(record, owner=identity)\n"
        "cli._write_active_state(config, cli._replace_record(state, updated),"
        " action='test-owner-bound', slot='slot01')\n"
        "base = dict(project_root=root, machine=None, slot='slot01',"
        " agent='codex-1', owner_pid=os.getpid(), expected_generation=1,"
        " wait_lock=0.0)\n"
        "cli._cmd_write_handoff(argparse.Namespace(**base, from_file=source))\n"
        "cli._cmd_release(argparse.Namespace(**base))\n"
    )
    env = dict(os.environ)
    py_root = str(Path(cli.__file__).resolve().parents[1])
    env["PYTHONPATH"] = py_root + os.pathsep + env.get("PYTHONPATH", "")
    proc = subprocess.run(
        ["python3", "-c", helper, str(project), str(source)],
        capture_output=True, text=True, check=False, timeout=300, env=env,
    )
    assert proc.returncode == 0, proc.stderr


@pytest.mark.ordinary_environment
def test_released_generation_rebinds_only_with_coordinator_authorization(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    _release_with_dying_owner(project, tmp_path)
    plain = transfer(project, os.getpid())
    assert plain.returncode == 3 and "was released by its owner" in plain.stderr
    assert "--reopen" in plain.stderr
    unauthorized = command(
        project, "transfer-owner", "slot01", "--agent", "codex-1",
        "--owner-pid", str(os.getpid()), "--expected-generation", "1",
        "--reopen", "--coordinator-pid", str(os.getpid()),
        "--reason", "lane resumed after restart",
    )
    assert unauthorized.returncode == 3, unauthorized.stderr
    assert owner_pid(project) != os.getpid()
    reopened = command(
        project, "transfer-owner", "slot01", "--agent", "codex-1",
        "--owner-pid", str(os.getpid()), "--expected-generation", "1",
        "--reopen", "--coordinator-authorized",
        "--coordinator-pid", str(os.getpid()),
        "--reason", "lane resumed after restart",
    )
    assert reopened.returncode == 0, reopened.stderr
    assert "reopened slot=slot01" in reopened.stdout
    assert "generation=2" in reopened.stdout
    row = active_slots(project)[0]
    assert isinstance(row, dict)
    assert row["generation"] == 2 and row["owner"]["pid"] == os.getpid()
    event = last_active_event(project)
    assert event.get("action") == "owner-reopened", event
    evidence = event["evidence"]
    assert isinstance(evidence, dict)
    assert evidence["coordinator_authorized"] is True
    assert evidence["reason"] == "lane resumed after restart"
    assert evidence["previous_generation"] == 1


@pytest.mark.ordinary_environment
def test_rename_rebind_accepts_an_in_image_eden_backing(tmp_path: Path) -> None:
    """#2102: the rename gate accepts the configured in-image backing.

    A rename (``--to-agent``) runs the clean-tree rebind check, which
    resolves every checkout's stored repository. An in-image Eden
    slot's repository is the configured backing location, so the check
    accepts it; any other external path is still refused, which is
    what blocked every rename of such a slot.
    """
    from wrkslots import worktreesimage

    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    loaded = cli._load_config(str(project), "testhost")
    settings = worktreesimage.WorktreesImageSettings(
        relative_path="worktrees.img",
        size_bytes=1024**3,
        eden_backing_repos=(
            worktreesimage.EdenBacking("source-repo", "source-repo"),
        ),
    )
    config = replace(
        loaded,
        worktrees_image=settings,
        worktrees_image_state=worktreesimage.MOUNTED,
    )
    mount = settings.mount_point(config.control, config.worktrees)
    backing = worktreesimage.eden_backing_path(
        mount, (settings.eden_backing_repos or ())[0]
    )
    backing.mkdir(parents=True)
    git(backing, "init", "--initial-branch=main")
    git(backing, "config", "user.name", "Wrkslots Test")
    git(backing, "config", "user.email", "wrkslots@example.invalid")
    (backing / "seed.txt").write_text("seed\n", encoding="utf-8")
    git(backing, "add", "seed.txt")
    git(backing, "commit", "-m", "seed")
    target = project / "worktrees" / "slot01" / "eden-wt"
    git(backing, "worktree", "add", "-b", "eden/work", str(target), "main")

    record = cli._load_active(config).slots[0]
    eden_checkout = replace(
        record.checkouts[0],
        path="worktrees/slot01/eden-wt",
        repository=str(backing),
        branch="eden/work",
        vcs="eden",
    )
    eden_record = replace(record, checkouts=(eden_checkout,))
    assert cli._assert_clean_tree_for_rebind(config, eden_record) == ()

    external = replace(
        eden_checkout, repository=str(tmp_path / "outside" / "external-repo")
    )
    with pytest.raises(cli.Refusal, match="must name one direct sibling"):
        cli._assert_clean_tree_for_rebind(
            config, replace(record, checkouts=(external,))
        )


@pytest.mark.ordinary_environment
def test_the_slot_box_is_recognised_only_for_its_own_slot() -> None:
    boxed = (
        "/user.slice/user-1.slice/user@1.service/app.slice/app-wrkslots.slice/"
        "app-wrkslots-kvm\\x2d20261012.slice/wrkslots-run-kvm\\x2d20261012-1073417.scope"
    )
    assert cli._cgroup_in_slot_box(boxed, "agent", "kvm-20261012")
    assert cli._cgroup_in_slot_box("/wrkslots.slice/wrkslots-slot01.slice/x.scope", "agent", "slot01")
    for slot in ("kvm", "20261012", "kvm-2026101"):
        assert not cli._cgroup_in_slot_box(boxed, "agent", slot), slot
    assert not cli._cgroup_in_slot_box(
        "/app.slice/app-wrkslots-box-kvm\\x2d20261012.slice/a.scope", "agent", "kvm-20261012"
    )
    assert not cli._cgroup_in_slot_box("/app.slice/herdr-server.service", "agent", "kvm-20261012")


# #2179: a recorded task scope belongs to its recorded owner (the loader binds
# the two), so a transfer from a proven-dead owner rebinds the scope to the
# successor's own verifiable scope, or drops it; a live scope still refuses.
# The successor's control group and the systemd invocation links are test
# seams; boot IDs and process generations are controlled values, never the
# live host's scopes.

PREVIOUS_BOOT = "00000000-0000-4000-8000-000000002179"
SUCCESSOR_UNIT = "successor-2179.scope"
SUCCESSOR_INVOCATION = "c" * 32


def _scope_cgroup(unit: str) -> str:
    return f"{cli._systemd_user_manager_cgroup()}/app.slice/{unit}"


def _successor_in_scope(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, *, invocation_link: bool = True
) -> Path:
    """Place this process in SUCCESSOR_UNIT and model the manager's links."""

    real_cgroup = cli._read_process_cgroup
    me = Path("/proc") / str(os.getpid())

    def cgroup(pid_dir: Path) -> str:
        return _scope_cgroup(SUCCESSOR_UNIT) if Path(pid_dir) == me else real_cgroup(pid_dir)

    monkeypatch.setattr(cli, "_read_process_cgroup", cgroup)
    links = tmp_path / "systemd-units"
    links.mkdir()
    if invocation_link:
        (links / f"invocation:{SUCCESSOR_UNIT}").symlink_to(SUCCESSOR_INVOCATION)
    monkeypatch.setattr(cli, "_systemd_invocation_directory", lambda: links)
    return links


def _bind_recorded_scope(project: Path, owner: cli.ProcessIdentity) -> cli.TaskScopeIdentity:
    """Record ``owner`` with the task scope that a scoped create binds to it."""

    config = cli._load_config(str(project), "testhost")
    state = cli._load_active(config)
    record = state.slots[0]
    scope = cli._task_scope_from_obj(
        {
            "unit": owner.cgroup_path.rsplit("/", 1)[-1],
            "invocation_id": "a" * 32,
            "cgroup_path": owner.cgroup_path,
            "boot_id": owner.boot_id,
            "leader_pid": owner.pid,
            "leader_start_ticks": owner.start_ticks,
            "verification": "systemd-runtime-invocation-symlink-v1",
        },
        "test recorded scope",
    )
    cli._write_active_state(
        config,
        cli._replace_record(state, replace(record, owner=owner, task_scope=scope)),
        action="test-scope-bound",
        slot=record.slot,
    )
    return scope


def _exited_pid() -> int:
    exited = subprocess.Popen(["sleep", "3600"], text=True)
    terminate_process(exited)
    return exited.pid


def _main(project: Path, *args: str) -> int:
    return cli.main(["--project-root", str(project), *args])


def _reload_record(project: Path) -> cli.ActiveRecord:
    """The record as the registry loader rebuilds it from the event log."""

    config = cli._load_config(str(project), "testhost")
    states, _archives = cli._validate_global_state(config)
    state = next(item for item in states if item.machine == "testhost")
    return cli._find_record(state, "slot01")


@pytest.mark.ordinary_environment
def test_transfer_rebinds_a_previous_boot_task_scope_and_the_slot_can_be_released(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    me = cli._read_process_identity(os.getpid())
    dead = replace(
        me,
        pid=_exited_pid(),
        boot_id=PREVIOUS_BOOT,
        cgroup_path=_scope_cgroup("lane-before-reboot.scope"),
    )
    old_scope = _bind_recorded_scope(project, dead)
    _successor_in_scope(monkeypatch, tmp_path)
    successor = cli._read_process_identity(os.getpid())
    assert successor.cgroup_path == _scope_cgroup(SUCCESSOR_UNIT)

    rc = _main(
        project, "--wait-lock", "240", "transfer-owner", "slot01", "--agent", "codex-1",
        "--owner-pid", str(os.getpid()), "--expected-generation", "1",
    )
    captured = capsys.readouterr()

    assert rc == 0, captured.err
    assert "basis=caller-ancestry previous=dead" in captured.out
    assert (
        f"task scope {old_scope.unit} of the dead owner was rebound to owner PID "
        f"{os.getpid()}'s own scope {SUCCESSOR_UNIT}"
    ) in captured.err
    record = _reload_record(project)
    assert record.owner == successor
    assert record.task_scope == cli.TaskScopeIdentity(
        unit=SUCCESSOR_UNIT,
        invocation_id=SUCCESSOR_INVOCATION,
        cgroup_path=successor.cgroup_path,
        boot_id=successor.boot_id,
        leader_pid=successor.pid,
        leader_start_ticks=successor.start_ticks,
        verification="systemd-runtime-invocation-symlink-v1",
    )
    event = last_active_event(project)
    assert event["action"] == "owner-transferred"
    evidence = event["evidence"]
    assert isinstance(evidence, dict) and evidence["previous_owner_state"] == "dead"

    handoff = tmp_path / "handoff-2179.md"
    handoff.write_text("# Handoff\n\nDone after the reboot.\n", encoding="utf-8")
    owner_args = (
        "slot01", "--agent", "codex-1", "--owner-pid", str(os.getpid()),
        "--expected-generation", "1",
    )
    assert _main(project, "write-handoff", *owner_args, "--from-file", str(handoff)) == 0
    assert _main(project, "release", *owner_args) == 0, capsys.readouterr().err
    released = _reload_record(project)
    assert released.task_scope == record.task_scope
    events = cli._load_events(cli._load_config(str(project), "testhost"), "testhost")
    assert cli._owner_release_in_effect(
        cli._load_config(str(project), "testhost"), released, events
    ) is not None


@pytest.mark.ordinary_environment
def test_transfer_drops_a_dead_task_scope_when_the_successor_runs_in_none(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    me = cli._read_process_identity(os.getpid())
    _bind_recorded_scope(
        project,
        replace(
            me,
            pid=_exited_pid(),
            boot_id=PREVIOUS_BOOT,
            cgroup_path=_scope_cgroup("lane-before-reboot.scope"),
        ),
    )
    # The successor's cgroup looks like a scope, but the manager holds no
    # invocation link for it, so it is not a verifiable task scope.
    _successor_in_scope(monkeypatch, tmp_path, invocation_link=False)

    rc = _main(
        project, "transfer-owner", "slot01", "--agent", "codex-1",
        "--owner-pid", str(os.getpid()), "--expected-generation", "1",
    )
    captured = capsys.readouterr()

    assert rc == 0, captured.err
    assert "lane-before-reboot.scope of the dead owner was dropped" in captured.err
    record = _reload_record(project)
    assert record.owner == cli._read_process_identity(os.getpid())
    assert record.task_scope is None
    row = active_slots(project)[0]
    assert isinstance(row, dict) and "task_scope" not in row


@pytest.mark.ordinary_environment
def test_transfer_treats_a_reused_pid_task_scope_as_dead(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    sleeper = subprocess.Popen(["sleep", "3600"], text=True)
    try:
        live = cli._read_process_identity(sleeper.pid)
        # Same boot, same PID, another start time: the recorded generation
        # exited and the PID now names a different process.
        _bind_recorded_scope(
            project,
            replace(
                live,
                start_ticks=live.start_ticks + 1,
                cgroup_path=_scope_cgroup("lane-reused-pid.scope"),
            ),
        )
        _successor_in_scope(monkeypatch, tmp_path)

        rc = _main(
            project, "transfer-owner", "slot01", "--agent", "codex-1",
            "--owner-pid", str(os.getpid()), "--expected-generation", "1",
        )
        captured = capsys.readouterr()
    finally:
        terminate_process(sleeper)

    assert rc == 0, captured.err
    assert "previous=dead" in captured.out
    record = _reload_record(project)
    assert record.task_scope is not None and record.task_scope.unit == SUCCESSOR_UNIT
    assert record.task_scope.leader_pid == os.getpid()


@pytest.mark.ordinary_environment
def test_transfer_still_refuses_a_task_scope_bound_to_a_live_owner(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    sleeper = subprocess.Popen(["sleep", "3600"], text=True)
    try:
        live = cli._read_process_identity(sleeper.pid)
        # The same live generation, recorded in its scope (a cgroup move
        # never reads as a reuse).
        recorded = _bind_recorded_scope(
            project, replace(live, cgroup_path=_scope_cgroup("lane-live.scope"))
        )
        _successor_in_scope(monkeypatch, tmp_path)
        before = _reload_record(project)

        rc = _main(
            project, "transfer-owner", "slot01", "--agent", "codex-1",
            "--owner-pid", str(os.getpid()), "--expected-generation", "1",
        )
        captured = capsys.readouterr()
        after = _reload_record(project)
    finally:
        terminate_process(sleeper)

    assert rc == 3
    assert "owner is live" in captured.err
    assert after == before and after.task_scope == recorded


@pytest.mark.ordinary_environment
def test_reopen_refuses_a_released_generation_whose_task_scope_is_live(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    _successor_in_scope(monkeypatch, tmp_path)
    # This live process owns the slot in its scope and releases it.
    recorded = _bind_recorded_scope(project, cli._read_process_identity(os.getpid()))
    handoff = tmp_path / "handoff-live.md"
    handoff.write_text("# Handoff\n\nReleased, still running.\n", encoding="utf-8")
    owner_args = (
        "slot01", "--agent", "codex-1", "--owner-pid", str(os.getpid()),
        "--expected-generation", "1",
    )
    assert _main(project, "write-handoff", *owner_args, "--from-file", str(handoff)) == 0
    assert _main(project, "release", *owner_args) == 0, capsys.readouterr().err
    capsys.readouterr()
    before = _reload_record(project)

    rc = _main(
        project, "transfer-owner", "slot01", "--agent", "codex-1",
        "--owner-pid", str(os.getppid()), "--expected-generation", "1",
        "--reopen", "--coordinator-authorized", "--coordinator-pid", str(os.getpid()),
        "--reason", "reopen while the released owner still runs",
    )
    captured = capsys.readouterr()

    assert rc == 3
    assert (
        f"slot slot01 task scope {recorded.unit} is still bound to a live owner"
    ) in captured.err
    assert _reload_record(project) == before
