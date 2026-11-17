"""Post-provision hooks need a Git working tree (#1731).

A slot whose checkout is an Eden (Sapling) mount — or any checkout
without a `.git` — has no working tree for the Git provisioning
hooks, and `git submodule update` there fails with "cannot be used
without a working tree".  The hooks are skipped for such checkouts
(progress still recorded, so recovery recomputes the same plan),
and ambient GIT_DIR-style variables never retarget a hook's Git
away from its checkout directory.
"""

from __future__ import annotations

import json
import os
from pathlib import Path

import pytest

from wrkslots import cli


def _config(tmp_path: Path) -> cli.Config:
    return cli.Config(
        config_path=tmp_path / ".wrkslots.yml",
        root=tmp_path,
        worktrees=tmp_path,
        control=tmp_path,
        machine="node-a",
        default_remote="origin",
        default_landed_ref="refs/remotes/origin/main",
        heartbeat_ttl_seconds=60,
        liveness_command=tmp_path / "live.py",
        post_provision_hooks=("false",),
    )


def _checkout(tmp_path: Path, name: str, vcs: str) -> cli.Checkout:
    return cli.Checkout(
        name=name,
        path=name,
        repository="repo",
        branch="eden" if vcs == "eden" else "main",
        start_point="a" * 40,
        remote="origin",
        remote_url_sha256="b" * 64,
        landed_ref="refs/remotes/origin/main",
        head="a" * 40,
        containing_remote_refs=(),
        vcs=vcs,
    )


def test_eden_checkout_hooks_are_skipped(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(cli, "_write_journal", lambda *a, **k: None)
    (tmp_path / "checkout").mkdir()
    config = _config(tmp_path)
    journal: dict[str, object] = {
        "slot_type": "agent",
        "post_provision_hooks": ["false"],
        "hook_progress": 0,
    }
    journal_path = tmp_path / "CREATE.journal"
    total = cli._run_post_provision_hooks(
        config,
        [_checkout(tmp_path, "checkout", "eden")],
        journal_path,
        journal,
    )
    # The failing hook never ran; the step still counts as done.
    assert total == 1
    assert journal["hook_progress"] == 1


def test_hook_env_drops_ambient_git_location(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(cli, "_write_journal", lambda *a, **k: None)
    checkout_dir = tmp_path / "checkout"
    checkout_dir.mkdir()
    (checkout_dir / ".git").mkdir()
    seen = tmp_path / "seen.json"
    monkeypatch.setenv("GIT_DIR", "/bare/repo.git")
    config = cli.Config(
        config_path=tmp_path / ".wrkslots.yml",
        root=tmp_path,
        worktrees=tmp_path,
        control=tmp_path,
        machine="node-a",
        default_remote="origin",
        default_landed_ref="refs/remotes/origin/main",
        heartbeat_ttl_seconds=60,
        liveness_command=tmp_path / "live.py",
        post_provision_hooks=(
            "python3 -c \"import json,os; "
            f"json.dump(dict(os.environ), open({str(seen)!r},'w'))\"",
        ),
    )
    journal: dict[str, object] = {
        "slot_type": "agent",
        "post_provision_hooks": list(config.post_provision_hooks),
        "hook_progress": 0,
    }
    total = cli._run_post_provision_hooks(
        config,
        [_checkout(tmp_path, "checkout", "git")],
        tmp_path / "CREATE.journal",
        journal,
    )
    assert total == 1
    env = json.loads(seen.read_text(encoding="utf-8"))
    assert "GIT_DIR" not in env
    assert env["WRKSLOTS_CHECKOUT_NAME"] == "checkout"
