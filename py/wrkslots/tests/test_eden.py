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
    assert commands[-1][-1].endswith("draft-stack.bundle")
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
