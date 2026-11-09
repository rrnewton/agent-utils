"""Differential: wrkslotsd quarantine-journal against Python (#1086c).

The Python oracle quarantines a dead-owner create journal whose slot
already has an archive record (classification and recover both refuse
it); the Rust writer runs on a twin fixture. Dry-run and act payloads,
the tombstone event, the post-quarantine classification, the crash
window (tombstone written, move not yet done), and the documented
hand restore must agree apart from fixture roots, ages, and the
quarantine filename timestamp. Temporary fixture registries only.
"""

from __future__ import annotations

import contextlib
import hashlib
import io
import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path
from typing import cast

from wrkslots import cli as wrkslots_cli

from wrkslots.tests.rust_binary import rust_wrkslotsd
from wrkslots.tests.test_classify_journals_rust import _norm
from wrkslots.tests.test_create_text_rust import _fixture

AU_ROOT = Path(__file__).resolve().parents[3]
CLI = AU_ROOT / "py" / "wrkslots" / "cli.py"

_CREATE_CODE = """
import os, sys
sys.path.insert(0, {py!r})
from wrkslots import cli
rc = cli.main([
    "--project-root", sys.argv[1], "--machine", "node-a",
    "create", "slot-b", "--slot-type", "agent", "--coordinator-authorized",
    "--agent", "agent-new", "--task", "task-1", "--purpose", "testing",
    "--owner-pid", str(os.getpid()), "--coordinator-pid", str(os.getpid()),
    "--repo", "repo=../upstream-repo", "--branch", "repo=suite/new",
])
raise SystemExit(rc)
""".format(py=str(AU_ROOT / "py"))


def _dead_interrupted(root: Path) -> Path:
    """Interrupted create whose owner/coordinator process has exited."""
    env = dict(os.environ)
    env["WRKSLOTS_TEST_INTERRUPT"] = "after-create-worktree"
    env["WRKSLOTS_INIT_REPRESENTATION"] = "worktree"
    result = subprocess.run(
        [sys.executable, "-c", _CREATE_CODE, str(root)],
        capture_output=True, text=True, check=False, timeout=900, env=env,
    )
    assert result.returncode == 86, result.stderr
    journal = next((root / "worktrees").glob("CREATE.*.journal"))
    old = time.time() - 3 * 86400
    os.utime(journal, (old, old))
    return journal


def _archive_claim(root: Path) -> None:
    journal = next((root / "worktrees").glob("CREATE.*.journal"))
    raw = json.loads(journal.read_text(encoding="utf-8"))
    config = wrkslots_cli._load_config(str(root), "node-a")
    finished = "2026-10-26T02:00:00Z"
    entry: dict[str, object] = {
        "archive_id": f"node-a:slot-b:1:{finished}",
        "slot": "slot-b",
        "agent": "agent-new",
        "task": "task-1",
        "purpose": "testing",
        "machine": "node-a",
        "generation": 1,
        "created_at": "2026-10-26T00:00:00Z",
        "finished_at": finished,
        "mode": "remove",
        "actor": "coordinator",
        "physical_storage": "removed",
        "validation": ["archived before the interrupted create"],
        "limitations": [],
        "continuation": "none needed",
        "checkouts": raw["created"],
    }
    archive = wrkslots_cli._load_archive(config)
    wrkslots_cli._write_archive_state(
        config,
        wrkslots_cli.ArchiveState(
            archive.machine, archive.revision + 1, (*archive.records, entry)
        ),
        action="slot-archived",
        slot="slot-b",
        evidence={"archive_id": entry["archive_id"]},
    )


def _main(root: Path, *args: str) -> tuple[int, str, str]:
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        rc = wrkslots_cli.main(
            ["--project-root", str(root), "--machine", "node-a", *args]
        )
    return rc, out.getvalue(), err.getvalue()


def _classify(root: Path) -> tuple[int, str, str]:
    return _main(
        root,
        "classify-create-journals", "slot-c", "--slot-type", "agent",
        "--agent", "agent-other", "--format", "human",
    )


def _quarantine_args(*extra: str) -> list[str]:
    return [
        "quarantine-journal", "slot-b", "--coordinator-authorized",
        "--coordinator-pid", str(os.getpid()),
        "--reason", "archived slot name reused; worktrees preserved",
        *extra,
    ]


def _rust(root: Path, *args: str) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    env["WRKSLOTSD_FIXTURE_WRITER"] = "1"
    return subprocess.run(
        [str(rust_wrkslotsd()), *args, "--machine", "node-a", "--root", str(root)],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
        env=env,
    )


def _scrub(text: str, root: Path) -> str:
    text = _norm(text, root)
    text = re.sub(r'"age_seconds": \d+', '"age_seconds": 0', text)
    text = re.sub(r'"(coordinator|owner)_pid": \d+', r'"\1_pid": 0', text)
    text = text.replace('"', "'").replace("\\'", "'")
    text = re.sub(r"\b[0-9a-f]{64}\b", "SHA256", text)
    text = re.sub(r"age=\d+s", "age=Ns", text)
    return re.sub(r"\d{8}T\d{6}Z-", "TS-", text)


