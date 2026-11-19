"""B2 cold-review fallback wiring: an absent or unreadable slice never
proves emptiness by itself; the ordinary reference census still judges
validation teardown (escaped live members are caught by it)."""

import dataclasses
import functools
import os
import subprocess
import sys
from pathlib import Path

import pytest

from wrkslots import cli
from wrkslots.tests.test_lifecycle import create, make_project

_SLEEPER = """
import time

while True:
    time.sleep(60)
"""

_HOLDER = """
import os
import sys
import time

os.open(sys.argv[1], os.O_RDONLY)
print("ready", flush=True)
while True:
    time.sleep(60)
"""


def _dead_record(project: Path) -> tuple[cli.Config, cli.ActiveRecord]:
    parent = subprocess.Popen([sys.executable, "-I", "-c", _SLEEPER], cwd="/")
    identity = cli._read_process_identity(parent.pid)
    parent.kill()
    parent.wait(timeout=10)
    config = cli._load_config(str(project), "testhost")
    state = cli._load_active(config)
    record = cli._find_record(state, "validate-x")
    cli._write_active_state(
        config,
        cli._replace_record(state, dataclasses.replace(record, owner=identity)),
        action="test-owner-exited",
        slot="validate-x",
    )
    state = cli._load_active(config)
    return config, cli._find_record(state, "validate-x")


def _setup(tmp_path: Path) -> tuple[Path, cli.Config, cli.ActiveRecord, Path]:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project, slot="validate-x", slot_type="validate").returncode == 0
    config, record = _dead_record(project)
    target = cli._slot_directory(config, "validate-x", "validate")
    fenced = target.parent / ".validate-x.fenced.1.test"
    os.rename(target, fenced)
    return project, config, record, fenced


def _spawn_holder(path: Path) -> subprocess.Popen[str]:
    holder = subprocess.Popen(
        [sys.executable, "-I", "-c", _HOLDER, str(path)],
        stdout=subprocess.PIPE,
        text=True,
        cwd="/",
    )
    assert holder.stdout is not None
    assert holder.stdout.readline() == "ready\n"
    return holder


def test_teardown_without_a_slice_falls_back_to_the_census(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """No cgroup root at all (no slice exists): state is 'unknown', the
    closure defers to the census, and an unused fenced slot passes."""
    _project, config, record, fenced = _setup(tmp_path)
    monkeypatch.setattr(
        cli,
        "_slice_subtree_state",
        functools.partial(
            cli._slice_subtree_state, cgroup_root=tmp_path / "no-cgroups"
        ),
    )
    cli._assert_validate_teardown_unused(config, record, (fenced,))


def test_teardown_escaped_member_is_caught_by_the_fallback_census(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A live holder of the fenced tree outside any slice: the census
    refuses even though the slice state is 'unknown'."""
    _project, config, record, fenced = _setup(tmp_path)
    monkeypatch.setattr(
        cli,
        "_slice_subtree_state",
        functools.partial(
            cli._slice_subtree_state, cgroup_root=tmp_path / "no-cgroups"
        ),
    )
    holder = _spawn_holder(fenced)
    try:
        with pytest.raises(cli.Refusal, match="(?i)us"):
            cli._assert_validate_teardown_unused(
                config, record, (fenced,)
            )
    finally:
        holder.kill()
        holder.wait(timeout=10)


def test_teardown_empty_slice_does_not_excuse_a_live_holder(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The recorded slice exists and is empty, but a holder escaped it
    (moved to a sibling scope): the census still refuses."""
    _project, config, record, fenced = _setup(tmp_path)
    proc_root = tmp_path / "cgroupfs"
    slice_dir = proc_root / "user.slice" / "validate-validate-x.slice"
    slice_dir.mkdir(parents=True)
    (slice_dir / "cgroup.events").write_text("populated 0\n", encoding="ascii")
    monkeypatch.setattr(
        cli,
        "_slice_subtree_state",
        functools.partial(cli._slice_subtree_state, cgroup_root=proc_root),
    )
    holder = _spawn_holder(fenced)
    try:
        with pytest.raises(cli.Refusal, match="(?i)us"):
            cli._assert_validate_teardown_unused(
                config, record, (fenced,)
            )
    finally:
        holder.kill()
        holder.wait(timeout=10)
