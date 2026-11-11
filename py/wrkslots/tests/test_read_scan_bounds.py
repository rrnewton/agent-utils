"""Read-only scan lock scope and wall bound (#1247 wrkslots-status-lock).

A read-only scan (status here) measures from a snapshot taken under a short
shared lock; the measurement itself never runs under the lock, so a mutation
can take the exclusive lock while a scan is in progress. A scan past its
--scan-timeout bound reports partial results and exits non-zero. The
algorithmic expectations count Git subprocess calls and record parses, never
wall time.
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import io
import json
import resource
import subprocess
import threading
from pathlib import Path

import pytest
from wrkslots import cli as wrkslots_cli

pytestmark = pytest.mark.usefixtures("pinned_process_evidence")


def _git(cwd: Path, *args: str) -> str:
    return subprocess.run(
        ["git", "-C", str(cwd), *args],
        check=True, capture_output=True, text=True,
    ).stdout.strip()


def _registry(base: Path, slots: int) -> Path:
    root = base / "project"
    (root / "worktrees").mkdir(parents=True)
    (root / "live.py").write_text(
        "#!/usr/bin/env python3\nimport sys\nsys.exit(0)\n"
    )
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
    upstream.mkdir(parents=True)
    _git(upstream, "init", "-q", "-b", "main")
    _git(upstream, "config", "user.email", "test@example.invalid")
    _git(upstream, "config", "user.name", "Test")
    (upstream / "file.txt").write_text("one\n")
    _git(upstream, "add", "file.txt")
    _git(upstream, "commit", "-q", "-m", "one")
    head = _git(upstream, "rev-parse", "HEAD")
    remote = base / "remote.git"
    _git(base, "init", "-q", "--bare", "-b", "main", str(remote))
    _git(upstream, "remote", "add", "origin", str(remote))
    _git(upstream, "push", "-q", "origin", "main")
    proc = subprocess.Popen(["sleep", "30"])
    try:
        identity_obj = wrkslots_cli._read_process_identity(proc.pid)
        identity = {
            "pid": identity_obj.pid,
            "start_ticks": identity_obj.start_ticks,
            "boot_id": identity_obj.boot_id,
            "host_id": identity_obj.host_id,
            "cgroup_path": identity_obj.cgroup_path,
        }
    finally:
        proc.kill()
        proc.wait()
    records = []
    for index in range(slots):
        slot = f"slot-{index:02d}"
        _git(
            upstream, "worktree", "add", "-q", "-b", f"suite/{slot}",
            str(root / "worktrees" / slot / "repo"), "HEAD",
        )
        records.append(
            {
                "slot": slot,
                "agent": f"agent-{slot}",
                "task": "do work",
                "purpose": "testing",
                "slot_type": "agent",
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
                        "path": f"worktrees/{slot}/repo",
                        "repository": "../upstream-repo",
                        "branch": f"suite/{slot}",
                        "start_point": head,
                        "remote": "origin",
                        "remote_url_sha256": hashlib.sha256(
                            str(remote).encode()
                        ).hexdigest(),
                        "landed_ref": "refs/remotes/origin/main",
                        "head": head,
                        "containing_remote_refs": ["refs/remotes/origin/main"],
                        "vcs": "git",
                    }
                ],
            }
        )
    (root / "worktrees" / "ACTIVE.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 7, "slots": records})
    )
    (root / "worktrees" / "ARCHIVED.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 3, "records": []})
    )
    return root


def _status_args(root: Path, **overrides: object) -> argparse.Namespace:
    values: dict[str, object] = {
        "project_root": str(root),
        "machine": None,
        "format": "json",
        "wait_lock": 0.0,
        "all_machines": False,
        "slot": None,
        "verbose": False,
        "scan_timeout": 180.0,
        "scan_memory_limit": 0.0,
    }
    values.update(overrides)
    return argparse.Namespace(**values)


def test_status_scan_in_progress_does_not_block_a_mutation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _registry(tmp_path / "case", 2)
    control = root / "worktrees"
    entered = threading.Event()
    release = threading.Event()
    inside: dict[str, bool] = {}
    original = wrkslots_cli._registry_storage_inconsistencies

    def slow_scan(*args: object, **kwargs: object) -> object:
        # The scan is in progress. A mutation's exclusive lock must still be
        # available: the scan measures outside the snapshot lock.
        try:
            with wrkslots_cli._locked(
                control / "ACTIVE", exclusive=True, wait_seconds=0
            ):
                inside["writer_lock_acquired"] = True
        except wrkslots_cli.Refusal:
            inside["writer_lock_acquired"] = False
        entered.set()
        release.wait(timeout=60)
        return original(*args, **kwargs)  # type: ignore[arg-type]

    monkeypatch.setattr(
        wrkslots_cli, "_registry_storage_inconsistencies", slow_scan
    )
    outcome: dict[str, object] = {}

    def run_status() -> None:
        stream = io.StringIO()
        with contextlib.redirect_stdout(stream), contextlib.redirect_stderr(
            io.StringIO()
        ):
            outcome["rc"] = wrkslots_cli._cmd_status(_status_args(root))

    worker = threading.Thread(target=run_status)
    worker.start()
    try:
        assert entered.wait(timeout=60), "status never reached its scan"
        # While the scan is blocked mid-measurement, a mutation takes the
        # exclusive registry lock at once (wait 0 refuses if it is held).
        with wrkslots_cli._locked(control / "ACTIVE", exclusive=True, wait_seconds=0):
            pass
    finally:
        release.set()
        worker.join(timeout=60)
    assert not worker.is_alive()
    assert inside["writer_lock_acquired"] is True
    assert outcome["rc"] == 0


def test_status_past_scan_timeout_reports_partial_and_exits_nonzero(
    tmp_path: Path,
) -> None:
    root = _registry(tmp_path / "case", 2)
    stream = io.StringIO()
    with contextlib.redirect_stdout(stream), contextlib.redirect_stderr(io.StringIO()):
        rc = wrkslots_cli._cmd_status(_status_args(root, scan_timeout=1e-9))
    assert rc == 2
    payload = json.loads(stream.getvalue())
    assert payload["partial"] is True
    assert "scan_timeout" in payload
    # Rows still come from the snapshot; only the measurements are missing.
    assert len(payload["active"]) == 2


def test_storage_scan_git_calls_grow_per_checkout_not_per_finding(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    counts: dict[int, int] = {}
    for slots in (3, 7):
        root = _registry(tmp_path / f"case-{slots}", slots)
        calls = {"n": 0}
        original_run = wrkslots_cli._GitVcs._run

        def counting(repository: object, args: object, **kwargs: object) -> object:
            calls["n"] += 1
            return original_run(repository, args, **kwargs)  # type: ignore[arg-type]

        monkeypatch.setattr(
            wrkslots_cli._GitVcs, "_run", staticmethod(counting)
        )
        stream = io.StringIO()
        with contextlib.redirect_stdout(stream), contextlib.redirect_stderr(
            io.StringIO()
        ):
            assert wrkslots_cli._cmd_status(_status_args(root)) == 0
        monkeypatch.undo()
        counts[slots] = calls["n"]
    # One worktree-identity probe per checkout plus the two per-repository
    # calls (common directory, worktree list), cached across slots that
    # share the repository: growth is exactly the checkout growth.
    assert counts[3] == 3 + 2
    assert counts[7] == 7 + 2


def test_registry_identity_never_reads_a_large_non_registry_file(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _registry(tmp_path / "case", 1)
    control = root / "worktrees"
    image = control / "worktrees.img"
    with open(image, "wb") as handle:
        handle.truncate(10 * (1 << 30))  # 10 GiB apparent, sparse
    (control / "notes.txt").write_text("not registry state\n")
    config = wrkslots_cli._load_config(str(root), None)
    read_names: list[str] = []
    original_read_bytes = Path.read_bytes

    def counting_read_bytes(self: Path) -> bytes:
        read_names.append(self.name)
        return original_read_bytes(self)

    monkeypatch.setattr(Path, "read_bytes", counting_read_bytes)
    before = wrkslots_cli._registry_read_identity(config)
    assert "worktrees.img" not in read_names, read_names
    assert "notes.txt" not in read_names, read_names
    # Registry content still identifies the identity: changing ACTIVE moves it.
    active = control / "ACTIVE.node-a.json"
    document = json.loads(active.read_text())
    document["revision"] = 8
    active.write_text(json.dumps(document))
    assert wrkslots_cli._registry_read_identity(config) != before


def test_event_retention_stays_flat_as_the_log_grows(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _registry(tmp_path / "case", 1)
    config = wrkslots_cli._load_config(str(root), None)
    # Keep every written event settled, so the memo may keep it at once.
    later = wrkslots_cli._EVENT_MEMO_STABLE_NS * 10
    monkeypatch.setattr(
        wrkslots_cli, "_event_memo_clock_ns", lambda: 10**18 + later
    )
    monkeypatch.setattr(wrkslots_cli, "_EVENT_MEMO_BYTES_LIMIT", 4096)
    monkeypatch.setattr(wrkslots_cli, "_CHAIN_MEMO_EVENTS_LIMIT", 4)
    retained: list[int] = []
    total = 0
    for batch in (30, 120):
        for _ in range(batch):
            wrkslots_cli._write_event_file(config, "node-a", "heartbeat", {})
        total += batch
        with wrkslots_cli._event_memo_scope() as memo:
            events = wrkslots_cli._load_events(config, "node-a")
            assert len(events) == total
            retained.append(memo.file_bytes)
            # No whole chain is kept past the event cap.
            assert memo.chains == {}
    # Retained parsed bytes never pass the cap, however long the log grows.
    assert all(size <= 4096 for size in retained), retained
    # The streaming verifier still sees every record, retaining none.
    assert sum(1 for _ in wrkslots_cli._iter_events(config, "node-a")) == 150


def test_status_memory_bound_reports_partial_and_exits_nonzero(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _registry(tmp_path / "case", 2)

    def exhausted(*args: object, **kwargs: object) -> object:
        raise MemoryError

    monkeypatch.setattr(wrkslots_cli, "_owner_liveness_states", exhausted)
    stream = io.StringIO()
    with contextlib.redirect_stdout(stream), contextlib.redirect_stderr(io.StringIO()):
        rc = wrkslots_cli._cmd_status(_status_args(root))
    assert rc == 2
    payload = json.loads(stream.getvalue())
    assert payload["partial"] is True
    assert "scan_memory_limit" not in payload  # detail lives in scan_timeout
    assert "memory" in payload["scan_timeout"]


def test_scan_memory_limit_applies_an_address_space_cap(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    applied: list[tuple[int, tuple[int, int]]] = []
    monkeypatch.setattr(
        resource,
        "getrlimit",
        lambda which: (0, resource.RLIM_INFINITY),
    )
    monkeypatch.setattr(
        resource,
        "setrlimit",
        lambda which, limits: applied.append((which, limits)),
    )
    wrkslots_cli._apply_scan_memory_limit(
        argparse.Namespace(scan_memory_limit=2.0)
    )
    assert applied == [
        (resource.RLIMIT_AS, (2 * (1 << 30), resource.RLIM_INFINITY))
    ]
    applied.clear()
    wrkslots_cli._apply_scan_memory_limit(
        argparse.Namespace(scan_memory_limit=0.0)
    )
    assert applied == []


def test_handoff_publication_parses_each_historical_record_once(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    proc = subprocess.Popen(["sleep", "30"])
    try:
        owner = wrkslots_cli._read_process_identity(proc.pid)
    finally:
        proc.kill()
        proc.wait()
    record = wrkslots_cli.ActiveRecord(
        slot="slot-a",
        agent="agent-a",
        task="do work",
        purpose="testing",
        slot_type="agent",
        machine="node-a",
        generation=1,
        created_at="2026-10-26T00:00:00Z",
        heartbeat_at="2026-10-26T01:00:00Z",
        heartbeat_ttl_seconds=60,
        owner=owner,
        coordinator_lease=owner,
        coordinator_recovery_note=None,
        handoff=None,
        checkouts=(
            wrkslots_cli.Checkout(
                name="repo",
                path="worktrees/slot-a/repo",
                repository="../upstream-repo",
                branch="suite/work",
                start_point="0" * 40,
                remote="origin",
                remote_url_sha256="0" * 64,
                landed_ref="refs/remotes/origin/main",
                head="0" * 40,
            ),
        ),
    )
    record_obj = wrkslots_cli._record_to_obj(record)
    writer_obj = wrkslots_cli._identity_to_obj(owner)
    events: list[dict[str, object]] = []
    sequence = 0

    def next_seq() -> int:
        nonlocal sequence
        sequence += 1
        return sequence

    historical = 24
    for _ in range(historical):
        events.append(
            {
                "kind": "active-state-recorded",
                "sequence": next_seq(),
                "payload": {"slot": "slot-a", "record": record_obj},
            }
        )
    artifact: dict[str, object] = {}
    intents = 6
    for _ in range(intents):
        artifact = {
            "schema": wrkslots_cli.HANDOFF_SIDECAR_SCHEMA,
            "machine": "node-a",
            "slot": "slot-a",
            "generation": 1,
            "source": "write-handoff",
        }
        intent_seq = next_seq()
        events.append(
            {
                "kind": "handoff-write-intended",
                "sequence": intent_seq,
                "payload": {
                    "slot": "slot-a",
                    "generation": 1,
                    "artifact": artifact,
                    "writer": writer_obj,
                },
            }
        )
        events.append(
            {
                "kind": "handoff-write-completed",
                "sequence": next_seq(),
                "payload": {
                    "slot": "slot-a",
                    "generation": 1,
                    "intent_sequence": intent_seq,
                    "artifact_sha256": wrkslots_cli._event_digest(artifact),
                },
            }
        )
    calls = {"n": 0}
    original_parse = wrkslots_cli._record_from_obj

    def counting(value: object, label: str) -> object:
        calls["n"] += 1
        return original_parse(value, label)

    monkeypatch.setattr(wrkslots_cli, "_record_from_obj", counting)
    publication = wrkslots_cli._handoff_write_publication(record, events)
    assert publication is not None
    assert publication[2] is True
    # Single pass: every historical record is parsed once, however many
    # intents follow it. The per-intent rescan parsed the whole prefix for
    # each intent (here 6 x 24 parses).
    assert calls["n"] == historical
