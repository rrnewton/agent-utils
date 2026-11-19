"""Absent-slot recovery closure (B2-A): owners dead + two clean scans.

Real processes throughout: a recorded owner that forks a descendant
and dies, the descendant holding a descriptor into the slot, and the
slot directory then deleted.  The descendant pins only unlinked
inodes, so recovery may proceed; a live reference to an existing
slot, or an owner not proven dead, still refuses.
"""

from __future__ import annotations

import dataclasses
import json
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


_ROOT_HOLDER = """
import os, sys, time

os.chdir(sys.argv[1])
fd = os.open(".", os.O_RDONLY)
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


def test_reparented_descendant_holding_a_deleted_fd_does_not_block(
    tmp_path: Path,
) -> None:
    with _record_with_owner(tmp_path) as (config, record, slot_path, _parent):
        # The fixture's own holder is not part of this case.
        _parent.kill()
        _parent.wait(timeout=10)
        # The exempted reference is the slot root itself, deleted;
        # the descendant's working directory is that root.
        root = cli._slot_directory(config, "slot01", record.slot_type)
        holder = subprocess.Popen(
            [sys.executable, "-I", "-c", _ROOT_HOLDER, str(root)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True,
            cwd="/",
        )
        try:
            assert holder.stdout is not None and holder.stdin is not None
            assert holder.stdout.readline() == "ready\n"
            record = dataclasses.replace(
                record, owner=cli._read_process_identity(holder.pid)
            )
            holder.stdin.write("fork\n")
            holder.stdin.flush()
            child = int(holder.stdout.readline())
            assert holder.wait(timeout=10) == 0
            try:
                shutil.rmtree(root)
                # The owner is proven dead and the only reference left
                # is the descendant's descriptor into the deleted root.
                cli._assert_absent_slot_unused(config, record, (root,))
            finally:
                os.kill(child, signal.SIGKILL)
        finally:
            holder.kill()
            holder.wait(timeout=10)


def test_a_live_reference_to_an_existing_slot_still_refuses(tmp_path: Path) -> None:
    with _record_with_owner(tmp_path) as (config, record, slot_path, parent):
        assert parent.stdin is not None and parent.stdout is not None
        parent.stdin.write("fork\n")
        parent.stdin.flush()
        child = int(parent.stdout.readline())
        assert parent.wait(timeout=10) == 0
        try:
            with pytest.raises(cli.Refusal, match="uses slot "):
                cli._assert_absent_slot_unused(config, record, (slot_path,))
        finally:
            os.kill(child, signal.SIGKILL)


def test_an_owner_not_proven_dead_still_refuses(tmp_path: Path) -> None:
    with _record_with_owner(tmp_path) as (config, record, slot_path, _parent):
        with pytest.raises(cli.Refusal, match="not proven dead"):
            cli._assert_absent_slot_unused(config, record, (slot_path,))


_SLEEPER = """
import time

while True:
    time.sleep(60)
"""


def _interrupt_with_dead_owner(
    project: Path, monkeypatch: pytest.MonkeyPatch
) -> Path:
    """Create the sealed validate slot from a short-lived subprocess (so
    its recorded owner is genuinely dead), then interrupt its private
    finish after the fence; return the fenced path."""
    from wrkslots.tests.test_lifecycle import (
        _FinishInterrupted,
        finish_journal_path,
        remove_completed_validation,
    )

    owner = subprocess.Popen(
        [sys.executable, "-I", "-c", "import time; time.sleep(60)"], cwd="/"
    )
    try:
        assert (
            create(
                project, slot="target", slot_type="validate", branch=None,
                owner_pid=owner.pid,
            ).returncode
            == 0
        )
    finally:
        owner.kill()
        owner.wait(timeout=10)

    def interrupt(point: str) -> None:
        if point == "after-path-fence":
            raise _FinishInterrupted

    monkeypatch.setattr(cli, "_interrupt_for_test", interrupt)
    with pytest.raises(_FinishInterrupted):
        remove_completed_validation(project)
    monkeypatch.setattr(cli, "_interrupt_for_test", lambda _point: None)
    config = cli._load_config(str(project), "testhost")
    scoped = json.loads(
        finish_journal_path(project, "target").read_text(encoding="utf-8")
    )
    assert "private_census_identity" in scoped
    target = cli._slot_directory(config, "target", "validate")
    fenced = tuple(target.parent.glob(".target.fenced.1.*"))
    assert len(fenced) == 1
    return fenced[0]


def test_recover_absent_sealed_target_with_a_deleted_fd_descendant(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Wiring (A): an interrupted sealed finish whose target is absent,
    its recorded owner genuinely dead, and the only reference left a
    descendant's descriptor into the deleted fenced root: recover
    completes through the absent-slot closure."""
    from wrkslots.tests.test_lifecycle import recover_scoped_target

    project, _repository, _remote = make_project(tmp_path)
    fenced = _interrupt_with_dead_owner(project, monkeypatch)
    holder = subprocess.Popen(
        [sys.executable, "-I", "-c", _ROOT_HOLDER, str(fenced)],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=True,
        cwd="/",
    )
    try:
        assert holder.stdout is not None and holder.stdin is not None
        assert holder.stdout.readline() == "ready\n"
        holder.stdin.write("fork\n")
        holder.stdin.flush()
        child = int(holder.stdout.readline())
        assert holder.wait(timeout=10) == 0
        try:
            shutil.rmtree(fenced)
            # The recovery's preparation censuses can hit the
            # privileged census stderr bound under unrelated host
            # churn; that refusal names its remedy (retry), so retry
            # it a bounded number of times.  The absent-slot closure
            # itself is what this test judges.
            code = 3
            for _attempt in range(3):
                code = recover_scoped_target(project)
                if code == 0:
                    break
            assert code == 0
            assert active_slots(project) == []
        finally:
            os.kill(child, signal.SIGKILL)
    finally:
        holder.kill()
        holder.wait(timeout=10)


