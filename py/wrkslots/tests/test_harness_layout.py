"""Focused tests for #1299 harness-layout-mode."""

from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from wrkslots import cli as wrkslots  # noqa: E402


def _git(cwd: Path, *args: str) -> str:
    out = subprocess.run(
        ["git", "-C", str(cwd), *args], text=True, capture_output=True, check=False
    )
    assert out.returncode == 0, out.stderr
    return out.stdout.strip()


def _repo(path: Path) -> Path:
    path.mkdir(parents=True)
    _git(path, "init", "-b", "main")
    _git(path, "config", "user.email", "slot@example.invalid")
    _git(path, "config", "user.name", "Slot")
    (path / "f.txt").write_text("one\n")
    _git(path, "add", "f.txt")
    _git(path, "commit", "-m", "one")
    return path


def _identity(pid: int) -> wrkslots.ProcessIdentity:
    return wrkslots.ProcessIdentity(
        pid=pid, start_ticks=100 + pid, boot_id="boot-1", host_id="host-1",
        cgroup_path="/user.slice/session.scope",
    )


def test_parse_harness_declaration() -> None:
    assert wrkslots._parse_harness_declaration({}, "nested") == (None, ())
    assert wrkslots._parse_harness_declaration(
        {"harness_repo": "harness", "product_repos": ["product"]}, "nested"
    ) == ("harness", ("product",))
    with pytest.raises(Exception, match="requires configuration.product_repos"):
        wrkslots._parse_harness_declaration({"harness_repo": "harness"}, "nested")
    with pytest.raises(Exception, match="requires configuration.harness_repo"):
        wrkslots._parse_harness_declaration({"product_repos": ["product"]}, "nested")
    with pytest.raises(Exception, match="both harness and product"):
        wrkslots._parse_harness_declaration(
            {"harness_repo": "harness", "product_repos": ["harness"]}, "nested"
        )
    with pytest.raises(Exception, match="duplicate"):
        wrkslots._parse_harness_declaration(
            {"harness_repo": "harness", "product_repos": ["product", "product"]}, "nested"
        )
    with pytest.raises(Exception, match="at least one product"):
        wrkslots._parse_harness_declaration(
            {"harness_repo": "harness", "product_repos": []}, "nested"
        )
    with pytest.raises(Exception, match="nested layout"):
        wrkslots._parse_harness_declaration(
            {"harness_repo": "harness", "product_repos": ["product"]}, "flat"
        )


def _config_root(tmp_path: Path, extra: str = "") -> Path:
    root = tmp_path / "project"
    (root / "worktrees").mkdir(parents=True)
    (root / "live.py").write_text("# probe\n")
    (root / ".wrkslots.yml").write_text(
        "schema: 2\nmachine: node-a\nworktrees_dir: worktrees\n"
        "default_remote: origin\ndefault_landed_ref: refs/remotes/origin/main\n"
        "heartbeat_ttl_seconds: 60\nliveness_command: live.py\n" + extra
    )
    return root


def test_load_config_harness_mode(tmp_path: Path) -> None:
    root = _config_root(tmp_path, "harness_repo: harness\nproduct_repos: [product]\n")
    config = wrkslots._load_config(str(root), None)
    assert config.harness_repo == "harness"
    assert config.product_repos == ("product",)
    root2 = _config_root(tmp_path / "b", "harness_repo: harness\nproduct_repos: [harness]\n")
    with pytest.raises(Exception, match="both harness and product"):
        wrkslots._load_config(str(root2), None)
    root3 = _config_root(tmp_path / "c")
    assert wrkslots._load_config(str(root3), None).harness_repo is None


def _create_args(**over: object) -> argparse.Namespace:
    base: dict[str, object] = dict(
        slot="slot01", slot_type="agent", agent="agent-1", task="t1", purpose="p",
        coordinator_pid=123, owner_pid=None,
        repo=["harness=../harness", "product=../product"],
        branch=["harness=b-h", "product=b-p"],
        include_harness_checkout=False, harness_checkout_reason=None,
    )
    base.update(over)
    return argparse.Namespace(**base)


def _harness_config() -> wrkslots.Config:
    return wrkslots.Config(
        root=Path("/tmp/x"), config_path=Path("/tmp/x/.wrkslots.yml"),
        worktrees=Path("/tmp/x/worktrees"), control=Path("/tmp/x/worktrees"),
        machine="node-a", default_remote="origin",
        default_landed_ref="refs/remotes/origin/main", heartbeat_ttl_seconds=60,
        liveness_command=Path("/tmp/x/live.py"),
        harness_repo="harness", product_repos=("product",),
    )


def test_create_refusal_prints_product_only_command() -> None:
    config = _harness_config()
    with pytest.raises(wrkslots.Refusal) as exc:
        wrkslots._assert_harness_create_allowed(config, _create_args())
    text = str(exc.value)
    assert "harness repo 'harness'" in text
    assert "--repo product=../product" in text
    assert "--repo harness=" not in text.split("instead:")[1]
    assert "--include-harness-checkout-i-know-this-is-heavy" in text
    # flag without reason, and reason without flag, both refuse
    with pytest.raises(wrkslots.Refusal, match="requires --harness-checkout-reason"):
        wrkslots._assert_harness_create_allowed(
            config, _create_args(include_harness_checkout=True)
        )
    with pytest.raises(wrkslots.Refusal, match="requires --include-harness"):
        wrkslots._assert_harness_create_allowed(
            config, _create_args(harness_checkout_reason="need it")
        )
    # both together passes; product-only passes; unknown repo refuses
    wrkslots._assert_harness_create_allowed(
        config,
        _create_args(include_harness_checkout=True, harness_checkout_reason="need it"),
    )
    wrkslots._assert_harness_create_allowed(
        config, _create_args(repo=["product=../product"], branch=["product=b-p"])
    )
    with pytest.raises(wrkslots.Refusal, match="not declared as product"):
        wrkslots._assert_harness_create_allowed(
            config, _create_args(repo=["other=../other"], branch=["other=b"])
        )


