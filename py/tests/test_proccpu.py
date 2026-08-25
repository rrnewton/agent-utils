"""Synthetic procfs brackets for the best-effort process-group CPU reader."""

import os
from pathlib import Path

import pytest

from dagrun.proccpu import subtree_cpu_seconds


def _write_stat(root: Path, pid: int, comm: str, pgrp: int, cpu: tuple[int, int, int, int]) -> None:
    directory = root / str(pid)
    directory.mkdir(parents=True)
    fields = [str(pid), f"({comm})", "R", "1", str(pgrp), *("0" for _ in range(8)), *(str(v) for v in cpu)]
    (directory / "stat").write_text(" ".join(fields), encoding="utf-8")


def test_sums_only_the_named_group_and_includes_reaped_children(tmp_path: Path) -> None:
    _write_stat(tmp_path, 100, "leader", 100, (10, 5, 20, 5))
    _write_stat(tmp_path, 101, "child (x)", 100, (30, 0, 0, 0))
    _write_stat(tmp_path, 200, "stranger", 200, (9999, 9999, 9999, 9999))
    got = subtree_cpu_seconds(100, proc_root=tmp_path)
    assert got == pytest.approx(70 / os.sysconf("SC_CLK_TCK"))


def test_zero_is_a_reading_but_absence_is_unknown(tmp_path: Path) -> None:
    _write_stat(tmp_path, 100, "leader", 100, (0, 0, 0, 0))
    assert subtree_cpu_seconds(100, proc_root=tmp_path) == 0.0
    assert subtree_cpu_seconds(999, proc_root=tmp_path) is None


def test_unreadable_or_malformed_procfs_is_unknown(tmp_path: Path) -> None:
    assert subtree_cpu_seconds(100, proc_root=tmp_path / "missing") is None
    bad = tmp_path / "100"
    bad.mkdir()
    (bad / "stat").write_text("malformed", encoding="utf-8")
    assert subtree_cpu_seconds(100, proc_root=tmp_path) is None


def test_refuses_degenerate_process_groups(tmp_path: Path) -> None:
    assert subtree_cpu_seconds(0, proc_root=tmp_path) is None
    assert subtree_cpu_seconds(1, proc_root=tmp_path) is None
