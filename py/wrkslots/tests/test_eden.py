"""Eden checkout kind, cap, salvage and remove checks (#1204)."""

from __future__ import annotations

import dataclasses
import json
from collections.abc import Sequence
from pathlib import Path

import pytest
from wrkslots import cli as wrkslots_cli
from wrkslots import eden

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

    with pytest.raises(wrkslots_cli.Refusal, match="revision changed"):
        wrkslots_cli._assert_eden_checkout_safe(config, checkout, moved)


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
    calls: list[tuple[str, ...]] = []

    def run(argv: Sequence[str]) -> tuple[int, str, str]:
        calls.append(tuple(argv))
        if tuple(argv[:2]) == ("sl", "clone"):
            _fake_backing(Path(argv[-1]), default=str(source))
            return 0, "", ""
        return 0, "a" * 40 + "\n", ""

    eden.seed_backing_repo(run, source, staging)

    assert calls[0] == ("sl", "clone", "-U", str(source), str(staging))
    assert calls[1][:3] == ("sl", "-R", str(staging))
    facts = eden.backing_facts(staging)
    assert facts["default_path"] == "fb:source-repo"


def test_seed_refuses_when_clone_stays_pointed_at_source(tmp_path: Path) -> None:
    source = _fake_backing(tmp_path / "source", default=None)
    staging = tmp_path / "staging"

    def run(argv: Sequence[str]) -> tuple[int, str, str]:
        if tuple(argv[:2]) == ("sl", "clone"):
            _fake_backing(Path(argv[-1]), default=str(source))
            return 0, "", ""
        return 0, "a" * 40 + "\n", ""

    with pytest.raises(eden.EdenError, match="source"):
        eden.seed_backing_repo(run, source, staging)


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
            eden_backing_repo="source-repo",
        ),
        worktrees_image_state=worktreesimage.MOUNTED,
    )
    args = argparse.Namespace(eden=["main=/elsewhere/repo"], eden_rev=[])
    with pytest.raises(wrkslots_cli.Refusal, match="eden_backing_repo"):
        wrkslots_cli._eden_create_plan(config, args)
    args = argparse.Namespace(eden=["main=source-repo"], eden_rev=[])
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
