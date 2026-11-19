"""Absent-slot recovery closure (B2-A): owners dead + two clean scans.

Real processes throughout: a recorded owner that forks a descendant
and dies, the descendant holding a descriptor into the slot, and the
slot directory then deleted.  The descendant pins only unlinked
inodes, so recovery may proceed; a live reference to an existing
slot, or an owner not proven dead, still refuses.
"""

from __future__ import annotations

import dataclasses
import os
import shutil
import signal
import subprocess
import sys
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path

import pytest

from wrkslots import cli
from wrkslots.tests.test_lifecycle import (
    active_slots,
    checkout,
    create,
    make_project,
)

pytestmark = pytest.mark.skipif(
    not cli._has_identity_user_namespace(),
    reason="the census commands are trusted only in the initial identity user namespace",
)

_HOLDER = """
import os, sys, time

slot = sys.argv[1]
os.chdir(slot)
fd = os.open("held.txt", os.O_RDWR | os.O_CREAT, 0o644)
sys.stdout.write("ready\\n")
sys.stdout.flush()
if sys.stdin.readline():
    if os.fork() == 0:
        sys.stdout.write(f"{os.getpid()}\\n")
        sys.stdout.flush()
        while True:
            time.sleep(60)
    os._exit(0)
"""


@contextmanager
def _record_with_owner(
    tmp_path: Path,
) -> Iterator[tuple[cli.Config, cli.ActiveRecord, Path, subprocess.Popen[str]]]:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    slot_path = checkout(project, "slot01")
    (slot_path / "held.txt").write_text("held\n", encoding="utf-8")
    parent = subprocess.Popen(
        [sys.executable, "-I", "-c", _HOLDER, str(slot_path)],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=True,
        cwd="/",
    )
    try:
        assert parent.stdout is not None
        assert parent.stdout.readline() == "ready\n"
        config = cli._load_config(str(project), "testhost")
        rows = active_slots(project)
        record = next(
            cli._record_from_obj(row, "active row")
            for row in rows
            if isinstance(row, dict) and row.get("slot") == "slot01"
        )
        identity = cli._read_process_identity(parent.pid)
        owned = dataclasses.replace(record, owner=identity)
        yield config, owned, slot_path, parent
    finally:
        parent.kill()
        parent.wait(timeout=10)


def test_reparented_descendant_holding_a_deleted_fd_does_not_block(
    tmp_path: Path,
) -> None:
    with _record_with_owner(tmp_path) as (config, record, slot_path, parent):
        assert parent.stdin is not None and parent.stdout is not None
        parent.stdin.write("fork\n")
        parent.stdin.flush()
        child = int(parent.stdout.readline())
        assert parent.wait(timeout=10) == 0
        try:
            shutil.rmtree(slot_path)
            # The owner is proven dead and the only reference left is
            # the descendant's descriptor into the deleted tree.
            cli._assert_absent_slot_unused(config, record, (slot_path,))
        finally:
            os.kill(child, signal.SIGKILL)


def test_a_live_reference_to_an_existing_slot_still_refuses(tmp_path: Path) -> None:
    with _record_with_owner(tmp_path) as (config, record, slot_path, parent):
        assert parent.stdin is not None and parent.stdout is not None
        parent.stdin.write("fork\n")
        parent.stdin.flush()
        child = int(parent.stdout.readline())
        assert parent.wait(timeout=10) == 0
        try:
            with pytest.raises(cli.Refusal, match="may still use absent slot"):
                cli._assert_absent_slot_unused(config, record, (slot_path,))
        finally:
            os.kill(child, signal.SIGKILL)


def test_an_owner_not_proven_dead_still_refuses(tmp_path: Path) -> None:
    with _record_with_owner(tmp_path) as (config, record, slot_path, _parent):
        with pytest.raises(cli.Refusal, match="not proven dead"):
            cli._assert_absent_slot_unused(config, record, (slot_path,))