def test_recover_refuses_a_spoofed_deleted_directory_name(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Wiring (A), a guard: an existing directory literally named
    "<fenced>/build (deleted)" with a live holder is not the exempted
    deleted root: recover refuses.  (This also passes on the pre-B2
    base, where recover refuses for other reasons; the case that shows
    the exemption itself is not spoofable by name is the unit-level
    holder in test_slice_fallbacks.py and the deleted-root proof in
    _deleted_target_match_is_exempt.)"""
    from wrkslots.tests.test_lifecycle import (
        interrupt_scoped_private_finish,
        recover_scoped_target,
    )

    project, _repository, _remote = make_project(tmp_path)
    fenced = _interrupt_with_dead_owner(project, monkeypatch)
    spoof = fenced / "build (deleted)"
    spoof.mkdir()
    holder = subprocess.Popen(
        [sys.executable, "-I", "-c", _ROOT_HOLDER, str(spoof)],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=True,
        cwd="/",
    )
    try:
        assert holder.stdout is not None
        assert holder.stdout.readline() == "ready\n"
        assert recover_scoped_target(project) != 0
    finally:
        holder.kill()
        holder.wait(timeout=10)


def test_a_spoofed_deleted_sibling_directory_still_refuses(
    tmp_path: Path,
) -> None:
    """The exemption is not a name check: a live holder whose working
    directory is an existing sibling literally named "<root> (deleted)"
    still refuses, because its inode is linked (the held deleted root's
    inode has no links left)."""
    with _record_with_owner(tmp_path) as (config, record, slot_path, parent):
        parent.kill()
        parent.wait(timeout=10)
        root = cli._slot_directory(config, "slot01", record.slot_type)
        owner = subprocess.Popen(
            [sys.executable, "-I", "-c", _SLEEPER], cwd="/"
        )
        identity = cli._read_process_identity(owner.pid)
        owner.kill()
        owner.wait(timeout=10)
        record = dataclasses.replace(record, owner=identity)
        spoof = Path(f"{root} (deleted)")
        spoof.mkdir()
        holder = subprocess.Popen(
            [sys.executable, "-I", "-c", _ROOT_HOLDER, str(spoof)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True,
            cwd="/",
        )
        try:
            assert holder.stdout is not None
            assert holder.stdout.readline() == "ready\n"
            shutil.rmtree(root)
            with pytest.raises(cli.Refusal, match="uses slot"):
                cli._assert_absent_slot_unused(config, record, (root,))
        finally:
            holder.kill()
            holder.wait(timeout=10)


_MIXED_HOLDER = """
import os, sys, time

target = sys.argv[1]
fd = os.open(target, os.O_RDONLY)
os.unlink(target)
os.mkdir(target)
os.chdir(target)
sys.stdout.write("ready\\n")
sys.stdout.flush()
while True:
    time.sleep(60)
"""


def test_a_linked_fd_with_the_deleted_roots_text_still_refuses(
    tmp_path: Path,
) -> None:
    """One process holds two entries with the deleted root's text: its
    working directory is the deleted root (no links left), but a file
    descriptor names a still-linked file that had that path.  The
    exemption requires every such entry to be unlinked, so this
    refuses."""
    with _record_with_owner(tmp_path) as (config, record, _slot, parent):
        parent.kill()
        parent.wait(timeout=10)
        owner = subprocess.Popen(
            [sys.executable, "-I", "-c", _SLEEPER], cwd="/"
        )
        identity = cli._read_process_identity(owner.pid)
        owner.kill()
        owner.wait(timeout=10)
        record = dataclasses.replace(record, owner=identity)
        target = tmp_path / ".target.fenced.1.probe"
        target.write_text("held\n", encoding="utf-8")
        os.link(target, tmp_path / "held-elsewhere.txt")
        holder = subprocess.Popen(
            [sys.executable, "-I", "-c", _MIXED_HOLDER, str(target)],
            stdout=subprocess.PIPE,
            text=True,
            cwd="/",
        )
        try:
            assert holder.stdout is not None
            assert holder.stdout.readline() == "ready\n"
            target.rmdir()
            with pytest.raises(cli.Refusal, match="uses slot"):
                cli._assert_absent_slot_unused(config, record, (target,))
        finally:
            holder.kill()
            holder.wait(timeout=10)
