"""Validation-slot teardown closure (B2-B): owner dead, slice empty, one scan.

The slice check is exercised against a synthetic cgroup tree (kernel
membership is what it is; the walk is the code under test), and the
holder checks with real processes: a sibling-scope holder the slice
cannot see must refuse, and an exited validation whose boxes all ran
under the slice must close.
"""

from __future__ import annotations

import dataclasses
import os
import signal
import subprocess
import sys
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path

import pytest

from wrkslots import cli
from wrkslots.tests.test_lifecycle import active_slots, checkout, create, make_project

pytestmark = pytest.mark.skipif(
    not cli._has_identity_user_namespace(),
    reason="the census commands are trusted only in the initial identity user namespace",
)

_HOLDER = """
import os, sys, time

os.chdir(sys.argv[1])
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


def test_slice_subtree_state_reads_the_kernel_populated_answer(
    tmp_path: Path,
) -> None:
    root = tmp_path / "cgroup"
    slice_dir = root / "user.slice" / "validate-slot01.slice"
    slice_dir.mkdir(parents=True)
    (slice_dir / "cgroup.events").write_text(
        "populated 0\nfrozen 0\n", encoding="ascii"
    )
    assert cli._slice_subtree_state("validate-slot01.slice", cgroup_root=root) == "empty"
    (slice_dir / "cgroup.events").write_text(
        "populated 1\nfrozen 0\n", encoding="ascii"
    )
    assert (
        cli._slice_subtree_state("validate-slot01.slice", cgroup_root=root)
        == "nonempty"
    )
    # An absent slice, or one whose events cannot be read, is unknown:
    # never evidence of emptiness (the caller falls back).
    assert cli._slice_subtree_state("validate-absent.slice", cgroup_root=root) == "unknown"
    (slice_dir / "cgroup.events").unlink()
    assert cli._slice_subtree_state("validate-slot01.slice", cgroup_root=root) == "unknown"


@contextmanager
def _validate_record(
    tmp_path: Path,
) -> Iterator[tuple[cli.Config, cli.ActiveRecord, Path, subprocess.Popen[str]]]:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project, slot="slot01", slot_type="validate", branch=None)
    assert made.returncode == 0, made.stderr
    created = next(
        item
        for item in cli._load_active(
            cli._load_config(str(project), "testhost")
        ).slots
        if item.slot == "slot01"
    )
    slot_path = cli._slot_directory(
        cli._load_config(str(project), "testhost"), "slot01", "validate"
    )
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
        record = created
        owned = dataclasses.replace(
            record, owner=cli._read_process_identity(parent.pid)
        )
        yield config, owned, slot_path, parent
    finally:
        parent.kill()
        parent.wait(timeout=10)


@pytest.mark.load_quarantine  # #2065 hot-host-census-tests: teardown reference scan reads the live process table
def test_teardown_closes_when_the_slice_is_empty_and_owner_dead(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    with _validate_record(tmp_path) as (config, record, slot_path, parent):
        assert parent.stdin is not None and parent.stdout is not None
        parent.stdin.write("fork\n")
        parent.stdin.flush()
        child = int(parent.stdout.readline())
        assert parent.wait(timeout=10) == 0
        try:
            # The child already exited with its parent in this shape:
            # kill any survivor so only the slice proof remains.
            try:
                os.kill(child, signal.SIGKILL)
            except ProcessLookupError:
                pass
            monkeypatch.setattr(
                cli, "_slice_subtree_state", lambda _name: "empty"
            )
            cli._assert_validate_teardown_unused(config, record, (slot_path,))
        finally:
            try:
                os.kill(child, signal.SIGKILL)
            except ProcessLookupError:
                pass


@pytest.mark.load_quarantine  # #2065 hot-host-census-tests: privileged census exceeds its budget under host churn
def test_a_sibling_scope_holder_makes_teardown_refuse(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A box launched WITHOUT the slice is invisible to the subtree
    check; the ordinary reference scan is what refuses teardown."""
    with _validate_record(tmp_path) as (config, record, slot_path, parent):
        assert parent.stdin is not None and parent.stdout is not None
        parent.stdin.write("fork\n")
        parent.stdin.flush()
        child = int(parent.stdout.readline())
        assert parent.wait(timeout=10) == 0
        try:
            monkeypatch.setattr(
                cli, "_slice_subtree_state", lambda _name: "empty"
            )
            with pytest.raises(cli.Refusal, match="uses slot "):
                cli._assert_validate_teardown_unused(config, record, (slot_path,))
        finally:
            os.kill(child, signal.SIGKILL)


def test_a_nonempty_slice_makes_teardown_refuse(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    with _validate_record(tmp_path) as (config, record, slot_path, parent):
        parent.kill()
        parent.wait(timeout=10)
        monkeypatch.setattr(
            cli, "_slice_subtree_state", lambda _name: "nonempty"
        )
        with pytest.raises(cli.Refusal, match="still has living members"):
            cli._assert_validate_teardown_unused(config, record, (slot_path,))
