"""Differential: wrkslotsd parse-storage against the Python diagnosis.

#562 rust-primary-wrkslots slice 24. The Python oracle is
`_registry_storage_inconsistencies(..., tolerate_unavailable_repositories=True)`
on fixture projects; the Rust command must agree on the filesystem
findings (slot-directory presence/safety, nested checkout entries,
managed-directory enumeration). Git registration/identity findings
stay with Python and are filtered out of the oracle here.
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

from wrkslots import cli as wrkslots_cli


AU_ROOT = Path(__file__).resolve().parents[3]
SHA = "a" * 40
DIGEST = "b" * 64
KINDS = {
    "row-without-directory",
    "unsafe-slot-directory",
    "missing-checkout",
    "unexpected-entry",
    "slot-unreadable",
    "directory-without-row",
    "unsafe-slots-directory",
    "slots-directory-unreadable",
    "image-not-mounted",
}


def _root(tmp_path: Path) -> Path:
    root = tmp_path / "project"
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
    return root


def _identity() -> dict[str, object]:
    return {
        "pid": 123,
        "start_ticks": 1123,
        "boot_id": "boot-1",
        "host_id": "host-1",
        "cgroup_path": "/user.slice/session.scope",
    }


def _record(slot: str, slot_type: str = "agent") -> dict[str, object]:
    record: dict[str, object] = {
        "slot": slot,
        "agent": f"agent-{slot}",
        "task": "do work",
        "purpose": "testing",
        "machine": "node-a",
        "generation": 1,
        "created_at": "2026-10-26T00:00:00Z",
        "heartbeat_at": "2026-10-26T01:00:00Z",
        "heartbeat_ttl_seconds": 60,
        "owner": _identity(),
        "coordinator_lease": _identity(),
        "coordinator_recovery_note": None,
        "handoff": None,
        "checkouts": [
            {
                "name": "repo",
                "path": f"worktrees/{slot}/repo",
                "repository": "../upstream-repo",
                "branch": "suite/work",
                "start_point": SHA,
                "remote": "origin",
                "remote_url_sha256": DIGEST,
                "landed_ref": "refs/remotes/origin/main",
                "head": SHA,
                "containing_remote_refs": ["refs/remotes/origin/main"],
                "vcs": "git",
            }
        ],
    }
    if slot_type != "agent":
        record["slot_type"] = slot_type
        record["checkouts"] = [
            {
                "name": "repo",
                "path": f"worktrees/validate/{slot}/repo",
                "repository": "../upstream-repo",
                "branch": "suite/work",
                "start_point": SHA,
                "remote": "origin",
                "remote_url_sha256": DIGEST,
                "landed_ref": "refs/remotes/origin/main",
                "head": SHA,
                "containing_remote_refs": ["refs/remotes/origin/main"],
                "vcs": "git",
            }
        ]
    return record


def _fixture(root: Path) -> None:
    control = root / "worktrees"
    records = [
        _record("slot-a"),
        _record("slot-b"),
        _record("slot-c"),
        _record("slot-d", "validate"),
    ]
    (control / "ACTIVE.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 7, "slots": records})
    )
    (control / "ARCHIVED.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 3, "records": []})
    )
    slot_a = control / "slot-a"
    (slot_a / "repo").mkdir(parents=True)
    (slot_a / "stray").mkdir()
    (slot_a / "HANDOFF.md").write_text("hi")
    (control / "slot-c").mkdir()
    (control / "validate" / "slot-d" / "repo").mkdir(parents=True)
    (control / "orphan").mkdir()
    (control / "validate" / "orphan-v").mkdir()


def _expected(root: Path) -> dict[str, object]:
    config = wrkslots_cli._load_config(str(root), None)
    states = [
        wrkslots_cli._load_active_snapshot(config, machine, require_repository=False)
        for machine in wrkslots_cli._state_machines(config)
    ]
    findings = [
        item.to_obj()
        for item in wrkslots_cli._registry_storage_inconsistencies(
            config, states, tolerate_unavailable_repositories=True
        )
        if item.kind in KINDS
    ]
    return {"machines": [state.machine for state in states], "findings": findings}


def _rust(root: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["cargo", "run", "--quiet", "--release", "--manifest-path", "rs/Cargo.toml",
         "-p", "wrkslotsd", "--", "parse-storage", "--root", str(root)],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
    )


def test_parse_storage_matches_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _fixture(root)
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    actual = json.loads(result.stdout)
    assert actual == _expected(root)
    kinds = [item["kind"] for item in actual["findings"]]
    assert kinds == [
        "directory-without-row",
        "directory-without-row",
        "unexpected-entry",
        "row-without-directory",
        "missing-checkout",
    ]


def test_parse_storage_empty_images_directory_matches_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _fixture(root)
    (root / "worktrees" / "slot-images").mkdir()
    (root / "worktrees" / "slot-a" / "it's").mkdir()
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == _expected(root)


def test_parse_storage_unmounted_image_matches_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _fixture(root)
    control = root / "worktrees"
    (control / "slot-b").mkdir()
    image_dir = control / "slot-images" / "agent" / "slot-b"
    image_dir.mkdir(parents=True)
    (image_dir / "slot.img").write_bytes(b"")
    (image_dir / "IMAGE.json").write_text(
        json.dumps(
            {
                "schema": "wrkslots-slot-image/v1",
                "slot": "slot-b",
                "slot_type": "agent",
                "phase": "ready",
                "backend": "kernel",
                "location": str(control / "slot-b"),
                "created_at": "2026-10-26T00:00:00Z",
                "ceiling_bytes": 1024,
                "state_ceiling_bytes": 1024,
            }
        )
    )
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    actual = json.loads(result.stdout)
    assert actual == _expected(root)
    slot_b = [item["kind"] for item in actual["findings"] if item["slot"] == "slot-b"]
    assert slot_b == ["image-not-mounted"]


def test_parse_storage_bad_image_record_matches_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _fixture(root)
    control = root / "worktrees"
    (control / "slot-b").mkdir()
    image_dir = control / "slot-images" / "agent" / "slot-b"
    image_dir.mkdir(parents=True)
    (image_dir / "slot.img").write_bytes(b"")
    record = {
        "schema": "bogus",
        "slot": "slot-b",
        "slot_type": "agent",
        "phase": "ready",
        "backend": "kernel",
        "location": str(control / "slot-b"),
        "created_at": "2026-10-26T00:00:00Z",
        "ceiling_bytes": 1024,
        "state_ceiling_bytes": 1024,
    }
    (image_dir / "IMAGE.json").write_text(json.dumps(record))
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    actual = json.loads(result.stdout)
    assert actual == _expected(root)
    slot_b = [item for item in actual["findings"] if item["slot"] == "slot-b"]
    assert slot_b[0]["kind"] == "image-not-mounted"
    assert "cannot read the slot image record of slot-b" in slot_b[0]["detail"]
    # A non-canonical location refuses the same way, never silently
    # treating the slot as not image-backed.
    record["schema"] = "wrkslots-slot-image/v1"
    record["location"] = str(control) + "/../worktrees/slot-b"
    (image_dir / "IMAGE.json").write_text(json.dumps(record))
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    actual = json.loads(result.stdout)
    assert actual == _expected(root)
    slot_b = [item for item in actual["findings"] if item["slot"] == "slot-b"]
    assert "has a non-canonical location" in slot_b[0]["detail"]


def test_parse_storage_clean_registry(tmp_path: Path) -> None:
    root = _root(tmp_path)
    control = root / "worktrees"
    (control / "ACTIVE.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 0, "slots": []})
    )
    (control / "ARCHIVED.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 0, "records": []})
    )
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == {"machines": ["node-a"], "findings": []}