def _events(root: Path) -> list[dict[str, object]]:
    directory = root / "worktrees" / "EVENTS.node-a"
    return [
        json.loads(path.read_text(encoding="utf-8"))
        for path in sorted(directory.glob("*.json"))
    ]


def _tombstone(root: Path) -> dict[str, object]:
    return next(
        event for event in _events(root)
        if event.get("kind") == "operation-quarantined"
    )


def _tombstone_payload(root: Path) -> dict[str, object]:
    return cast(dict[str, object], _tombstone(root)["payload"])


def test_quarantine_archive_claim_both_editions_and_hand_restore(
    tmp_path: Path,
) -> None:
    rust_root = _fixture(tmp_path / "rust")
    oracle_root = _fixture(tmp_path / "oracle")
    for root in (rust_root, oracle_root):
        _dead_interrupted(root)
        _archive_claim(root)
    rc_before, _out, err_before = _classify(oracle_root)
    assert rc_before == 3 and "conflicts with an archive" in err_before

    rc, out, err = _main(oracle_root, *_quarantine_args("--format", "json"))
    assert rc == 0, err
    dry = json.loads(out)
    assert dry["action"] == "dry-run"
    assert dry["recover_refusal"] == (
        "create journal for slot 'slot-b' conflicts with an archive"
    )
    assert dry["owner_state"] == "dead"
    artifacts = {item["kind"]: item for item in dry["artifacts"]}
    assert artifacts["worktree"]["exists"] is True
    assert artifacts["worktree"]["branch_exists"] is True
    assert artifacts["archive-record"]["exists"] is True
    assert artifacts["active-row"]["exists"] is False

    result = _rust(rust_root, *_quarantine_args("--format", "json"))
    assert result.returncode == 0, result.stderr
    assert _scrub(result.stdout, rust_root) == _scrub(out, oracle_root)
    rust_dry = json.loads(result.stdout)

    rc, out, err = _main(
        oracle_root,
        *_quarantine_args(
            "--yes", "--journal", dry["journal"],
            "--journal-sha256", dry["journal_sha256"], "--format", "json",
        ),
    )
    assert rc == 0, err
    acted = json.loads(out)
    result = _rust(
        rust_root,
        *_quarantine_args(
            "--yes", "--journal", rust_dry["journal"],
            "--journal-sha256", rust_dry["journal_sha256"], "--format", "json",
        ),
    )
    assert result.returncode == 0, result.stderr
    assert _scrub(result.stdout, rust_root) == _scrub(out, oracle_root)
    rust_acted = json.loads(result.stdout)
    for payload in (acted, rust_acted):
        assert Path(payload["quarantine_path"]).is_file()
        assert not Path(payload["journal"]).exists()
    for root in (rust_root, oracle_root):
        assert _tombstone_payload(root)["journal_sha256"] == (
            dry["journal_sha256"] if root == oracle_root
            else rust_dry["journal_sha256"]
        )
    # Both editions read both resulting registries as unblocked.
    for root in (rust_root, oracle_root):
        rc_after, out_after, err_after = _classify(root)
        assert rc_after == 0 and "NONBLOCKING" in out_after, err_after

    # Hand restore, exactly the user guide steps: verify the digest with
    # sha256 of the quarantined file, move it back to original_path, and
    # classification reports the journal exactly as before quarantine.
    payload = _tombstone_payload(oracle_root)
    quarantined = Path(cast(str, payload["quarantine_path"]))
    digest = hashlib.sha256(quarantined.read_bytes()).hexdigest()
    assert digest == payload["journal_sha256"]
    quarantined.rename(Path(cast(str, payload["original_path"])))
    rc_restored, _out, err_restored = _classify(oracle_root)
    assert rc_restored == 3
    assert _scrub(err_restored, oracle_root) == _scrub(err_before, oracle_root)


