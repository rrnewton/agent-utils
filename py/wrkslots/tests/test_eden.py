"""Eden checkout kind, cap, salvage and remove checks (#1204)."""

from __future__ import annotations

import dataclasses
import json
from collections.abc import Sequence
from pathlib import Path

import pytest
from wrkslots import cli as wrkslots_cli
from wrkslots import eden, slotimage

REV = "a" * 40
DIGEST = "b" * 64


def _eden_obj() -> dict[str, object]:
    return {
        "name": "main",
        "path": "checkout",
        "repository": "/backing/repo",
        "branch": eden.EDEN_BRANCH,
        "start_point": REV,
        "remote": eden.EDEN_REMOTE,
        "remote_url_sha256": DIGEST,
        "landed_ref": eden.EDEN_LANDED_REF,
        "head": REV,
        "containing_remote_refs": [],
        "vcs": "eden",
    }


def test_eden_checkout_round_trip() -> None:
    checkout = wrkslots_cli._checkout_from_obj(_eden_obj(), "row.checkouts[0]")
    assert checkout.vcs == "eden" and checkout.head == REV
    assert wrkslots_cli._checkout_to_obj(checkout) == _eden_obj()


def test_eden_checkout_destination_is_always_checkout() -> None:
    slot = Path("/slots/s1")
    for layout in ("nested", "flat"):
        assert (
            wrkslots_cli._checkout_destination(slot, layout, "main", vcs="eden")
            == slot / "checkout"
        )
    assert (
        wrkslots_cli._checkout_destination(slot, "nested", "main")
        == slot / "main"
    )
    assert wrkslots_cli._checkout_destination(slot, "flat", "main") == slot






def test_parse_proc_stat_allows_pid1_zero_start() -> None:
    raw = (
        b"1 (systemd) S 0 1 1 0 -1 4194560 100 200 0 0 "
        b"10 10 0 0 20 0 1 0 0 1000 0 18446744073709551615"
    )
    parsed = wrkslots_cli._parse_proc_stat(raw)
    assert parsed is not None and parsed.pid == 1 and parsed.start_ticks == 0
    other = raw.replace(b"1 (systemd)", b"2 (kthreadd)", 1)
    assert wrkslots_cli._parse_proc_stat(other) is not None


def test_slot_eden_daemon_cmdline_recognition() -> None:
    slot = Path("/slots/s1")
    own = "/usr/local/libexec/eden/edenfs --edenfs --edenDir /slots/s1/.eden-state --foreground"
    other = "/usr/local/libexec/eden/edenfs --edenfs --edenDir /slots/s2/.eden-state"
    assert wrkslots_cli._cmdline_is_slot_eden_daemon(own, slot)
    assert wrkslots_cli._cmdline_is_slot_eden_daemon(
        "edenfs --edenDir=/slots/s1/.eden-state", slot
    )
    assert not wrkslots_cli._cmdline_is_slot_eden_daemon(other, slot)
    assert not wrkslots_cli._cmdline_is_slot_eden_daemon("vim /slots/s1/x", slot)
    assert wrkslots_cli._cmdline_is_slot_eden_daemon(
        "edenfs_privhelper --privhelper_uid=1", slot
    )