def test_record_round_trip_harness_reason() -> None:
    owner = _identity(10)
    checkout = wrkslots.Checkout(
        name="harness", path="worktrees/slots/slot01/harness", repository="harness-src",
        branch="b", start_point="1" * 40, remote="origin",
        remote_url_sha256="2" * 64, landed_ref="refs/remotes/origin/main", head="3" * 40,
    )
    record = wrkslots.ActiveRecord(
        slot="slot01", agent="agent-1", task="t", purpose="p", slot_type="agent",
        machine="node-a", generation=1, created_at="2026-01-01T00:00:00+00:00",
        heartbeat_at="2026-01-01T00:00:00+00:00", heartbeat_ttl_seconds=60,
        owner=owner, coordinator_lease=_identity(11), coordinator_recovery_note=None,
        handoff=None, checkouts=(checkout,), harness_checkout_reason="need harness docs",
    )
    encoded = wrkslots._record_to_obj(record)
    assert encoded["harness_checkout_reason"] == "need harness docs"
    assert wrkslots._record_from_obj(encoded, "record") == record
    legacy = dict(encoded)
    del legacy["harness_checkout_reason"]
    assert wrkslots._record_from_obj(legacy, "legacy").harness_checkout_reason is None


def test_harness_listing_unpushed_consumer_shape(tmp_path: Path) -> None:
    """58-of-62 style fixture: harness slots listed separately, unpushed flagged."""
    root = tmp_path / "project"
    harness_src = _repo(root / "harness-src")
    product_src = _repo(root / "product-src")
    # harness slot with an extra unpushed commit; product slot published
    slot_h = root / "worktrees" / "slots" / "slot01"
    slot_p = root / "worktrees" / "slots" / "slot02"
    slot_h.mkdir(parents=True)
    slot_p.mkdir(parents=True)
    _git(harness_src, "worktree", "add", "-b", "topic-h", str(slot_h / "harness"))
    _git(product_src, "worktree", "add", "-b", "topic-p", str(slot_p / "product"))
    (slot_h / "harness" / "new.txt").write_text("wip\n")
    _git(slot_h / "harness", "add", "new.txt")
    _git(slot_h / "harness", "commit", "-m", "wip")
    _git(slot_p / "product", "update-ref", "refs/remotes/origin/main", "HEAD")
    config = wrkslots.Config(
        root=root, config_path=root / ".wrkslots.yml",
        worktrees=root / "worktrees" / "slots", control=root / "worktrees",
        machine="node-a", default_remote="origin",
        default_landed_ref="refs/remotes/origin/main", heartbeat_ttl_seconds=60,
        liveness_command=root / "live.py",
        harness_repo="harness", product_repos=("product",),
    )
    (root / "live.py").write_text("# probe\n")

    def rec(slot: str, name: str, repo: str, path: str, reason: str | None) -> wrkslots.ActiveRecord:
        return wrkslots.ActiveRecord(
            slot=slot, agent=f"a-{slot}", task="t", purpose="p", slot_type="agent",
            machine="node-a", generation=1, created_at="2026-01-01T00:00:00+00:00",
            heartbeat_at="2026-01-01T00:00:00+00:00", heartbeat_ttl_seconds=60,
            owner=_identity(20), coordinator_lease=_identity(21),
            coordinator_recovery_note=None, handoff=None,
            checkouts=(wrkslots.Checkout(
                name=name, path=path, repository=repo, branch="topic-h" if name == "harness" else "topic-p",
                start_point="1" * 40, remote="origin", remote_url_sha256="2" * 64,
                landed_ref="refs/remotes/origin/main", head="3" * 40,
            ),),
            harness_checkout_reason=reason,
        )

    harness_rec = rec("slot01", "harness", "harness-src", "worktrees/slots/slot01/harness", "legacy")
    product_rec = rec("slot02", "product", "product-src", "worktrees/slots/slot02/product", None)
    assert wrkslots._is_harness_checkout(config, harness_rec)
    assert not wrkslots._is_harness_checkout(config, product_rec)
    info = wrkslots._harness_unpushed_state(config, harness_rec)
    assert info["has_unpushed_work"] is True
    assert info["checkouts"][0]["state"] == "UNPUSHED-OR-STALE"  # type: ignore[index]
    # doctor finding lists only the harness slot, with unpushed flag
    states = [wrkslots.ActiveState(machine="node-a", revision=1, slots=(harness_rec, product_rec))]
    findings = wrkslots._harness_checkout_doctor_findings(config, states)
    assert len(findings) == 1
    assert findings[0]["kind"] == "harness-checkout-slot"
    assert findings[0]["slot"] == "slot01"
    assert "has_unpushed_work=yes" in str(findings[0]["detail"])
