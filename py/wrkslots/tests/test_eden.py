"""Eden checkout kind, cap, salvage and remove checks (#1204)."""

from __future__ import annotations

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
                repository="/home/newton/local/.eden-backing-repos/fbsource",
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