def test_eden_remote_unchanged_fails_closed(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    checkout = wrkslots_cli.Checkout(
        name="main",
        path="worktrees/slots/s1/checkout",
        repository="/backing/repo",
        branch=eden.EDEN_BRANCH,
        start_point=REV,
        remote=eden.EDEN_REMOTE,
        remote_url_sha256=DIGEST,
        landed_ref=eden.EDEN_LANDED_REF,
        head=REV,
        containing_remote_refs=(),
        vcs="eden",
    )
    path = Path("/slots/s1/checkout")
    vcs = wrkslots_cli._GitVcs()
    monkeypatch.setattr(
        wrkslots_cli, "_eden_runner", lambda argv: (1, "", "daemon down")
    )
    assert not wrkslots_cli._checkout_remote_unchanged(vcs, path, checkout)
    monkeypatch.setattr(
        wrkslots_cli, "_eden_runner", lambda argv: (0, "not json", "")
    )
    assert not wrkslots_cli._checkout_remote_unchanged(vcs, path, checkout)
    good = (
        '{"mount": "/slots/s1/checkout", "state_dir": "/s",'
        ' "backing_repo": "/backing/repo",'
        f' "checked_out_revision": "{REV}"}}'
    )
    monkeypatch.setattr(
        wrkslots_cli, "_eden_runner", lambda argv: (0, good, "")
    )
    assert wrkslots_cli._checkout_remote_unchanged(vcs, path, checkout)
    other = good.replace("/backing/repo", "/backing/other")
    monkeypatch.setattr(
        wrkslots_cli, "_eden_runner", lambda argv: (0, other, "")
    )
    assert not wrkslots_cli._checkout_remote_unchanged(vcs, path, checkout)


def test_eden_record_paths_accept_new_and_legacy_forms(
    tmp_path: Path,
) -> None:
    config = _direct_config(tmp_path)
    record = _eden_validate_record(config, "/backing/repo")
    new_path = record.checkouts[0].path
    legacy_path = new_path.rsplit("/", 1)[0] + "/main"
    for path in (new_path, legacy_path):
        legacy = dataclasses.replace(
            record,
            checkouts=(
                dataclasses.replace(record.checkouts[0], path=path),
            ),
        )
        wrkslots_cli._assert_record_paths(
            config, legacy, require_repository=False, historical=True
        )
        wrkslots_cli._assert_record_paths(
            config, legacy, require_repository=False
        )


def test_census_record_drops_owner_only_for_eden(tmp_path: Path) -> None:
    config = _direct_config(tmp_path)
    eden_record = _eden_validate_record(config, "/backing/repo")
    assert wrkslots_cli._census_record(None) is None
    git_record = dataclasses.replace(
        eden_record,
        checkouts=(
            dataclasses.replace(eden_record.checkouts[0], vcs="git"),
        ),
    )
    assert wrkslots_cli._census_record(git_record) is None
    census = wrkslots_cli._census_record(eden_record)
    assert census is not None
    assert census.owner is None
    assert wrkslots_cli._record_is_eden(census)


def test_eden_checkout_rejects_wrong_markers() -> None:
    bad = _eden_obj()
    bad["branch"] = "main"
    with pytest.raises(wrkslots_cli.StateError):
        wrkslots_cli._checkout_from_obj(bad, "row.checkouts[0]")
    bad = _eden_obj()
    bad["vcs"] = "svn"
    with pytest.raises(wrkslots_cli.StateError):
        wrkslots_cli._checkout_from_obj(bad, "row.checkouts[0]")


def _fake_runner(table: dict[str, tuple[int, str, str]]) -> eden.Runner:
    def run(argv: Sequence[str]) -> tuple[int, str, str]:
        return table[argv[2]]

    return run


def test_cap_counts_every_config_dir_and_dedupes() -> None:
    runner = _fake_runner(
        {
            "/state-a": (0, "/m/one\n/m/two\n", ""),
            "/state-b": (0, "/m/two\n/m/three\n", ""),
        }
    )
    mounts = eden.count_checkouts(runner, ("/state-a", "/state-b"))
    assert mounts == ("/m/one", "/m/two", "/m/three")
    with pytest.raises(eden.EdenError, match="cap reached"):
        eden.assert_under_cap(mounts)
    eden.assert_under_cap(mounts[:2])


def test_cap_refuses_when_a_daemon_cannot_be_queried() -> None:
    runner = _fake_runner({"/state-a": (1, "", "daemon down")})
    with pytest.raises(eden.EdenError, match="cannot list"):
        eden.count_checkouts(runner, ("/state-a",))


def test_config_dir_discovery_from_daemons() -> None:
    cmdlines = (
        "/usr/local/libexec/eden/edenfs --edenfs --edenDir /state-a --foreground",
        "edenfs --edenDir=/state-b",
        "unrelated process",
    )
    assert eden.discover_config_dirs(cmdlines, ("/state-c",)) == (
        "/state-a",
        "/state-b",
        "/state-c",
    )


def test_salvage_and_remove_verification() -> None:
    assert eden.salvage_needed(" M file\n", REV, REV)
    assert eden.salvage_needed("", "c" * 40, REV)
    assert not eden.salvage_needed("", REV, REV)
    commands = eden.salvage_commands("/m/one", Path("/salvage/slot"))
    assert commands[-1][0][-1].endswith("draft-stack.bundle")
    assert commands[0][1] is not None and commands[0][1].endswith(
        "working-copy.patch"
    )
    assert "-o" not in commands[0][0]
    assert eden.verify_removed((), "/m/one", ("/t/buck",), (), 10, 10) == ()
    problems = eden.verify_removed(
        ("/m/one",), "/m/one", ("/t/buck",), ("/t/buck",), 10, 99
    )
    assert len(problems) == 3


def test_eden_checkout_safe_proofs(tmp_path: Path) -> None:
    slot = tmp_path / "slot"
    (slot / "checkout").mkdir(parents=True)
    config = wrkslots_cli.Config(
        config_path=tmp_path / ".wrkslots.yml",
        root=tmp_path,
        worktrees=tmp_path,
        control=tmp_path,
        machine="node-a",
        default_remote="origin",
        default_landed_ref="refs/remotes/origin/main",
        heartbeat_ttl_seconds=60,
        liveness_command=tmp_path / "live.py",
    )
    checkout = wrkslots_cli._checkout_from_obj(_eden_obj(), "row.checkouts[0]")

    def clean(argv: Sequence[str]) -> tuple[int, str, str]:
        if "info" in argv:
            return (
                0,
                '{"mount": "/slot/checkout", "state_dir": "/slot/.eden-state/c",'
                f' "backing_repo": "/backing/repo", "checked_out_revision": "{REV}"}}',
                "",
            )
        return (0, "", "")

    assert wrkslots_cli._assert_eden_checkout_safe(config, checkout, clean) == checkout

    def dirty(argv: Sequence[str]) -> tuple[int, str, str]:
        if "status" in argv:
            return (0, "M file\n", "")
        return clean(argv)

    with pytest.raises(wrkslots_cli.Refusal, match="uncommitted Sapling"):
        wrkslots_cli._assert_eden_checkout_safe(config, checkout, dirty)

    def moved(argv: Sequence[str]) -> tuple[int, str, str]:
        if "info" in argv:
            return (
                0,
                '{"mount": "/slot/checkout", "state_dir": "/slot/.eden-state/c",'
                ' "backing_repo": "/backing/repo",'
                f' "checked_out_revision": "{"c" * 40}"}}',
                "",
            )
        return clean(argv)

    # A clean checkout moved to a newer revision is accepted, and
    # the observed revision is what the finish receipt records
    # (#1702: no provisioning-revision pin).
    accepted = wrkslots_cli._assert_eden_checkout_safe(config, checkout, moved)
    assert accepted.head == "c" * 40


def test_trial_record_shape_replays(tmp_path: Path) -> None:
    """Replay of the exact edentrial record main rejected (#1204)."""

    slot_dir = tmp_path / "worktrees" / "validate" / "edentrial"
    (slot_dir / "checkout").mkdir(parents=True)
    config = wrkslots_cli.Config(
        config_path=tmp_path / ".wrkslots.yml",
        root=tmp_path,
        worktrees=tmp_path / "worktrees",
        control=tmp_path / "worktrees",
        machine="node-a",
        default_remote="origin",
        default_landed_ref="refs/remotes/origin/main",
        heartbeat_ttl_seconds=60,
        liveness_command=tmp_path / "live.py",
        layout="flat",
    )
    record = wrkslots_cli.ActiveRecord(
        slot="edentrial",
        agent="mu-eden",
        task="eden-trial",
        purpose="trial",
        slot_type="validate",
        machine="node-a",
        generation=1,
        created_at="2026-11-12T00:00:00Z",
        heartbeat_at="2026-11-12T00:00:00Z",
        heartbeat_ttl_seconds=60,
        owner=None,
        coordinator_lease=wrkslots_cli.ProcessIdentity(
            pid=1, start_ticks=1, boot_id="boot", host_id="host", cgroup_path="/"
        ),
        coordinator_recovery_note=None,
        handoff=None,
        checkouts=(
            wrkslots_cli.Checkout(
                name="main",
                path="worktrees/validate/edentrial/checkout",
                repository="/backing/repo",
                branch=eden.EDEN_BRANCH,
                start_point=REV,
                remote=eden.EDEN_REMOTE,
                remote_url_sha256=DIGEST,
                landed_ref=eden.EDEN_LANDED_REF,
                head=REV,
                containing_remote_refs=(),
                vcs="eden",
            ),
        ),
    )
    parsed = wrkslots_cli._record_from_obj(
        wrkslots_cli._record_to_obj(record), "replay.record"
    )
    assert parsed.checkouts[0].vcs == "eden"
    wrkslots_cli._assert_record_paths(config, parsed)


def test_remove_sequence_cleans_and_stops() -> None:
    calls: list[tuple[str, ...]] = []

    def runner(argv: Sequence[str]) -> tuple[int, str, str]:
        calls.append(tuple(argv))
        if "remove" in argv:
            return (0, "", "")
        if "list" in argv:
            return (0, "/other/mount\n", "")
        if "du" in argv:
            return (0, '{"orphaned_redirections": 10}', "")
        return (0, "", "")

    problems = eden.remove_sequence(
        runner, "/slot/.eden-state", "/slot/checkout", ("/slot/buck",), (), 10
    )
    assert problems == ()
    assert any("stop" in call for call in calls)
    # The slot's own daemon unit is stopped, disabled and verified
    # gone (#1702); no other unit and no shared daemon is named.
    unit = eden.unit_name("/slot/.eden-state")
    assert ("systemctl", "--user", "stop", unit) in calls
    assert ("systemctl", "--user", "disable", unit) in calls
    assert ("systemctl", "--user", "is-active", unit) in calls
    assert unit.startswith("edenfs@") and unit.endswith(".service")
    assert all(
        "/slot/.eden-state" in eden.unit_name("/slot/.eden-state")
        or "eden-state" not in call[-1] or call[-1] == unit
        for call in calls
        if call[:1] == ("systemctl",)
    )

    def failing(argv: Sequence[str]) -> tuple[int, str, str]:
        if "remove" in argv:
            return (1, "", "busy")
        return runner(argv)

    assert eden.remove_sequence(
        failing, "/slot/.eden-state", "/slot/checkout", (), (), 0
    ) == ("eden remove failed: busy",)


def test_provision_clone_info_redirects() -> None:
    calls: list[tuple[str, ...]] = []

    def runner(argv: Sequence[str]) -> tuple[int, str, str]:
        calls.append(tuple(argv))
        if "clone" in argv:
            return (0, "", "")
        if "info" in argv:
            return (
                0,
                '{"mount": "/slot/checkout", "state_dir": "/slot/.eden-state/c",'
                f' "backing_repo": "/backing/repo", "checked_out_revision": "{REV}"}}',
                "",
            )
        return (0, '[{"repo_path": "buck-out", "type": "bind", "target": "/slot/buck"}]', "")

    info, redirects = eden.provision_checkout(
        runner, "/slot/.eden-state", "/backing/repo", "/slot/checkout", None
    )
    assert info.revision == REV and redirects[0].repo_path == "buck-out"
    assert eden.is_inside("/slot/buck", "/slot")
    assert not eden.is_inside("/data/scratch/buck", "/slot")
    assert calls[0][3] == "clone"


def test_storage_consistency_skips_eden_backing(tmp_path: Path) -> None:
    """An Eden row's external backing is not Git drift (#1409).

    The backing repo lives outside the project by design, so the
    storage check must not judge it by the Git sibling rule that
    refused Eden slot removal on devbig030.
    """

    slot_dir = tmp_path / "worktrees" / "validate" / "edentrial"
    (slot_dir / "checkout").mkdir(parents=True)
    config = wrkslots_cli.Config(
        config_path=tmp_path / ".wrkslots.yml",
        root=tmp_path,
        worktrees=tmp_path / "worktrees",
        control=tmp_path / "worktrees",
        machine="node-a",
        default_remote="origin",
        default_landed_ref="refs/remotes/origin/main",
        heartbeat_ttl_seconds=60,
        liveness_command=tmp_path / "live.py",
        layout="flat",
    )
    record = wrkslots_cli.ActiveRecord(
        slot="edentrial",
        agent="mu-eden",
        task="eden-trial",
        purpose="trial",
        slot_type="validate",
        machine="node-a",
        generation=1,
        created_at="2026-11-12T00:00:00Z",
        heartbeat_at="2026-11-12T00:00:00Z",
        heartbeat_ttl_seconds=60,
        owner=None,
        coordinator_lease=wrkslots_cli.ProcessIdentity(
            pid=1, start_ticks=1, boot_id="boot", host_id="host", cgroup_path="/"
        ),
        coordinator_recovery_note=None,
        handoff=None,
        checkouts=(
            wrkslots_cli.Checkout(
                name="main",
                path="worktrees/validate/edentrial/checkout",
                repository="/backing/repo",
                branch=eden.EDEN_BRANCH,
                start_point=REV,
                remote=eden.EDEN_REMOTE,
                remote_url_sha256=DIGEST,
                landed_ref=eden.EDEN_LANDED_REF,
                head=REV,
                containing_remote_refs=(),
                vcs="eden",
            ),
        ),
    )
    # Same proof for a plain-directory (non-image) agent Eden slot:
    # the Git-sibling rule never depended on image slots.
    agent_dir = tmp_path / "worktrees" / "edenagent"
    (agent_dir / "checkout").mkdir(parents=True)
    agent_record = dataclasses.replace(
        record,
        slot="edenagent",
        slot_type="agent",
        checkouts=(
            dataclasses.replace(
                record.checkouts[0], path="worktrees/edenagent/checkout"
            ),
        ),
    )
    state = wrkslots_cli.ActiveState(
        machine="node-a", revision=0, slots=(record, agent_record)
    )
    assert wrkslots_cli._registry_storage_inconsistencies(config, (state,)) == ()


def test_parse_daemon_facts() -> None:
    info = eden.parse_info(
        '{"mount": "/m/one", "state_dir": "/s/one",'
        ' "backing_repo": "/backing/repo",'
        f' "checked_out_revision": "{REV}"}}'
    )
    assert info.state_dir == "/s/one" and info.revision == REV
    redirects = eden.parse_redirections(
        '[{"repo_path": "buck-out", "type": "bind", "target": "/t/buck"}]'
    )
    assert redirects[0].target == "/t/buck"
    assert eden.parse_orphaned_bytes('{"orphaned_redirections": 41}') == 41
    with pytest.raises(eden.EdenError):
        eden.parse_info("{}")


# --------------------------------- absent Eden rows vs git scans (#1554)

import argparse
import os
import shutil
import subprocess


def _git(cwd: Path, *args: str) -> None:
    env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
    env.update(
        {
            "LC_ALL": "C",
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": "/dev/null",
            "GIT_CONFIG_SYSTEM": "/dev/null",
        }
    )
    subprocess.run(
        [
            "git", "-c", "user.email=test@example.invalid", "-c", "user.name=Test",
            "-c", "commit.gpgsign=false", "-C", str(cwd), *args,
        ],
        capture_output=True, text=True, check=True, env=env, timeout=60,
    )


def _absent_project(tmp_path: Path) -> tuple[Path, Path]:
    root = tmp_path / "project"
    (root / "worktrees").mkdir(parents=True)
    (root / "live.py").write_text("# probe\n")
    (root / ".wrkslots.yml").write_text(
        "schema: 2\n"
        "machine: node-a\n"
        "worktrees_dir: worktrees\n"
        "layout: flat\n"
        "default_remote: origin\n"
        "default_landed_ref: refs/remotes/origin/main\n"
        "heartbeat_ttl_seconds: 60\n"
        "liveness_command: live.py\n"
    )
    upstream = tmp_path / "upstream-repo"
    upstream.mkdir()
    _git(upstream, "init", "-q", "-b", "main")
    (upstream / "file.txt").write_text("one\n")
    _git(upstream, "add", "file.txt")
    _git(upstream, "commit", "-q", "-m", "one")
    _git(upstream, "remote", "add", "origin", str(upstream))
    _git(upstream, "fetch", "-q", "origin", "+refs/heads/*:refs/remotes/origin/*")
    return root, upstream


def _eden_validate_record(
    config: wrkslots_cli.Config, backing: str, slot: str = "edenstuck"
) -> wrkslots_cli.ActiveRecord:
    identity = wrkslots_cli._read_process_identity(os.getpid())
    slot_path = wrkslots_cli._slot_directory(config, slot, "validate")
    return wrkslots_cli.ActiveRecord(
        slot=slot,
        agent=f"agent-{slot}",
        task="task-1",
        purpose="testing",
        slot_type="validate",
        machine="node-a",
        generation=1,
        created_at="2026-11-15T00:00:00Z",
        heartbeat_at="2026-11-15T00:00:00Z",
        heartbeat_ttl_seconds=60,
        owner=None,
        coordinator_lease=identity,
        coordinator_recovery_note=None,
        handoff=None,
        checkouts=(
            wrkslots_cli.Checkout(
                name="main",
                path=(slot_path / "checkout").relative_to(config.root).as_posix(),
                repository=backing,
                branch=eden.EDEN_BRANCH,
                start_point=REV,
                remote=eden.EDEN_REMOTE,
                remote_url_sha256=DIGEST,
                landed_ref=eden.EDEN_LANDED_REF,
                head=REV,
                containing_remote_refs=(),
                vcs="eden",
            ),
        ),
    )


def _write_row(config: wrkslots_cli.Config, record: wrkslots_cli.ActiveRecord) -> None:
    wrkslots_cli._ensure_state_shard(config)
    state = wrkslots_cli._load_active(config)
    wrkslots_cli._write_active_state(
        config,
        wrkslots_cli._append_record(state, record),
        action="slot-created",
        slot=record.slot,
        evidence={
            "slot_type": record.slot_type,
            "coordinator_authorized": True,
            "coordinator": wrkslots_cli._identity_to_obj(record.coordinator_lease),
        },
    )


def _git_create_args(root: Path, **overrides: object) -> argparse.Namespace:
    values: dict[str, object] = dict(
        project_root=str(root), machine="node-a", slot="slot-b", slot_type="agent",
        coordinator_authorized=True, agent="agent-new", task="task-1",
        purpose="testing", owner_pid=os.getpid(), coordinator_pid=os.getpid(),
        task_scope_unit=None, task_scope_invocation_id=None,
        repo=["repo=../upstream-repo"], remote=[], remote_url=[],
        branch=["repo=suite/new"], start=[], override_disk_floor=False,
        representation="config", format="human", wait_lock=0.0,
    )
    values.update(overrides)
    return argparse.Namespace(**values)


def test_git_create_tolerates_an_absent_eden_validate_row(tmp_path: Path) -> None:
    root, _upstream = _absent_project(tmp_path)
    config = wrkslots_cli._load_config(str(root), "node-a")
    backing = str(tmp_path / "eden-backing-repos" / "backing")
    _write_row(config, _eden_validate_record(config, backing))

    assert wrkslots_cli._cmd_create(_git_create_args(root)) == 0


def test_create_overlapping_an_eden_slot_still_refuses(tmp_path: Path) -> None:
    root, _upstream = _absent_project(tmp_path)
    config = wrkslots_cli._load_config(str(root), "node-a")
    backing = str(tmp_path / "eden-backing-repos" / "backing")
    _write_row(config, _eden_validate_record(config, backing))

    with pytest.raises(wrkslots_cli.Refusal):
        wrkslots_cli._cmd_create(
            _git_create_args(root, slot="edenstuck", slot_type="validate")
        )


def test_absent_validate_storage_accepts_an_eden_backing(tmp_path: Path) -> None:
    root, _upstream = _absent_project(tmp_path)
    config = wrkslots_cli._load_config(str(root), "node-a")
    backing = str(tmp_path / "eden-backing-repos" / "backing")
    record = _eden_validate_record(config, backing)

    rows = wrkslots_cli._assert_absent_validate_rows_storage(config, [record])

    assert [row.slot for row, _paths in rows] == ["edenstuck"]


def test_absent_validate_storage_refuses_a_git_non_sibling(tmp_path: Path) -> None:
    root, _upstream = _absent_project(tmp_path)
    config = wrkslots_cli._load_config(str(root), "node-a")
    record = _eden_validate_record(
        config, str(tmp_path / "eden-backing-repos" / "backing")
    )
    git_path = (
        wrkslots_cli._slot_directory(config, "edenstuck", "validate")
        .relative_to(config.root)
        .as_posix()
    )
    git_record = dataclasses.replace(
        record,
        checkouts=(
            dataclasses.replace(
                record.checkouts[0], vcs="git", branch="main", path=git_path
            ),
        ),
    )

    with pytest.raises(wrkslots_cli.Refusal, match="direct sibling"):
        wrkslots_cli._assert_absent_validate_rows_storage(config, [git_record])


def test_absent_validate_storage_refuses_present_eden_storage(
    tmp_path: Path,
) -> None:
    root, _upstream = _absent_project(tmp_path)
    config = wrkslots_cli._load_config(str(root), "node-a")
    backing = str(tmp_path / "eden-backing-repos" / "backing")
    record = _eden_validate_record(config, backing)
    wrkslots_cli._slot_directory(config, "edenstuck", "validate").mkdir(parents=True)

    with pytest.raises(wrkslots_cli.Refusal, match="still has physical storage"):
        wrkslots_cli._assert_absent_validate_rows_storage(config, [record])


def test_forget_absent_validate_binds_drops_only_named_slots(
    tmp_path: Path,
) -> None:
    from wrkslots import worktreesimage

    root, _upstream = _absent_project(tmp_path)
    config = wrkslots_cli._load_config(str(root), "node-a")
    record_file = worktreesimage.record_path(config.control)
    record_file.write_text(
        '{"binds": {"edenstuck": "sub-a", "other": "sub-b"}}'
    )

    wrkslots_cli._forget_absent_validate_binds(config, ["edenstuck"])

    assert set(worktreesimage.load_bind_records(config.control)) == {"other"}


# ------------------------------ archived Eden rows (#1554 archive path)


def test_archive_round_trip_validates_an_eden_record(tmp_path: Path) -> None:
    root, _upstream = _absent_project(tmp_path)
    config = wrkslots_cli._load_config(str(root), "node-a")
    backing = str(tmp_path / "eden-backing-repos" / "backing")
    record = _eden_validate_record(config, backing)
    item = wrkslots_cli.AbsentValidateRow(
        machine="node-a",
        slot=record.slot,
        generation=record.generation,
        record_sha256=wrkslots_cli._record_sha256(record),
    )
    entry = wrkslots_cli._absent_validate_archive_entry(
        record, item, "2026-11-15T00:00:00Z"
    )
    archive_path = wrkslots_cli._archive_path(config)
    archive_path.write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 1, "records": [entry]})
    )

    archive = wrkslots_cli._load_archive(config)

    assert [row["slot"] for row in archive.records] == ["edenstuck"]
    assert wrkslots_cli._archive_to_obj(archive)["records"] == [entry]


def _absent_args(root: Path, input_path: Path, **overrides: object) -> argparse.Namespace:
    values: dict[str, object] = dict(
        project_root=str(root), machine="node-a", input=str(input_path),
        apply=False, coordinator_pid=os.getpid(), coordinator_authorized=True,
        format="human", wait_lock=0.0,
    )
    values.update(overrides)
    return argparse.Namespace(**values)


