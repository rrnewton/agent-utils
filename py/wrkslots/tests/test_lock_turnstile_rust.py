"""Differential: the Rust writer turnstile against Python readers.

#1199 rust-writer-turnstile (Python side #1172). A Rust writer
waiting for the registry lock must hold ACTIVE.global.intent.lock
exclusively, so new Python readers queue behind it instead of
starving it; once the readers inside release, it acquires within
a bounded time.
"""

from __future__ import annotations

import fcntl
import json
import os
import subprocess
import threading
import time
from pathlib import Path

import pytest
from wrkslots import cli as wrkslots_cli

from wrkslots.tests.rust_binary import rust_wrkslotsd


AU_ROOT = Path(__file__).resolve().parents[3]
SHA = "a" * 40
DIGEST = "b" * 64


def _fixture(tmp_path: Path) -> Path:
    root = tmp_path / "project"
    control = root / "worktrees"
    control.mkdir(parents=True)
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
    identity = wrkslots_cli._read_process_identity(os.getpid())
    lease = {
        "pid": os.getpid(),
        "start_ticks": identity.start_ticks,
        "boot_id": identity.boot_id,
        "host_id": identity.host_id,
        "cgroup_path": identity.cgroup_path,
    }
    record = {
        "slot": "slot-a",
        "agent": "agent-a",
        "task": "do work",
        "purpose": "testing",
        "slot_type": "agent",
        "machine": "node-a",
        "generation": 1,
        "created_at": "2026-11-07T00:00:00+00:00",
        "heartbeat_at": "2026-11-07T00:00:00+00:00",
        "heartbeat_ttl_seconds": 60,
        "owner": lease,
        "coordinator_lease": lease,
        "coordinator_recovery_note": None,
        "handoff": None,
        "checkouts": [
            {
                "name": "repo",
                "path": "worktrees/slot-a/repo",
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
    (control / "ACTIVE.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 1, "slots": [record]})
    )
    (control / "ARCHIVED.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 0, "records": []})
    )
    upstream = tmp_path / "upstream-repo"
    upstream.mkdir()

    def git(*args: str) -> None:
        subprocess.run(
            ["git", "-C", str(upstream), *args],
            check=True, capture_output=True, text=True, timeout=60,
            env={
                **os.environ,
                "GIT_AUTHOR_NAME": "Suite",
                "GIT_AUTHOR_EMAIL": "suite@example.invalid",
                "GIT_COMMITTER_NAME": "Suite",
                "GIT_COMMITTER_EMAIL": "suite@example.invalid",
            },
        )

    git("init", "-q", "-b", "main")
    (upstream / "file.txt").write_text("one\n")
    git("add", "file.txt")
    git("commit", "-q", "-m", "one")
    git("branch", "suite/work")
    git("remote", "add", "origin", str(upstream))
    git("fetch", "-q", "origin", "+refs/heads/*:refs/remotes/origin/*")
    head = subprocess.run(
        ["git", "-C", str(upstream), "rev-parse", "HEAD"],
        check=True, capture_output=True, text=True, timeout=60,
    ).stdout.strip()
    git(
        "worktree", "add", "-q", str(control / "slot-a" / "repo"), "suite/work"
    )
    record_path = control / "ACTIVE.node-a.json"
    document = json.loads(record_path.read_text())
    for slot in document["slots"]:
        for checkout in slot["checkouts"]:
            checkout["head"] = head
            checkout["start_point"] = head
    record_path.write_text(json.dumps(document))
    return root


def test_rust_writer_holds_intent_and_is_not_starved(tmp_path: Path) -> None:
    root = _fixture(tmp_path)
    control = root / "worktrees"
    subject = control / "ACTIVE"
    lock_path = control / "ACTIVE.global.lock"
    fd = os.open(lock_path, os.O_RDWR | os.O_CREAT, 0o600)
    fcntl.flock(fd, fcntl.LOCK_SH)
    env = dict(os.environ)
    env["WRKSLOTSD_FIXTURE_WRITER"] = "1"
    env["WRKSLOTS_WAIT_LOCK"] = "20"
    writer = subprocess.Popen(
        [
            str(rust_wrkslotsd()), "hold", "slot-a", "--reason", "turnstile probe",
            "--root", str(root),
        ],
        cwd=AU_ROOT, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env,
    )
    try:
        time.sleep(1.0)
        assert writer.poll() is None, "writer must still be waiting"
        # The waiting writer holds the intent lock exclusively.
        intent_fd = os.open(control / "ACTIVE.global.intent.lock", os.O_RDWR)
        try:
            with pytest.raises(BlockingIOError):
                fcntl.flock(intent_fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        finally:
            os.close(intent_fd)
        # A new Python reader queues behind the waiting writer.
        acquired = threading.Event()

        def reader() -> None:
            with wrkslots_cli._locked(subject, exclusive=False, wait_seconds=20):
                acquired.set()

        thread = threading.Thread(target=reader, daemon=True)
        thread.start()
        time.sleep(0.7)
        assert not acquired.is_set(), "reader must queue behind the waiting writer"
        started = time.monotonic()
        fcntl.flock(fd, fcntl.LOCK_UN)
        out, err = writer.communicate(timeout=30)
        waited = time.monotonic() - started
        assert writer.returncode == 0, err
        assert waited < 5.0, waited
        thread.join(timeout=30)
        assert acquired.is_set()
    finally:
        os.close(fd)
        if writer.poll() is None:
            writer.kill()
