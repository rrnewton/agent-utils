"""Differential: wrkslotsd create against the Python create mutation.

#562 rust-primary-wrkslots slice 34, the ninth writer slice. The
Python oracle is `_cmd_create` on a copy of the fixture registry; the
Rust writer runs on the original. Resulting HOLD records and
`slot-held` events must match apart from the wall-clock stamps (and
the event digest, re-verified with the Python digest function).
Temporary fixture registries only: writers are never pointed at a
live registry before the edition cutover.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
from pathlib import Path
from typing import cast

import pytest
from wrkslots import cli as wrkslots_cli

from wrkslots.tests.rust_binary import rust_wrkslotsd


AU_ROOT = Path(__file__).resolve().parents[3]
DIGEST = "b" * 64
HOLD_NAME = "HOLD.6.node-a.slot-a.json"


def _git(cwd: Path, *args: str) -> str:
    env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
    env.update(
        {
            "LC_ALL": "C",
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": "/dev/null",
            "GIT_CONFIG_SYSTEM": "/dev/null",
        }
    )
    result = subprocess.run(
        [
            "git", "-c", "user.email=test@example.invalid", "-c", "user.name=Test",
            "-c", "commit.gpgsign=false", "-C", str(cwd), *args,
        ],
        capture_output=True, text=True, check=True, env=env, timeout=120,
    )
    return result.stdout.strip()


def _repo(path: Path) -> str:
    path.mkdir(parents=True)
    _git(path, "init", "-q", "-b", "main")
    (path / "file.txt").write_text("one\n")
    _git(path, "add", "file.txt")
    _git(path, "commit", "-q", "-m", "one")
    return _git(path, "rev-parse", "HEAD")


def _record(head: str) -> dict[str, object]:
    identity = {
        "pid": 123,
        "start_ticks": 1123,
        "boot_id": "boot-1",
        "host_id": "host-1",
        "cgroup_path": "/user.slice/session.scope",
    }
    return {
        "slot": "slot-a",
        "agent": "agent-slot-a",
        "task": "do work",
        "purpose": "testing",
        "machine": "node-a",
        "generation": 1,
        "created_at": "2026-10-26T00:00:00Z",
        "heartbeat_at": "2026-10-26T01:00:00Z",
        "heartbeat_ttl_seconds": 60,
        "owner": identity,
        "coordinator_lease": identity,
        "coordinator_recovery_note": None,
        "handoff": None,
        "checkouts": [
            {
                "name": "repo",
                "path": "worktrees/slot-a/repo",
                "repository": "../upstream-repo",
                "branch": "suite/work",
                "start_point": head,
                "remote": "origin",
                "remote_url_sha256": DIGEST,
                "landed_ref": "refs/remotes/origin/main",
                "head": head,
                "containing_remote_refs": ["refs/remotes/origin/main"],
                "vcs": "git",
            }
        ],
    }


def _wire_origin(upstream: Path) -> None:
    _git(upstream, "remote", "add", "origin", str(upstream))
    _git(upstream, "fetch", "-q", "origin", "+refs/heads/*:refs/remotes/origin/*")


def _fixture(base: Path) -> Path:
    root = base / "project"
    (root / "worktrees").mkdir(parents=True)
    (root / "live.py").write_text("# probe\n")
    (root / ".wrkslots.yml").write_text(
        "schema: 2\n"
        "machine: node-a\n"
        "worktrees_dir: worktrees\n"
        "default_remote: origin\n"
        "default_landed_ref: refs/remotes/origin/main\n"
        "heartbeat_ttl_seconds: 60\n"
        "liveness_command: live.py\n"
    )
    upstream = base / "upstream-repo"
    head = _repo(upstream)
    _wire_origin(upstream)
    _git(
        upstream, "worktree", "add", "-q", "-b", "suite/work",
        str(root / "worktrees" / "slot-a" / "repo"), "HEAD",
    )
    (root / "worktrees" / "ACTIVE.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 7, "slots": [_record(head)]})
    )
    (root / "worktrees" / "ARCHIVED.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 3, "records": []})
    )
    return root


def _rust(root: Path, slot: str, reason: str, *, fixture: bool = True) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    if fixture:
        env["WRKSLOTSD_FIXTURE_WRITER"] = "1"
    else:
        env.pop("WRKSLOTSD_FIXTURE_WRITER", None)
    return subprocess.run(
        [str(rust_wrkslotsd()), "hold", slot, "--reason", reason, "--root", str(root)],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900, env=env,
    )


def _python(root: Path, slot: str, reason: str) -> int:
    args = argparse.Namespace(
        project_root=str(root), machine=None, slot=slot, reason=reason, wait_lock=0.0
    )
    return wrkslots_cli._cmd_hold(args)


def _pair(tmp_path: Path) -> tuple[Path, Path]:
    return _fixture(tmp_path / "rust"), _fixture(tmp_path / "oracle")


def _hold_record(root: Path) -> dict[str, object] | None:
    path = root / "worktrees" / HOLD_NAME
    if not path.exists():
        return None
    record: dict[str, object] = json.loads(path.read_text())
    record.pop("held_at")
    return record


def _events(root: Path) -> list[dict[str, object]]:
    directory = root / "worktrees" / "EVENTS.node-a"
    if not directory.is_dir():
        return []
    events: list[dict[str, object]] = []
    for path in sorted(directory.iterdir()):
        event: dict[str, object] = json.loads(path.read_text())
        assert event["sha256"] == wrkslots_cli._event_digest(
            {k: v for k, v in event.items() if k != "sha256"}
        )
        event.pop("recorded_at")
        event.pop("sha256")
        event.pop("previous_sha256")
        # Each fixture pair has its own Git history, so commit ids differ.
        payload = event.get("payload")
        if isinstance(payload, dict):
            active = payload.get("active")
            if isinstance(active, dict):
                for record in active.get("slots", []):
                    for checkout in record.get("checkouts", []):
                        checkout["head"] = "HEAD"
                        checkout["start_point"] = "HEAD"
        events.append(event)
    return events




def _empty_fixture(base: Path) -> Path:
    root = base / "project"
    (root / "worktrees").mkdir(parents=True)
    (root / "live.py").write_text("# probe\n")
    (root / ".wrkslots.yml").write_text(
        "schema: 2\n"
        "machine: node-a\n"
        "worktrees_dir: worktrees\n"
        "default_remote: origin\n"
        "default_landed_ref: refs/remotes/origin/main\n"
        "heartbeat_ttl_seconds: 60\n"
        "liveness_command: live.py\n"
    )
    upstream = base / "upstream-repo"
    _repo(upstream)
    _wire_origin(upstream)
    return root


def _rust_create(
    root: Path,
    *,
    slot: str = "slot-b",
    branch: str = "suite/new",
    agent: str = "agent-new",
    fixture: bool = True,
    authorized: bool = True,
    representation: str = "config",
    slot_type: str = "agent",
) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    if fixture:
        env["WRKSLOTSD_FIXTURE_WRITER"] = "1"
    else:
        env.pop("WRKSLOTSD_FIXTURE_WRITER", None)
    args = [
        str(rust_wrkslotsd()), "create", slot, "--slot-type", slot_type,
        "--agent", agent, "--task", "task-1", "--purpose", "testing",
        "--owner-pid", str(os.getpid()), "--coordinator-pid", str(os.getpid()),
        "--repo", "repo=../upstream-repo", "--branch", f"repo={branch}",
        "--representation", representation,
        "--machine", "node-a", "--root", str(root),
    ]
    if authorized:
        args.insert(args.index("--agent"), "--coordinator-authorized")
    return subprocess.run(
        args, cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900, env=env
    )


def _python_create(
    root: Path,
    *,
    slot: str = "slot-b",
    branch: str = "suite/new",
    agent: str = "agent-new",
    slot_type: str = "agent",
) -> int:
    args = argparse.Namespace(
        project_root=str(root), machine="node-a", slot=slot, slot_type=slot_type,
        coordinator_authorized=True, agent=agent, task="task-1",
        purpose="testing", owner_pid=os.getpid(), coordinator_pid=os.getpid(),
        task_scope_unit=None, task_scope_invocation_id=None,
        repo=["repo=../upstream-repo"], remote=[], remote_url=[],
        branch=[f"repo={branch}"], start=[], override_disk_floor=False,
        representation="config", format="human", wait_lock=0.0,
    )
    return wrkslots_cli._cmd_create(args)


def _active_doc(root: Path) -> dict[str, object]:
    doc: dict[str, object] = json.loads((root / "worktrees" / "ACTIVE.node-a.json").read_text())
    for record in cast(list[dict[str, object]], doc["slots"]):
        record.pop("created_at")
        record.pop("heartbeat_at")
        for checkout in cast(list[dict[str, object]], record["checkouts"]):
            checkout["head"] = "HEAD"
            checkout["start_point"] = "HEAD"
            checkout["remote_url_sha256"] = "URLSHA"
    return doc


def _kinds(root: Path) -> list[str]:
    return [str(event["kind"]) for event in _events(root)]


def test_create_matches_python(tmp_path: Path) -> None:
    rust_root = _empty_fixture(tmp_path / "rust")
    oracle_root = _empty_fixture(tmp_path / "oracle")
    assert _python_create(oracle_root) == 0
    result = _rust_create(rust_root)
    assert result.returncode == 0, result.stderr
    assert _active_doc(rust_root) == _active_doc(oracle_root)
    assert _kinds(rust_root) == _kinds(oracle_root)
    assert "active-state-recorded" in _kinds(rust_root)
    checkout = rust_root / "worktrees" / "slot-b" / "repo"
    assert (checkout / "file.txt").read_text() == "one\n"
    assert not list((rust_root / "worktrees").glob("CREATE.*.journal"))


def test_create_duplicate_slot_refuses(tmp_path: Path) -> None:
    rust_root = _empty_fixture(tmp_path / "rust")
    oracle_root = _empty_fixture(tmp_path / "oracle")
    assert _python_create(oracle_root) == 0
    assert _rust_create(rust_root).returncode == 0
    with pytest.raises(Exception):
        _python_create(oracle_root)
    result = _rust_create(rust_root)
    assert result.returncode == 1


def test_create_existing_branch_refuses(tmp_path: Path) -> None:
    rust_root = _empty_fixture(tmp_path / "rust")
    oracle_root = _empty_fixture(tmp_path / "oracle")
    assert _python_create(oracle_root, branch="suite/work") == 0
    assert _rust_create(rust_root, branch="suite/work").returncode == 0
    assert _python_create(oracle_root, slot="slot-c", branch="suite/work2", agent="agent-two") == 0
    # suite/work2 now exists in the oracle upstream only; use a branch that
    # exists in both histories: create slot-c in rust too, then reuse its branch.
    assert _rust_create(rust_root, slot="slot-c", branch="suite/work2", agent="agent-two").returncode == 0
    with pytest.raises(Exception) as caught:
        _python_create(oracle_root, slot="slot-d", branch="suite/work2", agent="agent-three")
    assert "local branch already exists" in str(caught.value)
    result = _rust_create(rust_root, slot="slot-d", branch="suite/work2", agent="agent-three")
    assert result.returncode == 1
    assert "local branch already exists" in result.stderr


def test_create_without_coordinator_authorization_refuses(tmp_path: Path) -> None:
    rust_root = _empty_fixture(tmp_path / "rust")
    result = _rust_create(rust_root, authorized=False)
    assert result.returncode == 1
    assert "coordinator assignment" in result.stderr
    assert not (rust_root / "worktrees" / "slot-b").exists()


def test_create_hook_failure_records_journal(tmp_path: Path) -> None:
    rust_root = _empty_fixture(tmp_path / "rust")
    config = rust_root / ".wrkslots.yml"
    config.write_text(
        config.read_text() + "post_provision_hooks:\n  - 'echo hook-broken >&2; exit 3'\n"
    )
    result = _rust_create(rust_root)
    assert result.returncode == 1
    assert "post-provision hook 0" in result.stderr
    journals = list((rust_root / "worktrees").glob("CREATE.*.journal"))
    assert len(journals) == 1
    journal = json.loads(journals[0].read_text())
    failure = cast(dict[str, object], journal["hook_failure"])
    assert failure["status"] == "failed"
    assert failure["hook_index"] == 0
    assert "hook-broken" in str(failure["detail"])
    assert not (rust_root / "worktrees" / "ACTIVE.node-a.json").exists() or (
        "slot-b" not in json.loads((rust_root / "worktrees" / "ACTIVE.node-a.json").read_text())["slots"]
    )


def _harness_fixture(base: Path) -> Path:
    root = _empty_fixture(base)
    config = root / ".wrkslots.yml"
    config.write_text(
        config.read_text()
        + "layout: nested\nharness_repo: app\nproduct_repos: [repo]\n"
    )
    return root


def _rust_create_harness(
    root: Path,
    *,
    repos: list[str],
    branches: list[str] | None = None,
    include: bool = False,
    reason: str | None = None,
) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    env["WRKSLOTSD_FIXTURE_WRITER"] = "1"
    args = [
        str(rust_wrkslotsd()), "create", "slot-b", "--slot-type", "agent",
        "--coordinator-authorized",
        "--agent", "agent-new", "--task", "task-1", "--purpose", "testing",
        "--owner-pid", str(os.getpid()), "--coordinator-pid", str(os.getpid()),
        "--machine", "node-a", "--root", str(root),
    ]
    for item in repos:
        args += ["--repo", item]
    for item in branches or []:
        args += ["--branch", item]
    if include:
        args.append("--include-harness-checkout-i-know-this-is-heavy")
    if reason is not None:
        args += ["--harness-checkout-reason", reason]
    return subprocess.run(
        args, cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900, env=env
    )


def _python_create_harness(
    root: Path,
    *,
    repos: list[str],
    branches: list[str] | None = None,
    include: bool = False,
    reason: str | None = None,
) -> int:
    args = argparse.Namespace(
        project_root=str(root), machine="node-a", slot="slot-b", slot_type="agent",
        coordinator_authorized=True, agent="agent-new", task="task-1",
        purpose="testing", owner_pid=os.getpid(), coordinator_pid=os.getpid(),
        task_scope_unit=None, task_scope_invocation_id=None,
        repo=repos, remote=[], remote_url=[],
        branch=branches or [], start=[], override_disk_floor=False,
        representation="config", format="human", wait_lock=0.0,
        include_harness_checkout=include, harness_checkout_reason=reason,
    )
    return wrkslots_cli._cmd_create(args)


def test_create_harness_repo_refuses_without_heavy_opt_in(tmp_path: Path) -> None:
    rust_root = _harness_fixture(tmp_path / "rust")
    oracle_root = _harness_fixture(tmp_path / "oracle")
    with pytest.raises(Exception) as caught:
        _python_create_harness(oracle_root, repos=["app=../upstream-repo"])
    result = _rust_create_harness(rust_root, repos=["app=../upstream-repo"])
    assert result.returncode == 1
    assert str(caught.value) in result.stderr
    _assert_no_create_side_effects(rust_root)


def test_create_harness_flags_validate_like_python(tmp_path: Path) -> None:
    rust_root = _harness_fixture(tmp_path / "rust")
    oracle_root = _harness_fixture(tmp_path / "oracle")
    cases = [
        (["app=../upstream-repo"], True, None,
         "--include-harness-checkout-i-know-this-is-heavy requires"),
        (["app=../upstream-repo"], False, "needed",
         "--harness-checkout-reason requires"),
        (["zzz=../upstream-repo"], False, None,
         "not declared as product repos in harness mode"),
    ]
    for repos, include, reason, needle in cases:
        with pytest.raises(Exception) as caught:
            _python_create_harness(oracle_root, repos=repos, include=include, reason=reason)
        result = _rust_create_harness(rust_root, repos=repos, include=include, reason=reason)
        assert result.returncode == 1
        assert needle in str(caught.value)
        assert needle in result.stderr
        _assert_no_create_side_effects(rust_root)


def test_create_harness_checkout_with_flags_matches_python(tmp_path: Path) -> None:
    rust_root = _harness_fixture(tmp_path / "rust")
    oracle_root = _harness_fixture(tmp_path / "oracle")
    assert _python_create_harness(
        oracle_root,
        repos=["app=../upstream-repo"], branches=["app=suite/new"],
        include=True, reason="suite setup",
    ) == 0
    result = _rust_create_harness(
        rust_root,
        repos=["app=../upstream-repo"], branches=["app=suite/new"],
        include=True, reason="suite setup",
    )
    assert result.returncode == 0, result.stderr
    assert _active_doc(rust_root) == _active_doc(oracle_root)
    record = _active_doc(rust_root)["slots"][0]  # type: ignore[index]
    assert isinstance(record, dict)
    assert record["harness_checkout_reason"] == "suite setup"


def test_create_refuses_without_fixture_attestation(tmp_path: Path) -> None:
    rust_root = _empty_fixture(tmp_path / "plain")
    result = _rust_create(rust_root, fixture=False)
    assert result.returncode == 1
    assert "fixture-only" in result.stderr
    assert not (rust_root / "worktrees" / "slot-b").exists()


def _assert_no_create_side_effects(root: Path) -> None:
    assert not (root / "worktrees" / "slot-b").exists()
    assert not list((root / "worktrees").glob("CREATE.*.journal"))
    assert not (root / "worktrees" / "ACTIVE.node-a.json").exists()


def test_create_config_image_refuses_without_side_effects(tmp_path: Path) -> None:
    # config -> image must refuse, never fall through to a plain
    # worktree (the create fail-open the resolution hook closes).
    rust_root = _empty_fixture(tmp_path / "rust")
    config = rust_root / ".wrkslots.yml"
    config.write_text(config.read_text() + "slot_representation: image\n")
    result = _rust_create(rust_root)
    assert result.returncode == 1, result.stderr
    assert "slot-image provisioning stays with the Python edition" in result.stderr
    _assert_no_create_side_effects(rust_root)


def test_create_explicit_image_refuses_without_side_effects(tmp_path: Path) -> None:
    rust_root = _empty_fixture(tmp_path / "rust")
    result = _rust_create(rust_root, representation="image")
    assert result.returncode == 1, result.stderr
    assert "slot-image provisioning stays with the Python edition" in result.stderr
    _assert_no_create_side_effects(rust_root)


def test_create_config_image_explicit_worktree_proceeds(tmp_path: Path) -> None:
    rust_root = _empty_fixture(tmp_path / "rust")
    config = rust_root / ".wrkslots.yml"
    config.write_text(config.read_text() + "slot_representation: image\n")
    result = _rust_create(rust_root, representation="worktree")
    assert result.returncode == 0, result.stderr
    checkout = rust_root / "worktrees" / "slot-b" / "repo"
    assert (checkout / "file.txt").read_text() == "one\n"


def test_create_invalid_representation_refuses(tmp_path: Path) -> None:
    rust_root = _empty_fixture(tmp_path / "rust")
    result = _rust_create(rust_root, representation="bogus")
    assert result.returncode == 1, result.stderr
    assert "--representation must be config, worktree, or image, not 'bogus'" in result.stderr
    _assert_no_create_side_effects(rust_root)


def _identity_fixture(base: Path, *, extra: str = "") -> Path:
    """An empty fixture whose repository has a submodule and an identity configured (#1096)."""

    root = _empty_fixture(base)
    leaf = base / "leaf-repo"
    _repo(leaf)
    upstream = base / "upstream-repo"
    _git(upstream, "-c", "protocol.file.allow=always", "submodule", "add", "-q", str(leaf), "leaf")
    _git(upstream, "commit", "-q", "-m", "add leaf")
    _git(upstream, "fetch", "-q", "origin", "+refs/heads/*:refs/remotes/origin/*")
    config = root / ".wrkslots.yml"
    config.write_text(
        config.read_text()
        + "identity: {name: Slot Bot, email: slot-bot@example.invalid}\n"
        + "post_provision_hooks:\n"
        + "  - 'git -c protocol.file.allow=always submodule update --init --quiet'\n"
        + extra
    )
    return root


def _identity_lines(text: str) -> list[str]:
    return [line for line in text.splitlines() if line.startswith("identity: ")]


def _config_list(cwd: Path, *scope: str) -> list[str]:
    return sorted(
        line for line in _git(cwd, "config", *scope, "--list").splitlines()
        if line.startswith(("user.", "extensions."))
    )


def test_create_identity_matches_python(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    rust_root = _identity_fixture(tmp_path / "rust")
    oracle_root = _identity_fixture(tmp_path / "oracle")
    capsys.readouterr()
    assert _python_create(oracle_root) == 0
    oracle_lines = _identity_lines(capsys.readouterr().err)
    result = _rust_create(rust_root)
    assert result.returncode == 0, result.stderr
    assert _identity_lines(result.stderr) == oracle_lines == [
        "identity: checkout=repo worktree config: Slot Bot <slot-bot@example.invalid>",
        "identity: checkout=repo submodule=leaf: set",
    ]
    assert _active_doc(rust_root) == _active_doc(oracle_root)
    for root in (rust_root, oracle_root):
        checkout = root / "worktrees" / "slot-b" / "repo"
        shared = root.parent / "upstream-repo" / ".git" / "config"
        assert _config_list(checkout, "--worktree") == [
            "user.email=slot-bot@example.invalid", "user.name=Slot Bot",
        ]
        assert _config_list(checkout / "leaf", "--local") == [
            "user.email=slot-bot@example.invalid", "user.name=Slot Bot",
        ]
        assert _config_list(checkout, "--file", str(shared)) == [
            "extensions.worktreeconfig=true",
        ]


def test_create_identity_refusal_matches_python(tmp_path: Path) -> None:
    rust_root = _identity_fixture(tmp_path / "rust")
    oracle_root = _identity_fixture(tmp_path / "oracle")
    for root in (rust_root, oracle_root):
        upstream = root.parent / "upstream-repo"
        _git(upstream, "config", "core.worktree", str(upstream))
    with pytest.raises(Exception) as caught:
        _python_create(oracle_root)
    result = _rust_create(rust_root)
    assert result.returncode == 1
    for text in (str(caught.value), result.stderr):
        assert "which also sets core.worktree" in text
        assert "remove identity from the configuration" in text
    for root in (rust_root, oracle_root):
        assert not (root / "worktrees" / "slot-b").exists()
        assert not list((root / "worktrees").glob("CREATE.*.journal"))


def test_create_hooks_see_the_slot_type_in_both_editions(tmp_path: Path) -> None:
    """#1122: every post-provision hook sees WRKSLOTS_SLOT_TYPE."""

    roots = {edition: _empty_fixture(tmp_path / edition) for edition in ("rust", "python")}
    for root in roots.values():
        config = root / ".wrkslots.yml"
        config.write_text(
            config.read_text()
            + "post_provision_hooks:\n  - 'printf %s \"$WRKSLOTS_SLOT_TYPE\" > slot-type'\n"
        )
    assert _python_create(roots["python"]) == 0
    result = _rust_create(roots["rust"])
    assert result.returncode == 0, result.stderr
    for root in roots.values():
        assert (root / "worktrees" / "slot-b" / "repo" / "slot-type").read_text() == "agent"


def test_create_hook_slot_types_entry_matches_python(tmp_path: Path) -> None:
    """#1122 parity: both editions run an agent-scoped mapping hook for an agent slot."""

    roots = {edition: _empty_fixture(tmp_path / edition) for edition in ("rust", "python")}
    for root in roots.values():
        config = root / ".wrkslots.yml"
        config.write_text(
            config.read_text()
            + "post_provision_hooks:\n"
            + "  - {command: 'printf ran > agent-hook', slot_types: [agent]}\n"
        )
    assert _python_create(roots["python"]) == 0
    result = _rust_create(roots["rust"])
    assert result.returncode == 0, result.stderr
    for root in roots.values():
        assert (root / "worktrees" / "slot-b" / "repo" / "agent-hook").read_text() == "ran"
    assert _active_doc(roots["rust"]) == _active_doc(roots["python"])
    assert _kinds(roots["rust"]) == _kinds(roots["python"])


def test_create_hook_slot_types_select_by_type_in_both_editions(tmp_path: Path) -> None:
    """#1122 parity: scoped hooks run in only their slot types in both editions."""

    roots = {edition: _empty_fixture(tmp_path / edition) for edition in ("rust", "python")}
    hook_lines = (
        "post_provision_hooks:\n"
        "  - 'printf x >> everywhere-hooks'\n"
        "  - {command: 'printf ran > agent-only', slot_types: [agent]}\n"
        "  - {command: 'printf ran > validate-only', slot_types: [validate]}\n"
    )
    for root in roots.values():
        config = root / ".wrkslots.yml"
        config.write_text(config.read_text() + hook_lines)
    for edition, root in roots.items():
        for slot, branch, agent, slot_type in (
            ("slot-b", "suite/new", "agent-new", "agent"),
            ("slot-c", "suite/new-validate", "agent-validate", "validate"),
        ):
            if edition == "python":
                assert _python_create(root, slot=slot, branch=branch, agent=agent, slot_type=slot_type) == 0
            else:
                result = _rust_create(root, slot=slot, branch=branch, agent=agent, slot_type=slot_type)
                assert result.returncode == 0, result.stderr
    assert _active_doc(roots["rust"]) == _active_doc(roots["python"])
    assert _kinds(roots["rust"]) == _kinds(roots["python"])
    for root in roots.values():
        agent_checkout = root / "worktrees" / "slot-b" / "repo"
        validate_checkout = root / "worktrees" / "validate" / "slot-c" / "repo"
        assert (agent_checkout / "agent-only").read_text() == "ran"
        assert not (agent_checkout / "validate-only").exists()
        assert (agent_checkout / "everywhere-hooks").read_text() == "x"
        assert (validate_checkout / "validate-only").read_text() == "ran"
        assert not (validate_checkout / "agent-only").exists()
        assert (validate_checkout / "everywhere-hooks").read_text() == "x"