def test_recover_absent_validate_rows_disposes_an_eden_row(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    root, _upstream = _absent_project(tmp_path)
    config = wrkslots_cli._load_config(str(root), "node-a")
    backing = str(tmp_path / "eden-backing-repos" / "backing")
    record = _eden_validate_record(config, backing)
    _write_row(config, record)
    input_path = root / "absent-validate-rows.json"
    input_path.write_text(
        json.dumps(
            {
                "schema": 1,
                "rows": [
                    {
                        "machine": "node-a",
                        "slot": record.slot,
                        "generation": record.generation,
                        "record_sha256": wrkslots_cli._record_sha256(record),
                    }
                ],
            }
        )
    )
    monkeypatch.setattr(wrkslots_cli, "_short_hostname", lambda: "node-a")
    monkeypatch.setattr(wrkslots_cli, "_absent_validate_process_snapshot", lambda: ())
    monkeypatch.setattr(
        wrkslots_cli, "_absent_validate_privileged_path_match", lambda *_a, **_k: None
    )
    monkeypatch.setattr(wrkslots_cli, "_user_systemd_snapshot", lambda: ())

    assert wrkslots_cli._cmd_recover_absent_validate_rows(
        _absent_args(root, input_path)
    ) == 0
    assert "outcome=planned" in capsys.readouterr().out
    assert wrkslots_cli._cmd_recover_absent_validate_rows(
        _absent_args(root, input_path, apply=True)
    ) == 0
    assert "outcome=recovered" in capsys.readouterr().out

    assert wrkslots_cli._load_active(config).slots == ()
    archive = wrkslots_cli._load_archive(config)
    assert [row["slot"] for row in archive.records] == ["edenstuck"]


# ------------------------------------------- create failure cleanup (#1558)


class _StubEdenDaemon:
    """A stubbed eden runner: mounts live per config dir, nothing is real."""

    def __init__(self, redirections: list[dict[str, str]]) -> None:
        self.redirections = redirections
        self.mounts: dict[str, set[str]] = {}
        self.calls: list[tuple[str, ...]] = []
        self.fail_fixup = False
        self.escape_after_fixup = False
        self.wedged = False

    discovery_root: str | None = None
    cloned_mount: str | None = None

    def __call__(self, argv: Sequence[str]) -> tuple[int, str, str]:
        self.calls.append(tuple(argv))
        if argv[0] == "sl":
            return (0, f"{self.discovery_root or self.cloned_mount or ''}\n", "")
        config_dir = argv[2] if len(argv) > 2 and argv[1] == "--config-dir" else None
        rest = list(argv[3:] if config_dir is not None else argv[1:])
        if rest[:1] == ["clone"]:
            mount = rest[2]
            self.cloned_mount = mount
            assert config_dir is not None
            Path(config_dir).mkdir(parents=True, exist_ok=True)
            Path(mount).mkdir(parents=True, exist_ok=True)
            for item in self.redirections:
                (Path(mount) / item["repo_path"]).mkdir(exist_ok=True)
            self.mounts.setdefault(config_dir, set()).add(mount)
            return (0, "", "")
        if rest[:1] == ["info"]:
            mount = rest[1]
            return (
                0,
                json.dumps(
                    {
                        "mount": mount,
                        "state_dir": str(Path(config_dir or "") / "c"),
                        "backing_repo": "/backing/repo",
                        "checked_out_revision": REV,
                    }
                ),
                "",
            )
        if rest[:2] == ["redirect", "list"]:
            return (0, json.dumps(self.redirections), "")
        if rest[:2] == ["redirect", "fixup"]:
            if self.fail_fixup:
                return (1, "", "fixup boom")
            if self.escape_after_fixup:
                for item in self.redirections:
                    derived = Path(item["target"])
                    if derived.is_symlink():
                        derived.unlink()
                        derived.mkdir()
            return (0, "", "")
        if rest[:2] == ["redirect", "unmount"]:
            return (0, "", "")
        if rest[:1] == ["remove"]:
            if not self.wedged and config_dir is not None:
                self.mounts.get(config_dir, set()).discard(rest[-1])
            return (0, "", "")
        if rest[:1] == ["stop"]:
            return (0, "", "")
        if rest[:1] == ["list"]:
            return (0, "".join(f"{m}\n" for m in sorted(self.mounts.get(config_dir or "", ()))), "")
        return (0, "", "")

    def daemon_writes(self) -> list[tuple[str, ...]]:
        """remove/stop calls: the only writes a cleanup may make to a daemon."""
        return [
            call
            for call in self.calls
            if len(call) > 3 and call[3] in {"remove", "stop"}
        ]


def _direct_config(tmp_path: Path) -> wrkslots_cli.Config:
    return wrkslots_cli.Config(
        config_path=tmp_path / ".wrkslots.yml",
        root=tmp_path,
        worktrees=tmp_path,
        control=tmp_path,
        machine="node-a",
        default_remote="origin",
        default_landed_ref="refs/remotes/origin/main",
        heartbeat_ttl_seconds=60,
        liveness_command=tmp_path / "live.py",
        eden_mount_root=tmp_path / "eden-mounts",
    )


def test_retarget_creates_the_missing_derived_parent(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Fresh clone: the derived parent under host scratch does not exist yet."""
    slot = tmp_path / "slot-a"
    derived = tmp_path / "scratch" / "deep" / "buck-out"
    daemon = _StubEdenDaemon(
        [{"repo_path": "buck-out", "type": "bind", "target": str(derived)}]
    )
    monkeypatch.setattr(wrkslots_cli, "_eden_runner", daemon)
    args = argparse.Namespace(
        slot="slot-a", slot_type="agent", eden=["main=/backing/repo"], eden_rev=[]
    )

    checkout, in_slot, derived_targets = wrkslots_cli._provision_eden_slot(
        _direct_config(tmp_path), args, slot
    )

    assert checkout.vcs == "eden"
    assert in_slot == ("buck-out-target",)
    assert derived_targets == (str(derived),)
    assert derived.is_symlink()
    assert os.path.realpath(derived) == str(slot / "buck-out-target")


def test_fixup_failure_stops_only_the_slots_daemon(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    slot = tmp_path / "slot-a"
    derived = tmp_path / "scratch" / "buck-out"
    daemon = _StubEdenDaemon(
        [{"repo_path": "buck-out", "type": "bind", "target": str(derived)}]
    )
    daemon.fail_fixup = True
    monkeypatch.setattr(wrkslots_cli, "_eden_runner", daemon)
    args = argparse.Namespace(
        slot="slot-a", slot_type="agent", eden=["main=/backing/repo"], eden_rev=[]
    )

    with pytest.raises(wrkslots_cli.Refusal, match="redirection fixup failed"):
        wrkslots_cli._provision_eden_slot(_direct_config(tmp_path), args, slot)

    config_dir = str(slot / ".eden-state")
    mount = tmp_path / "eden-mounts" / "agent" / "slot-a"
    assert daemon.daemon_writes() == [
        ("eden", "--config-dir", config_dir, "remove", "-y", str(mount)),
        ("eden", "--config-dir", config_dir, "stop"),
    ]
    assert not slot.exists()
    assert not derived.is_symlink()


def test_escape_refusal_stops_the_daemon(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    slot = tmp_path / "slot-a"
    derived = tmp_path / "scratch" / "buck-out"
    daemon = _StubEdenDaemon(
        [{"repo_path": "buck-out", "type": "bind", "target": str(derived)}]
    )
    daemon.escape_after_fixup = True
    monkeypatch.setattr(wrkslots_cli, "_eden_runner", daemon)
    args = argparse.Namespace(
        slot="slot-a", slot_type="agent", eden=["main=/backing/repo"], eden_rev=[]
    )

    with pytest.raises(wrkslots_cli.Refusal, match="escapes the slot"):
        wrkslots_cli._provision_eden_slot(_direct_config(tmp_path), args, slot)

    assert daemon.daemon_writes()
    assert all(call[2] == str(slot / ".eden-state") for call in daemon.daemon_writes())
    assert not slot.exists()


def test_nonempty_derived_target_is_refused_and_cleaned(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    slot = tmp_path / "slot-a"
    derived = tmp_path / "scratch" / "buck-out"
    derived.mkdir(parents=True)
    (derived / "keep.txt").write_text("evidence\n")
    daemon = _StubEdenDaemon(
        [{"repo_path": "buck-out", "type": "bind", "target": str(derived)}]
    )
    monkeypatch.setattr(wrkslots_cli, "_eden_runner", daemon)
    args = argparse.Namespace(
        slot="slot-a", slot_type="agent", eden=["main=/backing/repo"], eden_rev=[]
    )

    with pytest.raises(wrkslots_cli.Refusal, match="not a fresh empty directory"):
        wrkslots_cli._provision_eden_slot(_direct_config(tmp_path), args, slot)

    assert daemon.daemon_writes()
    assert not slot.exists()
    # Nothing beyond what the create made was deleted.
    assert (derived / "keep.txt").read_text() == "evidence\n"


def _eden_project(tmp_path: Path) -> Path:
    root = tmp_path / "project"
    (root / "worktrees").mkdir(parents=True)
    (root / "live.py").write_text("# probe\n")
    (root / ".wrkslots.yml").write_text(
        "schema: 2\n"
        "machine: node-a\n"
        "worktrees_dir: worktrees\n"
        "layout: flat\n"
        "default_remote: origin\n"
        "default_landed_ref: refs/remotes/origin/main\n"
        "heartbeat_ttl_seconds: 60\n"
        "liveness_command: live.py\n"
        f"eden_mount_root: {tmp_path}/eden-mounts\n"
    )
    return root


def _eden_create_args(root: Path, **overrides: object) -> argparse.Namespace:
    values: dict[str, object] = dict(
        project_root=str(root), machine="node-a", slot="eden-a", slot_type="agent",
        coordinator_authorized=True, agent="agent-eden", task="task-1",
        purpose="testing", owner_pid=os.getpid(), coordinator_pid=os.getpid(),
        task_scope_unit=None, task_scope_invocation_id=None,
        repo=[], remote=[], remote_url=[], branch=[], start=[],
        eden=["main=/backing/repo"], eden_rev=[],
        override_disk_floor=False, representation="config", format="human",
        wait_lock=0.0,
    )
    values.update(overrides)
    return argparse.Namespace(**values)


def _recover_args(root: Path, **overrides: object) -> argparse.Namespace:
    values: dict[str, object] = dict(
        project_root=str(root), machine="node-a",
        coordinator_pid=os.getpid(), wait_lock=0.0, slot="eden-a",
        coordinator_authorized=False, discard_partial=False,
        discard_stale_image_copy=False, legacy_validate_checkout=None,
        ownerless_validate_checkout=None, ownerless_validate_cargo_home=None,
        frozen_validate_checkout=None, completed_record=None, repository=None,
        validation_proof_manifest=None, recovery_note=None,
        abort_import=False, abort_create=True, retry_running_hook=False,
        format="human",
    )
    values.update(overrides)
    return argparse.Namespace(**values)


def test_oserror_mid_loop_is_journaled_then_recover_aborts_it(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _eden_project(tmp_path)
    config = wrkslots_cli._load_config(str(root), "node-a")
    slot_path = wrkslots_cli._slot_directory(config, "eden-a", "agent")
    # The derived parent is a regular file: mkdir parents raises OSError
    # mid-loop, after the clone has started the daemon.
    blocker = tmp_path / "scratch-file"
    blocker.write_text("not a directory\n")
    derived = blocker / "buck-out"
    daemon = _StubEdenDaemon(
        [{"repo_path": "buck-out", "type": "bind", "target": str(derived)}]
    )
    daemon.wedged = True
    other_config = "/other-slot/.eden-state"
    daemon.mounts[other_config] = {"/other-slot/checkout"}
    monkeypatch.setattr(wrkslots_cli, "_eden_runner", daemon)

    with pytest.raises(wrkslots_cli.Refusal, match="Eden cleanup incomplete"):
        wrkslots_cli._cmd_create(_eden_create_args(root))

    journals = sorted(config.control.glob("CREATE.*.journal"))
    assert len(journals) == 1
    fields = wrkslots_cli._parse_eden_create_journal(
        config, wrkslots_cli._as_mapping(
            wrkslots_cli._read_json(journals[0], "eden create journal"),
            "eden create journal",
        )
    )
    assert fields.phase == "cleanup-failed"
    assert fields.config_dir == str(slot_path / ".eden-state")
    assert fields.mount == str(tmp_path / "eden-mounts" / "agent" / "eden-a")
    assert fields.unit.startswith("edenfs@") and fields.unit.endswith(".service")
    printed = capsys.readouterr().err
    assert fields.config_dir in printed and fields.mount in printed
    # Another daemon was never written to.
    assert daemon.daemon_writes()
    assert all(call[2] == fields.config_dir for call in daemon.daemon_writes())
    assert daemon.mounts[other_config] == {"/other-slot/checkout"}

    # A retry over the journaled failure points at recover, not a bare path.
    with pytest.raises(wrkslots_cli.Refusal, match="recover"):
        wrkslots_cli._cmd_create(_eden_create_args(root))

    daemon.wedged = False
    assert wrkslots_cli._cmd_recover(_recover_args(root)) == 0
    assert not slot_path.exists()
    assert sorted(config.control.glob("CREATE.*.journal")) == []
    assert daemon.mounts[other_config] == {"/other-slot/checkout"}


def test_successful_eden_create_registers_and_clears_its_journal(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _eden_project(tmp_path)
    config = wrkslots_cli._load_config(str(root), "node-a")
    derived = tmp_path / "scratch" / "buck-out"
    daemon = _StubEdenDaemon(
        [{"repo_path": "buck-out", "type": "bind", "target": str(derived)}]
    )
    monkeypatch.setattr(wrkslots_cli, "_eden_runner", daemon)

    assert wrkslots_cli._cmd_create(_eden_create_args(root)) == 0

    state = wrkslots_cli._load_active(config)
    assert [record.slot for record in state.slots] == ["eden-a"]
    assert state.slots[0].checkouts[0].vcs == "eden"
    assert sorted(config.control.glob("CREATE.*.journal")) == []
    assert daemon.daemon_writes() == []


# ------------------- absent-batch journal release (#1554)


def test_replayed_absent_batch_journal_disposes_an_eden_row(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    """A journal left by a refused apply replays to completion (#1554A2)."""
    root, _upstream = _absent_project(tmp_path)
    config = wrkslots_cli._load_config(str(root), "node-a")
    backing = str(tmp_path / "eden-backing-repos" / "backing")
    record = _eden_validate_record(config, backing)
    _write_row(config, record)
    input_path = root / "absent-validate-rows.json"
    input_path.write_text(
        json.dumps(
            {
                "schema": 1,
                "rows": [
                    {
                        "machine": "node-a",
                        "slot": record.slot,
                        "generation": record.generation,
                        "record_sha256": wrkslots_cli._record_sha256(record),
                    }
                ],
            }
        )
    )
    monkeypatch.setattr(wrkslots_cli, "_short_hostname", lambda: "node-a")
    monkeypatch.setattr(wrkslots_cli, "_absent_validate_process_snapshot", lambda: ())
    monkeypatch.setattr(
        wrkslots_cli, "_absent_validate_privileged_path_match", lambda *_a, **_k: None
    )
    monkeypatch.setattr(wrkslots_cli, "_user_systemd_snapshot", lambda: ())
    # The journal a refused apply leaves behind, byte for byte.
    input_sha256, requested = wrkslots_cli._read_absent_validate_rows_input(
        config, str(input_path)
    )
    planned = wrkslots_cli._plan_absent_validate_rows(config, requested)
    coordinator = wrkslots_cli._read_process_identity(os.getpid())
    journal = wrkslots_cli._absent_validate_batch_journal(
        config, input_sha256, planned, coordinator
    )
    wrkslots_cli._write_journal(config, journal)
    journal_path = wrkslots_cli._journal_path(config)
    assert journal_path.exists()

    assert wrkslots_cli._cmd_recover_absent_validate_rows(
        _absent_args(root, input_path, apply=True)
    ) == 0

    assert "outcome=recovered" in capsys.readouterr().out
    assert not journal_path.exists()
    assert wrkslots_cli._load_active(config).slots == ()
    assert [row["slot"] for row in wrkslots_cli._load_archive(config).records] == [
        "edenstuck"
    ]

def test_refused_fresh_apply_releases_its_journal(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root, _upstream = _absent_project(tmp_path)
    config = wrkslots_cli._load_config(str(root), "node-a")
    backing = str(tmp_path / "eden-backing-repos" / "backing")
    record = _eden_validate_record(config, backing)
    _write_row(config, record)
    input_path = root / "absent-validate-rows.json"
    input_path.write_text(
        json.dumps(
            {
                "schema": 1,
                "rows": [
                    {
                        "machine": "node-a",
                        "slot": record.slot,
                        "generation": record.generation,
                        "record_sha256": wrkslots_cli._record_sha256(record),
                    }
                ],
            }
        )
    )
    monkeypatch.setattr(wrkslots_cli, "_short_hostname", lambda: "node-a")
    monkeypatch.setattr(wrkslots_cli, "_absent_validate_process_snapshot", lambda: ())
    monkeypatch.setattr(
        wrkslots_cli, "_absent_validate_privileged_path_match", lambda *_a, **_k: None
    )
    monkeypatch.setattr(wrkslots_cli, "_user_systemd_snapshot", lambda: ())

    def refused(*_args: object, **_kwargs: object) -> tuple[object, ...]:
        raise wrkslots_cli.Refusal("nothing was written")

    monkeypatch.setattr(wrkslots_cli, "_recover_absent_validate_rows", refused)
    with pytest.raises(wrkslots_cli.Refusal, match="nothing was written"):
        wrkslots_cli._cmd_recover_absent_validate_rows(
            _absent_args(root, input_path, apply=True)
        )
    assert not wrkslots_cli._journal_path(config).exists()
    assert [row.slot for row in wrkslots_cli._load_active(config).slots] == ["edenstuck"]


# ------------------------------- Eden validate remove flow (#1554B)


class _StubEdenRemoval:
    """Stubbed eden/sl runner for removal: one live mount, clean tree."""

    def __init__(self, mount: Path) -> None:
        self.mount = str(mount)
        self.calls: list[tuple[str, ...]] = []
        self.live = True

    def __call__(self, argv: Sequence[str]) -> tuple[int, str, str]:
        self.calls.append(tuple(argv))
        if argv[0] == "sl":
            if "status" in argv:
                return (0, "", "")
            return (0, REV, "")
        rest = list(argv[3:] if len(argv) > 2 and argv[1] == "--config-dir" else argv[1:])
        if rest[:1] == ["info"]:
            return (
                0,
                json.dumps(
                    {
                        "mount": self.mount,
                        "state_dir": "/state",
                        "backing_repo": "/backing/repo",
                        "checked_out_revision": REV,
                    }
                ),
                "",
            )
        if rest[:2] == ["redirect", "list"]:
            return (0, "[]", "")
        if rest[:1] == ["du"] or "du" in argv:
            return (0, '{"orphaned_redirections": 0}', "")
        if rest[:1] == ["remove"]:
            self.live = False
            shutil.rmtree(self.mount, ignore_errors=True)
            return (0, "", "")
        if rest[:1] == ["list"]:
            return (0, f"{self.mount}\n" if self.live else "", "")
        return (0, "", "")


def test_remove_of_an_eden_validate_slot_uses_no_git(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root, _upstream = _absent_project(tmp_path)
    config = wrkslots_cli._load_config(str(root), "node-a")
    backing = str(tmp_path / "eden-backing-repos" / "backing")
    record = _eden_validate_record(config, backing)
    dead_owner = dataclasses.replace(
        wrkslots_cli._read_process_identity(os.getpid()), pid=2_147_483_647
    )
    record = dataclasses.replace(record, owner=dead_owner)
    _write_row(config, record)
    slot_path = wrkslots_cli._slot_directory(config, "edenstuck", "validate")
    mount = slot_path / "checkout"
    (slot_path / ".eden-state").mkdir(parents=True)
    mount.mkdir()
    (slot_path / "buck-out-target").mkdir()
    daemon = _StubEdenRemoval(mount)
    monkeypatch.setattr(wrkslots_cli, "_eden_runner", daemon)

    assert (
        wrkslots_cli.main(
            [
                "--project-root", str(root),
                "remove", "edenstuck",
                "--coordinator-pid", str(os.getpid()),
                "--expected-generation", "1",
            ]
        )
        == 0
    )

    assert wrkslots_cli._load_active(config).slots == ()
    assert [row["slot"] for row in wrkslots_cli._load_archive(config).records] == [
        "edenstuck"
    ]
    assert not slot_path.exists()
    config_dir = str(slot_path / ".eden-state")
    assert ("eden", "--config-dir", config_dir, "remove", "-y", str(mount)) in daemon.calls
    assert ("eden", "--config-dir", config_dir, "stop") in daemon.calls


def test_not_mounted_mounts_parsing() -> None:
    stdout = "/m/one\n/m/two (not mounted)\n\n/m/three (not mounted)\n"

    assert eden.not_mounted_mounts(stdout) == ("/m/two", "/m/three")
    assert eden.not_mounted_mounts("/m/one\n") == ()


def test_peer_unmount_order_is_deepest_first_within_the_shared_group() -> None:
    mountinfo = "\n".join(
        [
            "1 0 0:1 / /host/mnt ro shared:14071 - fuse.edenfs edenfs rw",
            "2 0 0:1 / /host/mnt/sub ro shared:14071 - fuse.edenfs edenfs rw",
            "3 0 0:1 / /host/other/deep/nest ro shared:14071 - fuse.edenfs edenfs rw",
            "4 0 0:1 / /host/unrelated ro shared:99 - fuse.edenfs edenfs rw",
            "5 0 0:1 / /host/mnt ro shared:14071 - ext4 /dev/sda rw",
        ]
    )
    entries = slotimage._parse_mount_table(mountinfo)

    order = wrkslots_cli._eden_peer_unmount_order("/host/mnt", entries)

    assert order == ("/host/other/deep/nest", "/host/mnt/sub", "/host/mnt")
    # Without a shared peer id there are no proven peers.
    bare = slotimage._parse_mount_table(
        "1 0 0:1 / /host/mnt ro - fuse.edenfs edenfs rw"
    )
    assert wrkslots_cli._eden_peer_unmount_order("/host/mnt", bare) == ()


def _fake_peer_table(
    monkeypatch: pytest.MonkeyPatch, mountinfo: str
) -> list[list[str]]:
    entries = slotimage._parse_mount_table(mountinfo)
    monkeypatch.setattr(
        slotimage, "verification_table", lambda: entries
    )
    calls: list[list[str]] = []

    def fake_run(argv: Sequence[str]) -> str:
        calls.append(list(argv))
        return ""

    monkeypatch.setattr(slotimage, "_run_on_host", fake_run)
    return calls


def test_peer_outside_the_managed_root_refuses_and_unmounts_nothing(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    calls = _fake_peer_table(
        monkeypatch,
        "\n".join(
            [
                "1 0 0:1 / /managed/mnt ro shared:14071 - fuse.edenfs edenfs rw",
                "2 0 0:1 / /elsewhere/view ro shared:14071 - fuse.edenfs edenfs rw",
            ]
        ),
    )

    with pytest.raises(wrkslots_cli.Refusal, match="/elsewhere/view"):
        wrkslots_cli._unmount_eden_peer_group("/managed/mnt", ("/managed",))

    assert calls == []


def test_peer_group_under_the_root_unmounts_deepest_first(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    calls = _fake_peer_table(
        monkeypatch,
        "\n".join(
            [
                "1 0 0:1 / /managed/mnt ro shared:14071 - fuse.edenfs edenfs rw",
                "2 0 0:1 / /managed/mnt/sub ro shared:14071 - fuse.edenfs edenfs rw",
                "3 0 0:1 / /managed/a/b/c ro shared:14071 - fuse.edenfs edenfs rw",
            ]
        ),
    )

    wrkslots_cli._unmount_eden_peer_group("/managed/mnt", ("/managed",))

    assert calls == [
        ["umount", "/managed/a/b/c"],
        ["umount", "/managed/mnt/sub"],
        ["umount", "/managed/mnt"],
    ]


class _StubEdenNotMounted:
    """Eden already removed the checkout: list shows it (not mounted)."""

    def __init__(self, mount: Path) -> None:
        self.mount = str(mount)
        self.calls: list[tuple[str, ...]] = []
        self.live = True

    def __call__(self, argv: Sequence[str]) -> tuple[int, str, str]:
        self.calls.append(tuple(argv))
        if argv[0] == "sl":
            return (1, "", f"repository {self.mount} not found")
        rest = list(argv[3:] if len(argv) > 2 and argv[1] == "--config-dir" else argv[1:])
        if rest[:2] == ["redirect", "list"]:
            return (0, "[]", "")
        if rest[:1] == ["du"] or "du" in argv:
            return (0, '{"orphaned_redirections": 0}', "")
        if rest[:1] == ["remove"]:
            self.live = False
            shutil.rmtree(self.mount, ignore_errors=True)
            return (0, "", "")
        if rest[:1] == ["list"]:
            if self.live:
                return (0, f"{self.mount} (not mounted)\n", "")
            return (0, "", "")
        return (0, "", "")


def test_remove_resumes_when_eden_lists_the_checkout_not_mounted(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    config = _direct_config(tmp_path)
    slot_path = wrkslots_cli._slot_directory(config, "eden-a", "agent")
    mount = config.eden_mount_root / "agent" / "eden-a"  # type: ignore[operator]
    identity = wrkslots_cli._read_process_identity(os.getpid())
    record = wrkslots_cli.ActiveRecord(
        slot="eden-a",
        agent="agent-eden-a",
        task="task-1",
        purpose="testing",
        slot_type="agent",
        machine="node-a",
        generation=1,
        created_at="2026-11-15T00:00:00Z",
        heartbeat_at="2026-11-15T00:00:00Z",
        heartbeat_ttl_seconds=60,
        owner=None,
        coordinator_lease=identity,
        coordinator_recovery_note=None,
        handoff=None,
        checkouts=(
            wrkslots_cli.Checkout(
                name="main",
                path=str(mount),
                repository="/backing/repo",
                branch=eden.EDEN_BRANCH,
                start_point=REV,
                remote=eden.EDEN_REMOTE,
                remote_url_sha256=DIGEST,
                landed_ref=eden.EDEN_LANDED_REF,
                head=REV,
                containing_remote_refs=(),
                vcs="eden",
            ),
        ),
    )
    (slot_path / ".eden-state").mkdir(parents=True)
    mount.mkdir(parents=True)
    _write_row(config, record)
    daemon = _StubEdenNotMounted(mount)
    monkeypatch.setattr(wrkslots_cli, "_eden_runner", daemon)
    unmounted: list[str] = []
    monkeypatch.setattr(
        wrkslots_cli,
        "_unmount_eden_peer_group",
        lambda mount, roots: unmounted.append(mount),
    )
    journal: dict[str, object] = {
        "removed": [],
        "removing": ["main"],
    }
    journal_path = tmp_path / "finish.journal"

    result = wrkslots_cli._finish_remove_paths(
        config,
        record,
        journal,
        wrkslots_cli._GitVcs(),
        wrkslots_cli._FinishContext(mode="remove", actor="coordinator"),
        journal_path=journal_path,
    )

    assert result["removed"] == ["main"]
    assert not any(call[0] == "sl" for call in daemon.calls)
    assert not slot_path.exists()
    assert unmounted == [str(mount)]


# ------------------------------------------- discovery guard (#1555)


def test_discovery_mismatch_refuses_create_and_rolls_back(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _eden_project(tmp_path)
    config = wrkslots_cli._load_config(str(root), "node-a")
    slot_path = wrkslots_cli._slot_directory(config, "eden-a", "agent")
    derived = tmp_path / "scratch" / "buck-out"
    daemon = _StubEdenDaemon(
        [{"repo_path": "buck-out", "type": "bind", "target": str(derived)}]
    )
    daemon.discovery_root = str(slot_path.parent)
    monkeypatch.setattr(wrkslots_cli, "_eden_runner", daemon)

    with pytest.raises(wrkslots_cli.Refusal, match="discovery guard"):
        wrkslots_cli._cmd_create(_eden_create_args(root))

    assert daemon.daemon_writes()
    assert not slot_path.exists()
    assert sorted(config.control.glob("CREATE.*.journal")) == []


def test_doctor_flags_a_mismatched_eden_mount(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _eden_project(tmp_path)
    config = wrkslots_cli._load_config(str(root), "node-a")
    derived = tmp_path / "scratch" / "buck-out"
    daemon = _StubEdenDaemon(
        [{"repo_path": "buck-out", "type": "bind", "target": str(derived)}]
    )
    monkeypatch.setattr(wrkslots_cli, "_eden_runner", daemon)
    assert wrkslots_cli._cmd_create(_eden_create_args(root)) == 0
    states = [wrkslots_cli._load_active(config)]

    assert wrkslots_cli._eden_discovery_doctor_findings(config, states) == []

    daemon.discovery_root = str(root)
    findings = wrkslots_cli._eden_discovery_doctor_findings(config, states)
    assert len(findings) == 1
    assert findings[0]["kind"] == "eden-discovery-mismatch"


def test_partition_splits_outer_in_image_and_dedupes_aliases(
    tmp_path: Path,
) -> None:
    image = tmp_path / "image"
    (image / "s1").mkdir(parents=True)
    alias = tmp_path / "alias"
    alias.symlink_to(image, target_is_directory=True)
    mounts = (
        "/host/one",
        str(image / "s1" / "checkout"),
        str(alias / "s1" / "checkout"),
    )

    outer, in_image = eden.partition_checkouts(mounts, (str(image),))

    assert outer == ("/host/one",)
    assert in_image == (str(image / "s1" / "checkout"),)


def test_cap_ignores_in_image_and_refuses_at_outer_cap() -> None:
    outer = ("/host/one", "/host/two")
    in_image = ("/img/a/checkout", "/img/b/checkout")
    eden.assert_under_cap(outer, 3, in_image)

    with pytest.raises(eden.EdenError) as caught:
        eden.assert_under_cap((*outer, "/host/three"), 3, in_image)

    message = str(caught.value)
    assert "outer" in message and "/host/one" in message
    assert "in-image not counted" in message and "/img/a/checkout" in message


def _image_config(tmp_path: Path, **overrides: object) -> wrkslots_cli.Config:
    from wrkslots import worktreesimage

    values: dict[str, object] = dict(
        config_path=tmp_path / ".wrkslots.yml",
        root=tmp_path,
        worktrees=tmp_path / "slots",
        control=tmp_path / "ctrl",
        machine="node-a",
        default_remote="origin",
        default_landed_ref="refs/remotes/origin/main",
        heartbeat_ttl_seconds=60,
        liveness_command=tmp_path / "live.py",
        worktrees_image=worktreesimage.WorktreesImageSettings(
            relative_path="worktrees.img", size_bytes=1024**3
        ),
        worktrees_image_state=worktreesimage.MOUNTED,
    )
    values.update(overrides)
    return wrkslots_cli.Config(**values)  # type: ignore[arg-type]


def test_eden_image_roots_cover_mount_binds_and_validate(
    tmp_path: Path,
) -> None:
    from wrkslots import worktreesimage

    config = _image_config(tmp_path)
    worktreesimage.save_bind_records(
        config.control,
        {
            "s1": worktreesimage.BindRecord(
                subvolume="s1", target=str(config.worktrees / "s1")
            )
        },
    )

    roots = wrkslots_cli._eden_image_roots(config)

    assert str(worktreesimage.image_mount_dir(config.control)) in roots
    assert str(worktreesimage.validate_external_dir(config.worktrees)) in roots
    assert str(config.worktrees / "s1") in roots


def test_eden_image_roots_fail_safe_when_unknown(tmp_path: Path) -> None:
    from wrkslots import worktreesimage

    assert wrkslots_cli._eden_image_roots(
        _image_config(tmp_path, worktrees_image=None)
    ) == ()
    assert wrkslots_cli._eden_image_roots(
        _image_config(tmp_path, worktrees_image_state=worktreesimage.NOT_MOUNTED)
    ) == ()
    config = _image_config(tmp_path)
    config.control.mkdir(parents=True)
    worktreesimage.record_path(config.control).write_text("not json")
    assert wrkslots_cli._eden_image_roots(config) == ()


def test_eden_cap_report_counts_both(tmp_path: Path) -> None:
    config = _image_config(tmp_path)
    image = config.control / ".slots-image"
    runner = _fake_runner(
        {"/state-a": (0, f"/host/one\n{image}/s1/checkout\n", "")}
    )

    report = wrkslots_cli._eden_cap_report(
        config, runner=runner, cmdlines=(), known=("/state-a",)
    )

    assert report == {
        "cap": 3,
        "outer": 1,
        "in_image": 1,
        "total": 2,
        "outer_mounts": ["/host/one"],
        "in_image_mounts": [f"{image}/s1/checkout"],
    }


def test_partition_counts_an_outside_mount_by_its_in_image_state_dir(
    tmp_path: Path,
) -> None:
    image = tmp_path / "image"
    state = image / "validate" / "s1" / ".eden-state"
    state.mkdir(parents=True)
    mounts = ("/mounts/validate/s1", "/host/one", "/host/two")
    state_dirs = {
        "/mounts/validate/s1": str(state),
        "/host/one": "/home/u/local/.eden",
    }

    outer, in_image = eden.partition_checkouts(
        mounts, (str(image),), state_dirs=state_dirs
    )

    assert in_image == ("/mounts/validate/s1",)
    assert outer == ("/host/one", "/host/two")
    # Without state dirs the same outside mount is judged by path alone.
    assert eden.partition_checkouts(mounts, (str(image),)) == (mounts, ())


def test_checkouts_by_state_dir_pairs_and_dedupes() -> None:
    runner = _fake_runner(
        {
            "/state-a": (0, "/m/one\n/m/two\n", ""),
            "/state-b": (0, "/m/two\n/m/three\n", ""),
        }
    )

    pairs = eden.checkouts_by_state_dir(runner, ("/state-a", "/state-b"))

    assert pairs == (
        ("/m/one", "/state-a"),
        ("/m/two", "/state-a"),
        ("/m/three", "/state-b"),
    )
    assert eden.count_checkouts(runner, ("/state-a", "/state-b")) == (
        "/m/one",
        "/m/two",
        "/m/three",
    )


def test_eden_cap_report_counts_an_outside_mount_with_slot_state_in_image(
    tmp_path: Path,
) -> None:
    from wrkslots import worktreesimage

    config = _image_config(tmp_path)
    validate = worktreesimage.validate_external_dir(config.worktrees)
    slot_state = validate / "s1" / ".eden-state"
    slot_state.mkdir(parents=True)
    outside_mount = str(tmp_path / "eden-mounts" / "validate" / "s1")
    runner = _fake_runner(
        {
            "/state-a": (0, "/host/one\n", ""),
            str(slot_state): (0, f"{outside_mount}\n", ""),
        }
    )

    report = wrkslots_cli._eden_cap_report(
        config, runner=runner, cmdlines=(), known=("/state-a", str(slot_state))
    )

    assert report["outer_mounts"] == ["/host/one"]
    assert report["in_image_mounts"] == [outside_mount]
    assert (report["outer"], report["in_image"], report["total"]) == (1, 1, 2)


def test_slot_view_accepts_an_eden_slot_without_git_directories(
    tmp_path: Path,
) -> None:
    from wrkslots import imagecmd

    root, _upstream = _absent_project(tmp_path)
    config = wrkslots_cli._load_config(str(root), "node-a")
    backing = str(tmp_path / "eden-backing-repos" / "backing")
    record = _eden_validate_record(config, backing)
    slot_path = wrkslots_cli._slot_directory(config, record.slot, "validate")
    (slot_path / "checkout").mkdir(parents=True)
    _write_row(config, record)

    view = imagecmd.slot_view(config, record.slot, "validate")

    assert view.slot_path == slot_path
    assert view.git_directories == ()


def test_image_convert_refuses_an_eden_slot(tmp_path: Path) -> None:
    from wrkslots import imagecmd

    root, _upstream = _absent_project(tmp_path)
    config = wrkslots_cli._load_config(str(root), "node-a")
    backing = str(tmp_path / "eden-backing-repos" / "backing")
    record = _eden_validate_record(config, backing)

    with pytest.raises(wrkslots_cli.Refusal, match="Eden"):
        imagecmd._verify_checkouts(config, record)


def _fake_backing(root: Path, *, default: str | None = "fb:source-repo") -> Path:
    (root / ".hg" / "store").mkdir(parents=True)
    (root / ".hg" / "requires").write_text("remotefilelog\nstore\n")
    (root / ".hg" / "store" / "requires").write_text("lazychangelog\n")
    (root / ".hg" / "store" / "data.bin").write_bytes(b"x" * 100)
    lines = "" if default is None else f"[paths]\ndefault = {default}\n"
    (root / ".hg" / "hgrc").write_text(lines)
    return root


def test_backing_repo_problem_variants(tmp_path: Path) -> None:
    missing = tmp_path / "absent"
    assert "not seeded" in (eden.backing_repo_problem(missing) or "")
    plain = tmp_path / "plain"
    plain.mkdir()
    assert "not a Sapling backing repo" in (eden.backing_repo_problem(plain) or "")
    good = _fake_backing(tmp_path / "good")
    assert eden.backing_repo_problem(good) is None
    (good / ".hg" / "sharedpath").write_text("/elsewhere\n")
    assert "shared clone" in (eden.backing_repo_problem(good) or "")


def test_backing_facts_report_lazy_store_and_default(tmp_path: Path) -> None:
    good = _fake_backing(tmp_path / "good")
    facts = eden.backing_facts(good)
    assert facts["lazy"] is True
    assert facts["default_path"] == "fb:source-repo"
    assert "lazychangelog" in facts["requires"]  # type: ignore[operator]
    total, files = eden.measure_tree(good)
    assert total >= 100 and files >= 4


def test_seed_clone_argv_and_default_rewrite(tmp_path: Path) -> None:
    source = _fake_backing(tmp_path / "source")
    staging = tmp_path / "staging"
    cachepath = tmp_path / "image" / ".hgcache"
    calls: list[tuple[str, ...]] = []

    def run(argv: Sequence[str]) -> tuple[int, str, str]:
        calls.append(tuple(argv))
        if tuple(argv[:2]) == ("sl", "clone"):
            _fake_backing(Path(argv[-1]), default=str(source))
            return 0, "", ""
        return 0, "a" * 40 + "\n", ""

    eden.seed_backing_repo(run, source, staging, cachepath)

    # Lazy: clones the source's upstream, never the source checkout.
    assert calls[0] == ("sl", "clone", "-U", "fb:source-repo", str(staging))
    assert calls[1][:3] == ("sl", "-R", str(staging))
    facts = eden.backing_facts(staging)
    assert facts["default_path"] == "fb:source-repo"
    hgrc = (staging / ".hg" / "hgrc").read_text(encoding="utf-8")
    assert f"cachepath = {cachepath}" in hgrc
    assert (source / ".hg" / "hgrc").read_text(encoding="utf-8").find("cachepath") < 0


def test_seed_refuses_when_clone_stays_pointed_at_source(tmp_path: Path) -> None:
    source = _fake_backing(tmp_path / "source", default=None)
    staging = tmp_path / "staging"

    def run(argv: Sequence[str]) -> tuple[int, str, str]:
        if tuple(argv[:2]) == ("sl", "clone"):
            _fake_backing(Path(argv[-1]), default=str(source))
            return 0, "", ""
        return 0, "a" * 40 + "\n", ""

    with pytest.raises(eden.EdenError, match="no \\[paths\\] default"):
        eden.seed_backing_repo(run, source, staging, tmp_path / "cache")


def test_seed_refuses_a_local_default_path(tmp_path: Path) -> None:
    source = _fake_backing(tmp_path / "source", default="/host/backing")
    staging = tmp_path / "staging"

    def run(argv: Sequence[str]) -> tuple[int, str, str]:
        return 0, "", ""

    with pytest.raises(eden.EdenError, match="local path"):
        eden.seed_backing_repo(run, source, staging, tmp_path / "cache")


def test_create_eden_refuses_backing_mismatch_before_writing(
    tmp_path: Path,
) -> None:
    from wrkslots import worktreesimage

    config = wrkslots_cli.Config(
        config_path=tmp_path / ".wrkslots.yml",
        root=tmp_path,
        worktrees=tmp_path / "slots",
        control=tmp_path / "ctrl",
        machine="node-a",
        default_remote="origin",
        default_landed_ref="refs/remotes/origin/main",
        heartbeat_ttl_seconds=60,
        liveness_command=tmp_path / "live.py",
        worktrees_image=worktreesimage.WorktreesImageSettings(
            relative_path="worktrees.img",
            size_bytes=1024**3,
            eden_backing_repos=(
                worktreesimage.EdenBacking("source-repo", "source-repo"),
            ),
        ),
        worktrees_image_state=worktreesimage.MOUNTED,
    )
    args = argparse.Namespace(eden=["main=/elsewhere/repo"], eden_rev=[])
    with pytest.raises(wrkslots_cli.Refusal, match="eden_backing_repo"):
        wrkslots_cli._eden_create_plan(config, args)
    args = argparse.Namespace(eden=["main=source-repo"], eden_rev=[])
    with pytest.raises(wrkslots_cli.Refusal, match="not seeded"):
        wrkslots_cli._eden_create_plan(config, args)


def test_seed_refuses_to_choose_among_several_backings(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from wrkslots import imagecmd, worktreesimage

    config = _image_config(
        tmp_path,
        worktrees_image=worktreesimage.WorktreesImageSettings(
            relative_path="worktrees.img",
            size_bytes=1024**3,
            eden_backing_repos=(
                worktreesimage.EdenBacking("alpha", "alpha"),
                worktreesimage.EdenBacking("beta", "beta"),
            ),
        ),
    )
    monkeypatch.setattr(
        imagecmd, "_wt_settings", lambda args: (config, config.worktrees_image)
    )
    args = argparse.Namespace(
        name=None, source="/host/backing", yes=False, format="human"
    )
    with pytest.raises(wrkslots_cli.Refusal, match="several eden backings"):
        imagecmd._cmd_wt_seed_eden_backing(args)
    args = argparse.Namespace(
        name="gamma", source="/host/backing", yes=False, format="human"
    )
    with pytest.raises(wrkslots_cli.Refusal, match="no eden backing named"):
        imagecmd._cmd_wt_seed_eden_backing(args)


def test_create_eden_selects_among_configured_backings(tmp_path: Path) -> None:
    from wrkslots import worktreesimage

    config = wrkslots_cli.Config(
        config_path=tmp_path / ".wrkslots.yml",
        root=tmp_path,
        worktrees=tmp_path / "slots",
        control=tmp_path / "ctrl",
        machine="node-a",
        default_remote="origin",
        default_landed_ref="refs/remotes/origin/main",
        heartbeat_ttl_seconds=60,
        liveness_command=tmp_path / "live.py",
        worktrees_image=worktreesimage.WorktreesImageSettings(
            relative_path="worktrees.img",
            size_bytes=1024**3,
            eden_backing_repos=(
                worktreesimage.EdenBacking("alpha", "alpha"),
                worktreesimage.EdenBacking("beta", "team/beta"),
            ),
        ),
        worktrees_image_state=worktreesimage.MOUNTED,
    )
    args = argparse.Namespace(eden=["main=gamma"], eden_rev=[])
    with pytest.raises(wrkslots_cli.Refusal, match="alpha.*beta"):
        wrkslots_cli._eden_create_plan(config, args)
    args = argparse.Namespace(eden=["main=beta"], eden_rev=[])
    with pytest.raises(wrkslots_cli.Refusal, match="not seeded"):
        wrkslots_cli._eden_create_plan(config, args)


# ------------------------- Eden mount outside the project (#1730)

def _eden_project_with_mount(tmp_path: Path) -> tuple[Path, Path]:
    root = tmp_path / "project"
    (root / "worktrees").mkdir(parents=True)
    (root / "live.py").write_text("# probe\n")
    mount_root = tmp_path / "eden-mounts"
    (root / ".wrkslots.yml").write_text(
        "schema: 2\n"
        "machine: node-a\n"
        "worktrees_dir: worktrees\n"
        "layout: flat\n"
        "default_remote: origin\n"
        "default_landed_ref: refs/remotes/origin/main\n"
        "heartbeat_ttl_seconds: 60\n"
        "liveness_command: live.py\n"
        f"eden_mount_root: {mount_root}\n"
    )
    return root, mount_root


def test_create_eden_mounts_outside_the_project(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root, mount_root = _eden_project_with_mount(tmp_path)
    config = wrkslots_cli._load_config(str(root), "node-a")
    slot_path = wrkslots_cli._slot_directory(config, "eden-a", "agent")
    derived = tmp_path / "scratch" / "buck-out"
    daemon = _StubEdenDaemon(
        [{"repo_path": "buck-out", "type": "bind", "target": str(derived)}]
    )
    monkeypatch.setattr(wrkslots_cli, "_eden_runner", daemon)

    assert wrkslots_cli._cmd_create(_eden_create_args(root)) == 0

    mount = mount_root / "agent" / "eden-a"
    state = wrkslots_cli._load_active(config)
    assert [record.slot for record in state.slots] == ["eden-a"]
    checkout = state.slots[0].checkouts[0]
    assert checkout.path == str(mount)
    assert not str(mount).startswith(str(root) + "/")
    assert slot_path.is_dir() and (slot_path / ".eden-state").is_dir()
    wrkslots_cli._assert_record_paths(config, state.slots[0])
    assert daemon.cloned_mount == str(mount)
    assert sorted(config.control.glob("CREATE.*.journal")) == []


def test_eden_mount_root_inside_the_project_refuses(tmp_path: Path) -> None:
    root = tmp_path / "project"
    (root / "worktrees").mkdir(parents=True)
    (root / "live.py").write_text("# probe\n")
    (root / ".wrkslots.yml").write_text(
        "schema: 2\n"
        "machine: node-a\n"
        "worktrees_dir: worktrees\n"
        "layout: flat\n"
        "default_remote: origin\n"
        "default_landed_ref: refs/remotes/origin/main\n"
        "heartbeat_ttl_seconds: 60\n"
        "liveness_command: live.py\n"
        f"eden_mount_root: {root}/eden-mounts\n"
    )
    with pytest.raises((wrkslots_cli.StateError, wrkslots_cli.Refusal)):
        wrkslots_cli._load_config(str(root), "node-a")


def test_record_paths_refuse_an_eden_path_outside_the_mount_root(
    tmp_path: Path,
) -> None:
    root, _mount_root = _eden_project_with_mount(tmp_path)
    config = wrkslots_cli._load_config(str(root), "node-a")
    record = _eden_validate_record(config, "/backing/repo")
    outside = dataclasses.replace(
        record,
        checkouts=(
            dataclasses.replace(
                record.checkouts[0], path="/elsewhere/checkout"
            ),
        ),
    )
    with pytest.raises(wrkslots_cli.StateError):
        wrkslots_cli._assert_record_paths(
            config, outside, require_repository=False
        )


def test_remove_of_an_outside_mount_eden_validate_slot(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root, mount_root = _eden_project_with_mount(tmp_path)
    config = wrkslots_cli._load_config(str(root), "node-a")
    record = _eden_validate_record(config, "/backing/repo")
    mount = mount_root / "validate" / record.slot
    record = dataclasses.replace(
        record,
        checkouts=(
            dataclasses.replace(record.checkouts[0], path=str(mount)),
        ),
    )
    dead_owner = dataclasses.replace(
        wrkslots_cli._read_process_identity(os.getpid()), pid=2_147_483_647
    )
    record = dataclasses.replace(record, owner=dead_owner)
    _write_row(config, record)
    slot_path = wrkslots_cli._slot_directory(config, record.slot, "validate")
    (slot_path / ".eden-state").mkdir(parents=True)
    mount.mkdir(parents=True)
    daemon = _StubEdenRemoval(mount)
    monkeypatch.setattr(wrkslots_cli, "_eden_runner", daemon)

    assert (
        wrkslots_cli.main(
            [
                "--project-root", str(root),
                "remove", record.slot,
                "--coordinator-pid", str(os.getpid()),
                "--expected-generation", "1",
            ]
        )
        == 0
    )

    assert wrkslots_cli._load_active(config).slots == ()
    assert not slot_path.exists()
    assert not mount.exists()
    config_dir = str(slot_path / ".eden-state")
    assert ("eden", "--config-dir", config_dir, "remove", "-y", str(mount)) in daemon.calls


def test_classify_drafts_counts_landed_and_unaccounted() -> None:
    """Sapling drafts classify from sl evidence alone (#1702)."""
    landed_node = "a" * 40
    open_node = "b" * 40
    loose_node = "c" * 40

    def runner(argv: Sequence[str]) -> tuple[int, str, str]:
        args = list(argv)
        if "draft()" in args:
            return (0, f"{landed_node}\n{open_node}\n{loose_node}\n", "")
        if "public()" in args:
            return (0, "landed work\nDifferential Revision: D123\n", "")
        if "bookmark()" in args:
            return (0, f"{open_node}\n", "")
        if landed_node in args:
            return (0, "landed work\nDifferential Revision: D123\n", "")
        if open_node in args:
            return (0, "on review\nDifferential Revision: D999\n", "")
        return (0, "loose work\n", "")

    entry = eden.classify_drafts(runner, "/slot/checkout", "main", "d" * 40)
    assert entry["checkout"] == "main"
    assert [node for node, _s in entry["landed"]] == [landed_node]  # type: ignore[attr-defined]
    assert [node for node, _s in entry["open"]] == [open_node]  # type: ignore[attr-defined]
    assert [node for node, _s in entry["unaccounted"]] == [loose_node]  # type: ignore[attr-defined]


def test_finish_accounting_counts_eden_drafts(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Finish accounting for an Eden slot counts Sapling drafts (#1702)."""
    slot = tmp_path / "slot"
    (slot / "checkout").mkdir(parents=True)
    config = wrkslots_cli.Config(
        config_path=tmp_path / ".wrkslots.yml",
        root=tmp_path,
        worktrees=tmp_path,
        control=tmp_path,
        machine="node-a",
        default_remote="origin",
        default_landed_ref="refs/remotes/origin/main",
        heartbeat_ttl_seconds=60,
        liveness_command=tmp_path / "live.py",
    )
    checkout = wrkslots_cli._checkout_from_obj(_eden_obj(), "row.checkouts[0]")
    landed_node = "a" * 40

    def runner(argv: Sequence[str]) -> tuple[int, str, str]:
        args = list(argv)
        if "info" in args:
            return (
                0,
                '{"mount": "/slot/checkout", "state_dir": "/slot/.eden-state/c",'
                f' "backing_repo": "/backing/repo", "checked_out_revision": "{REV}"}}',
                "",
            )
        if "status" in args:
            return (0, "", "")
        if "draft()" in args:
            return (0, f"{landed_node}\n", "")
        if "public()" in args:
            return (0, "landed\nDifferential Revision: D7\n", "")
        if "bookmark()" in args:
            return (0, "", "")
        return (0, "landed\nDifferential Revision: D7\n", "")

    monkeypatch.setattr(wrkslots_cli, "_eden_runner", runner)
    sink: list[dict[str, object]] = []
    safe = wrkslots_cli._assert_checkout_safe(
        config, checkout, wrkslots_cli._GitVcs(),
        accounting=True, accounting_sink=sink,
    )
    assert safe.head == REV
    assert len(sink) == 1
    entry = sink[0]
    assert [node for node, _s in entry["landed"]] == [landed_node]  # type: ignore[attr-defined]
    assert entry["unaccounted"] == []


# --------------------------------- per-checkout drafts (#1925)


def test_classify_drafts_scoped_to_own_nodes() -> None:
    """Scoped classification never sees another checkout's draft."""
    own_node = "a" * 40
    other_node = "b" * 40
    revsets: list[str] = []

    def runner(argv: Sequence[str]) -> tuple[int, str, str]:
        args = list(argv)
        if "-r" in args:
            revset = args[args.index("-r") + 1]
            revsets.append(revset)
            if revset.startswith("successors("):
                return (0, f"{own_node}\n", "")
            if revset == "draft()":
                return (0, f"{own_node}\n{other_node}\n", "")
            if revset.startswith("public() &"):
                return (0, "", "")
            if revset == "public()":
                return (0, "", "")
            if revset == "bookmark()":
                return (0, "", "")
            return (0, "own loose work\n", "")
        return (0, "own loose work\n", "")

    entry = eden.classify_drafts(
        runner, "/slot/checkout", "main", "d" * 40, own_nodes=(own_node,)
    )
    assert [node for node, _s in entry["unaccounted"]] == [own_node]  # type: ignore[attr-defined]
    assert entry["landed"] == [] and entry["open"] == []
    assert "draft()" not in revsets


def test_classify_drafts_scoped_follows_amend_successor() -> None:
    """A recorded draft is classified at its amended successor."""
    old_node = "c" * 40
    new_node = "e" * 40

    def runner(argv: Sequence[str]) -> tuple[int, str, str]:
        args = list(argv)
        if "-r" in args:
            revset = args[args.index("-r") + 1]
            if revset.startswith("successors("):
                assert old_node in revset
                return (0, f"{new_node}\n", "")
            if revset.startswith("public() &"):
                return (0, "", "")
            if revset == "public()":
                return (0, "", "")
            if revset == "bookmark()":
                return (0, "", "")
            return (0, "amended loose work\n", "")
        return (0, "amended loose work\n", "")

    entry = eden.classify_drafts(
        runner, "/slot/checkout", "main", new_node, own_nodes=(old_node,)
    )
    assert [node for node, _s in entry["unaccounted"]] == [new_node]  # type: ignore[attr-defined]


def test_draft_ledger_round_trip_and_refusals() -> None:
    ledger = eden.DraftLedger(
        slot="s1", checkout="main", generation=2,
        nodes=("a" * 40, "b" * 40), captured_at="2026-11-19T00:00:00+00:00",
        last_error=None,
    )
    parsed = eden.draft_ledger_from_obj(eden.draft_ledger_payload(ledger), "ledger")
    assert parsed == ledger
    bad = eden.draft_ledger_payload(ledger)
    bad["nodes"] = ["b" * 40, "a" * 40]
    with pytest.raises(eden.EdenError, match="sorted"):
        eden.draft_ledger_from_obj(bad, "ledger")
    bad = eden.draft_ledger_payload(ledger)
    bad["nodes"] = ["not-a-node"]
    with pytest.raises(eden.EdenError, match="full Sapling"):
        eden.draft_ledger_from_obj(bad, "ledger")


def _scoped_config(tmp_path: Path) -> wrkslots_cli.Config:
    (tmp_path / "slot" / "checkout").mkdir(parents=True)
    return wrkslots_cli.Config(
        config_path=tmp_path / ".wrkslots.yml",
        root=tmp_path,
        worktrees=tmp_path,
        control=tmp_path,
        machine="node-a",
        default_remote="origin",
        default_landed_ref="refs/remotes/origin/main",
        heartbeat_ttl_seconds=60,
        liveness_command=tmp_path / "live.py",
    )


def _scoped_runner(own_node: str, other_node: str) -> eden.Runner:
    def runner(argv: Sequence[str]) -> tuple[int, str, str]:
        args = list(argv)
        if "info" in args:
            return (
                0,
                '{"mount": "/slot/checkout", "state_dir": "/slot/.eden-state/c",'
                f' "backing_repo": "/backing/repo", "checked_out_revision": "{REV}"}}',
                "",
            )
        if "status" in args:
            return (0, "", "")
        if "-r" in args:
            revset = args[args.index("-r") + 1]
            if revset == "draft() & ::.":
                return (0, f"{own_node}\n", "")
            if revset.startswith("successors("):
                return (0, f"{own_node}\n", "")
            if revset == "draft()":
                return (0, f"{own_node}\n{other_node}\n", "")
            if revset.startswith("public() &"):
                return (0, "", "")
            if revset == "public()":
                return (0, "", "")
            if revset == "bookmark()":
                return (0, "", "")
            return (0, "own loose work\n", "")
        return (0, "own loose work\n", "")

    return runner


def test_finish_accounting_scoped_by_ledger(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """With a ledger, finish accounts only this checkout's drafts."""
    config = _scoped_config(tmp_path)
    checkout = wrkslots_cli._checkout_from_obj(_eden_obj(), "row.checkouts[0]")
    own_node, other_node = "c" * 40, "e" * 40
    ledger_path = wrkslots_cli._eden_draft_ledger_path(config, checkout)
    wrkslots_cli._write_eden_draft_ledger(
        ledger_path,
        eden.DraftLedger(
            slot="slot", checkout="main", generation=1, nodes=(own_node,),
            captured_at="2026-11-19T00:00:00+00:00", last_error=None,
        ),
    )
    monkeypatch.setattr(
        wrkslots_cli, "_eden_runner", _scoped_runner(own_node, other_node)
    )
    sink: list[dict[str, object]] = []
    wrkslots_cli._assert_checkout_safe(
        config, checkout, wrkslots_cli._GitVcs(),
        accounting=True, accounting_sink=sink,
        eden_slot="slot", eden_generation=1,
    )
    assert len(sink) == 1
    entry = sink[0]
    assert [node for node, _s in entry["unaccounted"]] == [own_node]  # type: ignore[attr-defined]
    assert other_node not in str(entry)


def test_finish_accounting_without_ledger_refuses_naming_capture(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A pre-#1925 checkout fails closed, naming capture-drafts."""
    config = _scoped_config(tmp_path)
    checkout = wrkslots_cli._checkout_from_obj(_eden_obj(), "row.checkouts[0]")
    monkeypatch.setattr(
        wrkslots_cli, "_eden_runner", _scoped_runner("c" * 40, "e" * 40)
    )
    sink: list[dict[str, object]] = []
    with pytest.raises(wrkslots_cli.Refusal, match=r"capture-drafts slot"):
        wrkslots_cli._assert_checkout_safe(
            config, checkout, wrkslots_cli._GitVcs(),
            accounting=True, accounting_sink=sink,
            eden_slot="slot", eden_generation=1,
        )
    assert sink == []


def test_capture_ledger_create_update_and_no_seed(tmp_path: Path) -> None:
    """Capture unions stacks, never seeds silently, records failures."""
    config = _scoped_config(tmp_path)
    checkout = wrkslots_cli._checkout_from_obj(_eden_obj(), "row.checkouts[0]")
    path = wrkslots_cli._eden_draft_ledger_path(config, checkout)
    stack = ["a" * 40]

    def runner(argv: Sequence[str]) -> tuple[int, str, str]:
        return (0, "".join(f"{node}\n" for node in stack), "")

    assert wrkslots_cli._capture_eden_draft_ledger(
        config, checkout, slot="slot", generation=1, runner=runner, create=False
    ) is None
    assert not path.exists()
    created = wrkslots_cli._capture_eden_draft_ledger(
        config, checkout, slot="slot", generation=1, runner=runner, create=True
    )
    assert created is not None and created.nodes == ("a" * 40,)
    stack[:] = ["b" * 40]
    updated = wrkslots_cli._capture_eden_draft_ledger(
        config, checkout, slot="slot", generation=1, runner=runner, create=False
    )
    assert updated is not None and updated.nodes == ("a" * 40, "b" * 40)

    def failing(argv: Sequence[str]) -> tuple[int, str, str]:
        return (1, "", "daemon down")

    with pytest.raises(eden.EdenError, match="daemon down"):
        wrkslots_cli._capture_eden_draft_ledger(
            config, checkout, slot="slot", generation=1, runner=failing,
            create=False,
        )
    ledger = wrkslots_cli._load_eden_draft_ledger(
        path, slot="slot", checkout=checkout, generation=1
    )
    assert ledger is not None
    assert ledger.nodes == ("a" * 40, "b" * 40)
    assert ledger.last_error is not None and "daemon down" in ledger.last_error


def test_classify_drafts_names_hidden_own_node() -> None:
    """A hidden own draft stays unaccounted and is named as hidden."""
    node = "a" * 40

    def runner(argv: Sequence[str]) -> tuple[int, str, str]:
        args = list(argv)
        if "--hidden" in args:
            return (0, f"{node}\nhidden work\n", "")
        revset = args[args.index("-r") + 1] if "-r" in args else ""
        if revset.startswith("successors(present("):
            return (0, "", "")
        if revset == "public()" or revset == "bookmark()":
            return (0, "", "")
        return (0, "", "")

    entry = eden.classify_drafts(runner, "/m", "main", node, own_nodes=(node,))
    assert entry["hidden"] == [(node, "hidden work")]
    assert [n for n, _s in entry["unaccounted"]] == [node]  # type: ignore[attr-defined]
    refusal = wrkslots_cli._accounting_refusal("s1", [entry])
    assert f"sl unhide {node}" in refusal


def test_classify_drafts_unknown_node_refuses_with_rebuild() -> None:
    """A ledger node the store does not know refuses, naming repair."""
    node = "a" * 40

    def runner(argv: Sequence[str]) -> tuple[int, str, str]:
        # present() on an unknown node exits 0 with no output.
        return (0, "", "")

    with pytest.raises(eden.EdenError, match="not known to the store"):
        eden.classify_drafts(runner, "/m", "main", node, own_nodes=(node,))


def test_classify_drafts_public_without_revision_is_landed() -> None:
    """A public ledger node counts landed even without a D-revision."""
    node = "a" * 40

    def runner(argv: Sequence[str]) -> tuple[int, str, str]:
        args = list(argv)
        revset = args[args.index("-r") + 1] if "-r" in args else ""
        if revset.startswith("successors(present("):
            return (0, f"{node}\n", "")
        if revset.startswith("public() &"):
            return (0, f"{node}\n", "")
        if revset == "public()" or revset == "bookmark()":
            return (0, "", "")
        return (0, "landed silently\n", "")

    entry = eden.classify_drafts(runner, "/m", "main", node, own_nodes=(node,))
    assert [n for n, _s in entry["landed"]] == [node]  # type: ignore[attr-defined]
    assert entry["unaccounted"] == []


def test_classify_drafts_labels_foreign_ancestor() -> None:
    """An unaccounted node by another author is labelled foreign."""
    node = "a" * 40

    def runner(argv: Sequence[str]) -> tuple[int, str, str]:
        args = list(argv)
        revset = args[args.index("-r") + 1] if "-r" in args else ""
        if revset.startswith("successors(present("):
            return (0, f"{node}\n", "")
        if revset.startswith("public() &"):
            return (0, "", "")
        if revset == "public()" or revset == "bookmark()":
            return (0, "", "")
        if "{author}" in args:
            return (0, "Other Lane <other@example.invalid>", "")
        return (0, "foreign base\n", "")

    entry = eden.classify_drafts(
        runner, "/m", "main", node, own_nodes=(node,),
        own_author="Own Lane <own@example.invalid>",
    )
    assert [n for n, _s in entry["unaccounted"]] == [node]  # type: ignore[attr-defined]
    assert [n for n, _s in entry["foreign"]] == [node]  # type: ignore[attr-defined]
    refusal = wrkslots_cli._accounting_refusal("s1", [entry])
    assert "ancestor not authored by this checkout" in refusal


def test_capture_ledger_concurrent_captures_union(tmp_path: Path) -> None:
    """Overlapping captures under the ledger flock lose no node."""
    import threading

    config = _scoped_config(tmp_path)
    checkout = wrkslots_cli._checkout_from_obj(_eden_obj(), "row.checkouts[0]")
    wrkslots_cli._capture_eden_draft_ledger(
        config, checkout, slot="slot", generation=1,
        runner=lambda argv: (0, "", ""), create=True,
    )
    nodes = [f"{index:040x}" for index in range(8)]
    errors: list[BaseException] = []

    def capture(node: str) -> None:
        try:
            wrkslots_cli._capture_eden_draft_ledger(
                config, checkout, slot="slot", generation=1,
                runner=lambda argv: (0, f"{node}\n", ""), create=False,
            )
        except BaseException as exc:  # noqa: BLE001 - collected and asserted
            errors.append(exc)

    threads = [threading.Thread(target=capture, args=(node,)) for node in nodes]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
    assert errors == []
    ledger = wrkslots_cli._load_eden_draft_ledger(
        wrkslots_cli._eden_draft_ledger_path(config, checkout),
        slot="slot", checkout=checkout, generation=1,
    )
    assert ledger is not None and ledger.nodes == tuple(sorted(nodes))


class _EdenDraftStub:
    """Eden daemon stub plus a scripted Sapling store for CLI tests."""

    def __init__(self) -> None:
        self.daemon = _StubEdenDaemon([])
        self.stack: list[str] = []
        self.candidates: list[tuple[str, str, int, str]] = []
        self.public_nodes: set[str] = set()
        self.hidden_nodes: set[str] = set()
        self.subjects: dict[str, str] = {}
        self.authors: dict[str, str] = {}

    def __call__(self, argv: Sequence[str]) -> tuple[int, str, str]:
        args = list(argv)
        if args[0] == "eden":
            return self.daemon(argv)
        if "root" in args:
            return self.daemon(argv)
        if "status" in args:
            return (0, "", "")
        revset = args[args.index("-r") + 1] if "-r" in args else ""
        template = args[args.index("-T") + 1] if "-T" in args else ""
        if revset == "draft() & ::.":
            return (0, "".join(f"{node}\n" for node in self.stack), "")
        if revset == "draft() - ::.":
            return (0, "".join(
                f"{node}|{author}|{when} +0000|{subject}\n"
                for node, author, when, subject in self.candidates
            ), "")
        if "--hidden" in args:
            import re as _re
            found = _re.findall(r"[0-9a-f]{40}", revset)
            node = found[0] if found else ""
            if node not in self.hidden_nodes:
                return (1, "", "hidden")
            if revset.startswith("successors("):
                return (0, f"{node}|1|{self.subjects.get(node, '')}\n", "")
            return (0, f"{node}\n{self.subjects.get(node, '')}", "")
        if revset.startswith("successors(present("):
            node = revset[len("successors(present("):-2]
            if node in self.hidden_nodes:
                return (0, "", "")
            return (0, f"{node}\n" if node in self.subjects else "", "")
        if revset.startswith("public() &"):
            node = revset.rsplit("(", 1)[-1][:-1]
            return (0, f"{node}\n" if node in self.public_nodes else "", "")
        if revset in ("public()", "bookmark()"):
            return (0, "", "")
        if template == "{author}":
            return (0, self.authors.get(revset, ""), "")
        if template == "{desc}":
            return (0, self.subjects.get(revset, ""), "")
        return (0, "", "")


def _cli_args(root: Path, *argv: str) -> argparse.Namespace:
    parser = wrkslots_cli._build_parser()
    return parser.parse_args(
        ["--project-root", str(root), "--machine", "node-a", *argv]
    )


def test_eden_cli_create_heartbeat_finish_and_migration(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """End to end: create, heartbeat, capture, finish (#1925 D1)."""
    root = _eden_project(tmp_path)
    config = wrkslots_cli._load_config(str(root), "node-a")
    stub = _EdenDraftStub()
    monkeypatch.setattr(wrkslots_cli, "_eden_runner", stub)
    own, abandoned, foreign = "c" * 40, "d" * 40, "e" * 40
    stub.subjects = {own: "own work", abandoned: "abandoned own stack",
                     foreign: "other lane work"}
    stub.authors = {own: "Lane <lane@example.invalid>",
                    abandoned: "Lane <lane@example.invalid>",
                    foreign: "Other <other@example.invalid>"}

    assert wrkslots_cli._cmd_create(_eden_create_args(root)) == 0
    record = wrkslots_cli._load_active(config).slots[0]
    ledger_path = wrkslots_cli._eden_draft_ledger_path(config, record.checkouts[0])
    ledger = wrkslots_cli._load_eden_draft_ledger(
        ledger_path, slot="eden-a", checkout=record.checkouts[0], generation=1
    )
    assert ledger is not None and ledger.generation == 1 and ledger.nodes == ()

    # The lane works; heartbeat captures its stack.
    stub.stack = [own]
    heartbeat = _cli_args(
        root, "heartbeat", "eden-a", "--agent", "agent-eden",
        "--owner-pid", str(os.getpid()), "--expected-generation", "1",
    )
    assert heartbeat.handler(heartbeat) == 0
    ledger = wrkslots_cli._load_eden_draft_ledger(
        ledger_path, slot="eden-a", checkout=record.checkouts[0], generation=1
    )
    assert ledger is not None and ledger.nodes == (own,)

    def finish() -> argparse.Namespace:
        return _cli_args(
            root, "finish", "eden-a", "--agent", "agent-eden",
            "--owner-pid", str(os.getpid()), "--expected-generation", "1",
            "--validation", "e2e test",
        )

    args = finish()
    with pytest.raises(wrkslots_cli.Refusal) as refused:
        args.handler(args)
    assert own in str(refused.value) and foreign not in str(refused.value)

    # Migration: a pre-change slot has no ledger and two stacks off
    # its current one. First capture lists and refuses; the operator
    # includes the abandoned own stack and only that enters.
    ledger_path.unlink()
    stub.candidates = [
        (abandoned, stub.authors[abandoned], 4_000_000_000, "abandoned own stack"),
        (foreign, stub.authors[foreign], 4_000_000_000, "other lane work"),
    ]
    capture = _cli_args(
        root, "capture-drafts", "eden-a", "--agent", "agent-eden",
        "--owner-pid", str(os.getpid()), "--expected-generation", "1",
    )
    with pytest.raises(wrkslots_cli.Refusal) as listed:
        capture.handler(capture)
    assert abandoned in str(listed.value) and foreign in str(listed.value)
    assert not ledger_path.exists()
    capture = _cli_args(
        root, "capture-drafts", "eden-a", "--agent", "agent-eden",
        "--owner-pid", str(os.getpid()), "--expected-generation", "1",
        "--include", abandoned, "--disown", foreign,
    )
    assert capture.handler(capture) == 0
    ledger = wrkslots_cli._load_eden_draft_ledger(
        ledger_path, slot="eden-a", checkout=record.checkouts[0], generation=1
    )
    assert ledger is not None
    assert set(ledger.nodes) == {own, abandoned} and foreign not in ledger.nodes
    assert [node for node, _by, _at in ledger.disowned] == [foreign]

    # Once the own drafts are public, finish passes end to end.
    stub.public_nodes = {own, abandoned}
    args = finish()
    assert args.handler(args) == 0


def test_capture_drafts_needs_owner_and_rebuild_repairs_corrupt(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """capture-drafts refuses strangers; --rebuild repairs corruption."""
    root = _eden_project(tmp_path)
    config = wrkslots_cli._load_config(str(root), "node-a")
    stub = _EdenDraftStub()
    monkeypatch.setattr(wrkslots_cli, "_eden_runner", stub)
    assert wrkslots_cli._cmd_create(_eden_create_args(root)) == 0
    record = wrkslots_cli._load_active(config).slots[0]
    ledger_path = wrkslots_cli._eden_draft_ledger_path(config, record.checkouts[0])

    anonymous = _cli_args(root, "capture-drafts", "eden-a")
    with pytest.raises(wrkslots_cli.Refusal, match="needs its owner"):
        anonymous.handler(anonymous)

    ledger_path.write_text("{corrupt", encoding="utf-8")
    with pytest.raises(wrkslots_cli.Refusal, match="malformed JSON"):
        wrkslots_cli._eden_scoped_draft_nodes(
            config, record.checkouts[0], slot="eden-a", generation=1,
            runner=stub,
        )
    rebuild = _cli_args(
        root, "capture-drafts", "eden-a", "--agent", "agent-eden",
        "--owner-pid", str(os.getpid()), "--expected-generation", "1",
        "--rebuild",
    )
    with pytest.raises(wrkslots_cli.Refusal, match="unreadable"):
        rebuild.handler(rebuild)
    rebuild = _cli_args(
        root, "capture-drafts", "eden-a", "--agent", "agent-eden",
        "--owner-pid", str(os.getpid()), "--expected-generation", "1",
        "--rebuild", "--acknowledge-unreadable-ledger",
    )
    assert rebuild.handler(rebuild) == 0
    ledger = wrkslots_cli._load_eden_draft_ledger(
        ledger_path, slot="eden-a", checkout=record.checkouts[0], generation=1
    )
    assert ledger is not None and ledger.nodes == ()


def test_capture_rebuild_carries_hidden_and_generation_change(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """--rebuild never drops a recorded (hidden) node; generation is free."""
    root = _eden_project(tmp_path)
    config = wrkslots_cli._load_config(str(root), "node-a")
    stub = _EdenDraftStub()
    monkeypatch.setattr(wrkslots_cli, "_eden_runner", stub)
    hidden = "d" * 40
    stub.subjects = {hidden: "hidden own work"}
    assert wrkslots_cli._cmd_create(_eden_create_args(root)) == 0
    record = wrkslots_cli._load_active(config).slots[0]
    ledger_path = wrkslots_cli._eden_draft_ledger_path(config, record.checkouts[0])
    stub.stack = [hidden]
    heartbeat = _cli_args(
        root, "heartbeat", "eden-a", "--agent", "agent-eden",
        "--owner-pid", str(os.getpid()), "--expected-generation", "1",
    )
    assert heartbeat.handler(heartbeat) == 0

    # A generation bump (transfer-owner, supersede) must not strand
    # automatic captures: the ledger is per slot and checkout.
    captured = wrkslots_cli._capture_eden_draft_ledger(
        config, record.checkouts[0], slot="eden-a", generation=7,
        runner=stub, create=False,
    )
    assert captured is not None and captured.nodes == (hidden,)

    stub.stack = []
    stub.hidden_nodes = {hidden}
    rebuild = _cli_args(
        root, "capture-drafts", "eden-a", "--agent", "agent-eden",
        "--owner-pid", str(os.getpid()), "--expected-generation", "1",
        "--rebuild",
    )
    assert rebuild.handler(rebuild) == 0
    ledger = wrkslots_cli._load_eden_draft_ledger(
        ledger_path, slot="eden-a", checkout=record.checkouts[0], generation=1
    )
    assert ledger is not None and ledger.nodes == (hidden,)

    disown = _cli_args(
        root, "capture-drafts", "eden-a", "--agent", "agent-eden",
        "--owner-pid", str(os.getpid()), "--expected-generation", "1",
        "--rebuild", "--disown", hidden,
    )
    with pytest.raises(wrkslots_cli.Refusal, match="never dropped silently"):
        disown.handler(disown)
    ledger = wrkslots_cli._load_eden_draft_ledger(
        ledger_path, slot="eden-a", checkout=record.checkouts[0], generation=1
    )
    assert ledger is not None and ledger.nodes == (hidden,)


def test_capture_first_lists_older_drafts_and_records_disown(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Drafts older than the slot are listed, judged and recorded."""
    root = _eden_project(tmp_path)
    config = wrkslots_cli._load_config(str(root), "node-a")
    stub = _EdenDraftStub()
    monkeypatch.setattr(wrkslots_cli, "_eden_runner", stub)
    old = "d" * 40
    stub.subjects = {old: "backdated own work"}
    stub.authors = {old: "Lane <lane@example.invalid>"}
    assert wrkslots_cli._cmd_create(_eden_create_args(root)) == 0
    record = wrkslots_cli._load_active(config).slots[0]
    ledger_path = wrkslots_cli._eden_draft_ledger_path(config, record.checkouts[0])
    ledger_path.unlink()
    stub.candidates = [(old, stub.authors[old], 1_000_000, "backdated own work")]

    capture = _cli_args(
        root, "capture-drafts", "eden-a", "--agent", "agent-eden",
        "--owner-pid", str(os.getpid()), "--expected-generation", "1",
    )
    with pytest.raises(wrkslots_cli.Refusal) as listed:
        capture.handler(capture)
    assert old in str(listed.value) and "date=1970-01-12" in str(listed.value)
    assert not ledger_path.exists()

    capture = _cli_args(
        root, "capture-drafts", "eden-a", "--agent", "agent-eden",
        "--owner-pid", str(os.getpid()), "--expected-generation", "1",
        "--disown", old,
    )
    assert capture.handler(capture) == 0
    ledger = wrkslots_cli._load_eden_draft_ledger(
        ledger_path, slot="eden-a", checkout=record.checkouts[0], generation=1
    )
    assert ledger is not None and ledger.nodes == ()
    assert [(n, b) for n, b, _a in ledger.disowned] == [(old, "agent-eden")]


def test_capture_after_real_transfer_owner(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A real transfer-owner generation bump strands no capture."""
    root = _eden_project(tmp_path)
    config = wrkslots_cli._load_config(str(root), "node-a")
    stub = _EdenDraftStub()
    monkeypatch.setattr(wrkslots_cli, "_eden_runner", stub)
    node = "c" * 40
    stub.subjects = {node: "successor work"}
    sleeper = subprocess.Popen(["sleep", "30"])
    try:
        assert wrkslots_cli._cmd_create(
            _eden_create_args(root, owner_pid=sleeper.pid)
        ) == 0
    finally:
        sleeper.terminate()
        sleeper.wait()
    transfer = _cli_args(
        root, "transfer-owner", "eden-a", "--agent", "agent-eden",
        "--owner-pid", str(os.getpid()), "--expected-generation", "1",
    )
    assert transfer.handler(transfer) == 0
    record = wrkslots_cli._load_active(config).slots[0]

    stub.stack = [node]
    heartbeat = _cli_args(
        root, "heartbeat", "eden-a", "--agent", "agent-eden",
        "--owner-pid", str(os.getpid()),
        "--expected-generation", str(record.generation),
    )
    assert heartbeat.handler(heartbeat) == 0
    ledger = wrkslots_cli._load_eden_draft_ledger(
        wrkslots_cli._eden_draft_ledger_path(config, record.checkouts[0]),
        slot="eden-a", checkout=record.checkouts[0],
        generation=record.generation,
    )
    assert ledger is not None and ledger.nodes == (node,)


def test_capture_none_needs_shown_list_and_never_covers_recorded(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """--none-of-these-are-mine: two-step, and never over recorded."""
    root = _eden_project(tmp_path)
    config = wrkslots_cli._load_config(str(root), "node-a")
    stub = _EdenDraftStub()
    monkeypatch.setattr(wrkslots_cli, "_eden_runner", stub)
    own, foreign = "c" * 40, "e" * 40
    stub.subjects = {own: "own work", foreign: "other lane work"}
    stub.authors = {own: "Lane <l@example.invalid>",
                    foreign: "Other <o@example.invalid>"}
    assert wrkslots_cli._cmd_create(_eden_create_args(root)) == 0
    record = wrkslots_cli._load_active(config).slots[0]
    ledger_path = wrkslots_cli._eden_draft_ledger_path(config, record.checkouts[0])
    ledger_path.unlink()
    stub.candidates = [(foreign, stub.authors[foreign], 4_000_000_000,
                        "other lane work")]

    def capture(*extra: str) -> argparse.Namespace:
        return _cli_args(
            root, "capture-drafts", "eden-a", "--agent", "agent-eden",
            "--owner-pid", str(os.getpid()), "--expected-generation", "1",
            *extra,
        )

    first = capture("--none-of-these-are-mine")
    with pytest.raises(wrkslots_cli.Refusal, match="has now been shown") as shown:
        first.handler(first)
    # The refusal that writes the shown-list marker prints every
    # candidate node id, so the rerun can never disown unseen work.
    assert foreign in str(shown.value)
    assert not ledger_path.exists()
    second = capture("--none-of-these-are-mine")
    assert second.handler(second) == 0
    ledger = wrkslots_cli._load_eden_draft_ledger(
        ledger_path, slot="eden-a", checkout=record.checkouts[0], generation=1
    )
    assert ledger is not None and ledger.nodes == ()
    assert [n for n, _b, _a in ledger.disowned] == [foreign]

    # A recorded draft is never covered by the blanket: it needs
    # its own per-node --disown, so the blanket run refuses.
    stub.stack = [own]
    stub.candidates = [(own, stub.authors[own], 4_000_000_000, "own work")]
    repeat = capture()
    assert repeat.handler(repeat) == 0
    stub.stack = []
    blanket = capture("--rebuild", "--none-of-these-are-mine")
    with pytest.raises(wrkslots_cli.Refusal, match="Judge every listed"):
        blanket.handler(blanket)
    ledger = wrkslots_cli._load_eden_draft_ledger(
        ledger_path, slot="eden-a", checkout=record.checkouts[0], generation=1
    )
    assert ledger is not None and ledger.nodes == (own,)


def test_draft_ledger_tmp_leftover_names_manual_remedy(tmp_path: Path) -> None:
    """A crashed capture's .tmp file names its real remedy."""
    config = _scoped_config(tmp_path)
    checkout = wrkslots_cli._checkout_from_obj(_eden_obj(), "row.checkouts[0]")
    path = wrkslots_cli._eden_draft_ledger_path(config, checkout)
    path.parent.mkdir(parents=True, exist_ok=True)
    (path.parent / f"{path.name}.tmp.123").write_text("partial")
    with pytest.raises(wrkslots_cli.StateError, match="remove that .tmp file by hand"):
        wrkslots_cli._write_eden_draft_ledger(
            path,
            eden.DraftLedger(slot="slot", checkout="main", generation=1,
                             nodes=(), captured_at=None, last_error=None),
        )


def test_hidden_query_failure_refuses_not_drops() -> None:
    """A failed hidden query is an error, never an unknown node."""

    def runner(argv: Sequence[str]) -> tuple[int, str, str]:
        return (1, "", "daemon boom")

    with pytest.raises(eden.EdenError, match="hidden query failed"):
        eden.hidden_node_subject(runner, "/m", "a" * 40)


def test_scoped_journal_expansion_failure_refuses(tmp_path: Path) -> None:
    """A failed journal expansion refuses finish, dropping nothing."""
    config = _scoped_config(tmp_path)
    checkout = wrkslots_cli._checkout_from_obj(_eden_obj(), "row.checkouts[0]")
    full = "f" * 40
    ledger_path = wrkslots_cli._eden_draft_ledger_path(config, checkout)
    wrkslots_cli._write_eden_draft_ledger(
        ledger_path,
        eden.DraftLedger(slot="slot", checkout="main", generation=1,
                         nodes=(), captured_at=None, last_error=None),
    )

    def runner(argv: Sequence[str]) -> tuple[int, str, str]:
        args = list(argv)
        if "journal" in args:
            return (0, "aaaaaaaaaaaa -> bbbbbbbbbbbb someone\n", "")
        if "--hidden" in args:
            return (0, f"{full}\n", "")
        revset = args[args.index("-r") + 1] if "-r" in args else ""
        if revset == "draft() & ::.":
            return (0, "", "")
        if revset == "draft()":
            return (0, f"{full}\n", "")
        if revset.startswith("successors(present("):
            return (1, "", "expansion boom")
        return (0, "", "")

    with pytest.raises(wrkslots_cli.Refusal, match="expansion boom"):
        wrkslots_cli._eden_scoped_draft_nodes(
            config, checkout, slot="slot", generation=1, runner=runner
        )


def test_scoped_journal_failure_names_how_to_proceed(tmp_path: Path) -> None:
    """An unreadable Sapling journal refuses with the way forward."""
    config = _scoped_config(tmp_path)
    checkout = wrkslots_cli._checkout_from_obj(_eden_obj(), "row.checkouts[0]")
    ledger_path = wrkslots_cli._eden_draft_ledger_path(config, checkout)
    wrkslots_cli._write_eden_draft_ledger(
        ledger_path,
        eden.DraftLedger(slot="slot", checkout="main", generation=1,
                         nodes=(), captured_at=None, last_error=None),
    )

    def runner(argv: Sequence[str]) -> tuple[int, str, str]:
        args = list(argv)
        if "journal" in args:
            return (1, "", "journal unavailable")
        revset = args[args.index("-r") + 1] if "-r" in args else ""
        if revset == "draft() & ::.":
            return (0, "", "")
        return (0, "", "")

    with pytest.raises(
        wrkslots_cli.Refusal,
        match=r"make 'sl -R .* journal'.*then rerun finish",
    ):
        wrkslots_cli._eden_scoped_draft_nodes(
            config, checkout, slot="slot", generation=1, runner=runner
        )


def test_real_sl_shared_store_scopes_drafts(tmp_path: Path) -> None:
    """Real Sapling: two working copies, one backing store (#1925).

    The sharedpath construction gives both working copies the same
    store (the Eden shape), so store-wide draft() leaks the other
    copy's draft while the per-checkout stack and successors do not.
    """
    sl = shutil.which("sl")
    if sl is None:
        pytest.skip("sl is not available")

    home = tmp_path / "home"
    home.mkdir()
    env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
    env["HOME"] = str(home)

    def run(*args: str) -> str:
        result = subprocess.run(
            [sl, "--config", "ui.username=Test <t@example.invalid>", *args],
            capture_output=True, text=True, env=env, timeout=60,
        )
        assert result.returncode == 0, result.stderr
        return result.stdout.strip()

    def runner(argv: Sequence[str]) -> tuple[int, str, str]:
        result = subprocess.run(
            [sl, "--config", "ui.username=Test <t@example.invalid>",
             *[str(item) for item in argv[1:]]],
            capture_output=True, text=True, env=env, timeout=60,
        )
        return (result.returncode, result.stdout, result.stderr)

    backing = tmp_path / "backing"
    backing.mkdir()
    run("init", str(backing))
    (backing / "a.txt").write_text("a\n")
    run("-R", str(backing), "add", str(backing / "a.txt"))
    run("-R", str(backing), "commit", "-m", "public base")
    copies: dict[str, Path] = {}
    for name in ("wc1", "wc2"):
        copy = tmp_path / name
        run("clone", str(backing), str(copy))
        shutil.rmtree(copy / ".hg" / "store")
        (copy / ".hg" / "sharedpath").write_text(str(backing / ".hg") + "\n")
        copies[name] = copy
    nodes: dict[str, str] = {}
    for name, filename in (("wc1", "b.txt"), ("wc2", "c.txt")):
        copy = copies[name]
        (copy / filename).write_text(f"{name}\n")
        run("-R", str(copy), "add", str(copy / filename))
        run("-R", str(copy), "commit", "-m", f"{name} draft")
        nodes[name] = run("-R", str(copy), "log", "-r", ".", "-T", "{node}")

    wc1, wc2 = str(copies["wc1"]), str(copies["wc2"])
    # The defect, reproduced: the shared store leaks wc2's draft.
    assert set(eden.all_draft_nodes(runner, wc1)) >= {nodes["wc1"], nodes["wc2"]}
    # The per-checkout stack does not.
    assert nodes["wc1"] in eden.draft_stack_nodes(runner, wc1)
    assert nodes["wc2"] not in eden.draft_stack_nodes(runner, wc1)
    assert nodes["wc2"] in eden.draft_stack_nodes(runner, wc2)
    assert nodes["wc1"] not in eden.draft_stack_nodes(runner, wc2)

    (copies["wc1"] / "b.txt").write_text("wc1 amended\n")
    run("-R", wc1, "amend", "-m", "wc1 draft amended")
    amended = run("-R", wc1, "log", "-r", ".", "-T", "{node}")
    assert eden.successor_nodes(runner, wc1, (nodes["wc1"],)) == (amended,)
    entry = eden.classify_drafts(
        runner, wc1, "main", amended, own_nodes=(nodes["wc1"],)
    )
    assert [node for node, _s in entry["unaccounted"]] == [amended]  # type: ignore[attr-defined]
    assert nodes["wc2"] not in str(entry)

    # The per-working-copy journal closes the between-captures
    # window: wc1 sat on the amended draft, left it for the base,
    # and built a second draft. A ledger that missed the first
    # stack still finishes accounting for it, and never for wc2's.
    base_node = run("-R", str(backing), "log", "-r", ".", "-T", "{node}")
    run("-R", wc1, "goto", base_node)
    (copies["wc1"] / "d.txt").write_text("wc1 second\n")
    run("-R", wc1, "add", str(copies["wc1"] / "d.txt"))
    run("-R", wc1, "commit", "-m", "wc1 second draft")
    second = run("-R", wc1, "log", "-r", ".", "-T", "{node}")
    config = dataclasses.replace(
        _scoped_config(tmp_path), eden_mount_root=tmp_path
    )
    checkout = dataclasses.replace(
        wrkslots_cli._checkout_from_obj(_eden_obj(), "row.checkouts[0]"),
        path=wc1, repository=str(backing),
    )
    ledger_path = wrkslots_cli._eden_draft_ledger_path(config, checkout)
    wrkslots_cli._write_eden_draft_ledger(
        ledger_path,
        eden.DraftLedger(
            slot="slot", checkout="main", generation=1, nodes=(),
            captured_at="2026-11-19T00:00:00+00:00", last_error=None,
        ),
    )
    scoped = wrkslots_cli._eden_scoped_draft_nodes(
        config, checkout, slot="slot", generation=1, runner=runner
    )
    assert set(scoped) >= {amended, second}
    assert nodes["wc2"] not in scoped

    def fresh_scoped() -> tuple[str, ...]:
        wrkslots_cli._write_eden_draft_ledger(
            ledger_path,
            eden.DraftLedger(
                slot="slot", checkout="main", generation=1, nodes=(),
                captured_at="2026-11-19T00:00:00+00:00", last_error=None,
            ),
        )
        return wrkslots_cli._eden_scoped_draft_nodes(
            config, checkout, slot="slot", generation=1, runner=runner
        )

    def draft_by_subject(subject: str) -> str:
        for line in run(
            "-R", wc1, "log", "-r", "draft()", "-T", "{node} {desc|firstline}\n"
        ).splitlines():
            node, _, found = line.partition(" ")
            if found == subject:
                return node
        raise AssertionError(f"no draft with subject {subject!r}")

    # N3b, metaedit variant: a draft rewritten while not checked
    # out is hidden at the journal node; its visible successor is
    # still this checkout's and must be accounted.
    (copies["wc1"] / "f.txt").write_text("wc1 third\n")
    run("-R", wc1, "add", str(copies["wc1"] / "f.txt"))
    run("-R", wc1, "commit", "-m", "wc1 third draft")
    third = run("-R", wc1, "log", "-r", ".", "-T", "{node}")
    run("-R", wc1, "goto", base_node)
    result = subprocess.run(
        [sl, "--config", "ui.username=Test <t@example.invalid>",
         "--config", "ui.editor=sed -i 's/third draft/third edited/'",
         "-R", wc1, "metaedit", "-r", third],
        capture_output=True, text=True, env=env, timeout=60,
    )
    assert result.returncode == 0, result.stderr
    edited = draft_by_subject("wc1 third edited")
    assert edited in fresh_scoped()
    assert nodes["wc2"] not in fresh_scoped()

    # N3b, rebase variant: same shape through a rebase off the
    # checked-out parent.
    (copies["wc1"] / "g.txt").write_text("wc1 rebase\n")
    run("-R", wc1, "add", str(copies["wc1"] / "g.txt"))
    run("-R", wc1, "commit", "-m", "wc1 rebase draft")
    rebased_from = run("-R", wc1, "log", "-r", ".", "-T", "{node}")
    run("-R", wc1, "goto", base_node)
    run("-R", wc1, "rebase", "-s", rebased_from, "-d", second)
    rebased = draft_by_subject("wc1 rebase draft")
    assert rebased != rebased_from and rebased in fresh_scoped()

    # N1: a draft made and hidden between captures is included
    # as itself, so classification names it hidden.
    (copies["wc1"] / "h.txt").write_text("wc1 hidden\n")
    run("-R", wc1, "add", str(copies["wc1"] / "h.txt"))
    run("-R", wc1, "commit", "-m", "wc1 hidden draft")
    hidden_node = run("-R", wc1, "log", "-r", ".", "-T", "{node}")
    run("-R", wc1, "goto", base_node)
    run("-R", wc1, "hide", hidden_node)
    assert hidden_node in fresh_scoped()
    entry = eden.classify_drafts(
        runner, wc1, "main", hidden_node, own_nodes=(hidden_node,)
    )
    assert entry["hidden"] == [(hidden_node, "wc1 hidden draft")]

    # A hidden own draft is named as hidden, never silently dropped.
    run("-R", wc1, "hide", amended)
    entry = eden.classify_drafts(
        runner, wc1, "main", amended, own_nodes=(amended,)
    )
    assert entry["hidden"] == [(amended, "wc1 draft amended")]
    assert [node for node, _s in entry["unaccounted"]] == [amended]  # type: ignore[attr-defined]