def test_quarantine_crash_window_completes_idempotently(tmp_path: Path) -> None:
    root = _fixture(tmp_path / "crash")
    journal = _dead_interrupted(root)
    _archive_claim(root)
    sha = hashlib.sha256(journal.read_bytes()).hexdigest()
    rc_before, _out, err_before = _classify(root)
    assert rc_before == 3

    env = dict(os.environ)
    env["WRKSLOTS_TEST_INTERRUPT"] = "after-operation-quarantined"
    env["PYTHONPATH"] = str(AU_ROOT / "py")
    crash = subprocess.run(
        [
            sys.executable, str(CLI), "--project-root", str(root),
            "--machine", "node-a",
            *_quarantine_args(
                "--yes", "--journal", str(journal),
                "--journal-sha256", sha,
            ),
        ],
        capture_output=True, text=True, check=False, timeout=900, env=env,
    )
    assert crash.returncode == 86, crash.stderr
    assert journal.is_file(), "crash left the journal in place"
    assert _tombstone_payload(root)["journal_sha256"] == sha

    # Classify reports the crash-window state exactly as before: the same
    # named host-fault refusal, not a generic missing-journal error.
    rc_during, _out, err_during = _classify(root)
    assert rc_during == 3
    assert _scrub(err_during, root) == _scrub(err_before, root)

    rc, out, err = _main(root, *_quarantine_args("--format", "json"))
    assert rc == 0, err
    plan = json.loads(out)
    assert plan["already_journaled_sequence"] is not None

    events_before = len(_events(root))
    result = _rust(
        root,
        *_quarantine_args(
            "--yes", "--journal", str(journal),
            "--journal-sha256", sha, "--format", "json",
        ),
    )
    assert result.returncode == 0, result.stderr
    acted = json.loads(result.stdout)
    assert acted["event_sequence"] == plan["already_journaled_sequence"]
    assert len(_events(root)) == events_before, "no duplicate tombstone"
    assert Path(acted["quarantine_path"]).is_file()
    assert not journal.exists()
    rc_after, out_after, err_after = _classify(root)
    assert rc_after == 0 and "NONBLOCKING" in out_after, err_after


def test_quarantine_points_at_recover_for_resolvable_dead_orphan(
    tmp_path: Path,
) -> None:
    for label in ("oracle", "rust"):
        root = _fixture(tmp_path / label)
        _dead_interrupted(root)
        if label == "oracle":
            rc, _out, err = _main(root, *_quarantine_args("--format", "json"))
            assert rc == 3, err
            assert "which recover resolves" in err
        else:
            result = _rust(root, *_quarantine_args("--format", "json"))
            assert result.returncode == 1, result.stderr
            assert "which recover resolves" in result.stderr


def test_quarantine_refuses_young_and_live_journals(tmp_path: Path) -> None:
    young = _fixture(tmp_path / "young")
    journal = _dead_interrupted(young)
    _archive_claim(young)
    fresh = time.time()
    os.utime(journal, (fresh, fresh))
    rc, _out, err = _main(young, *_quarantine_args())
    assert rc == 3 and "too young to quarantine" in err, err
    assert "bound 86400s (max(24h, heartbeat_ttl_seconds=60))" in err
    result = _rust(young, *_quarantine_args())
    assert result.returncode == 1
    assert "too young to quarantine" in result.stderr

    live_root = _fixture(tmp_path / "live")
    # An in-process interrupted create records this live test process.
    from wrkslots.tests.test_classify_journals_rust import _interrupted

    _interrupted(live_root)
    _archive_claim(live_root)
    live_journal = next((live_root / "worktrees").glob("CREATE.*.journal"))
    old = time.time() - 3 * 86400
    os.utime(live_journal, (old, old))
    rc, _out, err = _main(live_root, *_quarantine_args())
    assert rc == 3 and "owners are not both proven dead" in err, err
    result = _rust(live_root, *_quarantine_args())
    assert result.returncode == 1
    assert "owners are not both proven dead" in result.stderr


def test_quarantined_journals_visible_in_doctor_and_audit(
    tmp_path: Path,
) -> None:
    root = _fixture(tmp_path / "visible")
    _dead_interrupted(root)
    _archive_claim(root)
    rc, out, err = _main(root, *_quarantine_args("--format", "json"))
    assert rc == 0, err
    dry = json.loads(out)
    rc, _out, err = _main(
        root,
        *_quarantine_args(
            "--yes", "--journal", dry["journal"],
            "--journal-sha256", dry["journal_sha256"],
        ),
    )
    assert rc == 0, err
    rc, out, err = _main(root, "doctor", "--format", "json")
    assert rc == 0, err
    findings = json.loads(out)["findings"]
    quarantined = [
        item for item in findings if item["kind"] == "quarantined-journal"
    ]
    assert len(quarantined) == 1 and quarantined[0]["slot"] == "slot-b"
    result = _rust(root, "doctor-text", "--format", "json")
    assert result.returncode == 0, result.stderr
    rust_findings = json.loads(result.stdout)["findings"]
    assert any(
        item["kind"] == "quarantined-journal" and item["slot"] == "slot-b"
        for item in rust_findings
    )
    rc, out, err = _main(root, "audit", "--format", "json")
    assert rc == 0, err
    rows = json.loads(out)["quarantined_journals"]
    assert len(rows) == 1 and rows[0]["slot"] == "slot-b"
    assert rows[0]["kind"] == "create"
    result = _rust(root, "audit-text", "--format", "json")
    assert result.returncode == 0, result.stderr
    rust_rows = json.loads(result.stdout)["quarantined_journals"]
    assert len(rust_rows) == 1 and rust_rows[0]["slot"] == "slot-b"
