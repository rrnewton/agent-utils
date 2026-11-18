"""Hermetic fallback-scan table (#1778).

When a seal records no operator (written before the extension slot
existed), both editions fall back to scanning the process table for a
wrkslots removal acting on the sealed targets. This table runs over
a fake process table -- never the host's real processes -- and the
Rust unit test of the same name builds the identical table, so any
rule difference between the editions fails one side loudly.
"""

from __future__ import annotations

import os
from pathlib import Path
from collections.abc import Sequence

from wrkslots import cli as wrkslots


def _table(tmp_path: Path) -> tuple[Path, Path]:
    root = tmp_path / "project"
    other = tmp_path / "other"
    proc = tmp_path / "proc"
    for directory in (root, other, proc, root / "sub", other / "sub"):
        directory.mkdir(parents=True, exist_ok=True)

    def process(pid: int, argv: Sequence[str], cwd: Path | None) -> None:
        entry = proc / str(pid)
        entry.mkdir(parents=True)
        (entry / "cmdline").write_bytes(
            b"\0".join(part.encode() for part in argv) + b"\0"
        )
        if cwd is not None:
            (entry / "cwd").symlink_to(cwd)

    process(4101, ["python3", "-m", "wrkslots", "--project-root", str(root), "remove", "target"], tmp_path)
    process(4102, ["python3", "-m", "wrkslots", "--project-root", str(other), "remove", "target"], tmp_path)
    process(4103, ["python3", "-m", "wrkslots", "remove", "target"], root / "sub")
    process(4104, ["python3", "-m", "wrkslots", "remove", "target"], other / "sub")
    process(4105, ["python3", "-m", "wrkslots", "remove", "target"], None)
    unreadable = proc / "4106"
    unreadable.mkdir(parents=True)
    (unreadable / "cmdline").mkdir()
    process(4107, ["python3", "-m", "wrkslots", "--project-root", str(root), "recover"], root)
    process(4108, ["/usr/bin/wrkslots", "remove", "target"], root)
    process(4109, ["python3", "-m", "wrkslots", "--project-root", str(root), "remove", "other"], tmp_path)
    process(4110, ["python3", "-m", "wrkslots", f"--project-root={root}", "remove-validate-batch", "--slot", "target=1"], tmp_path)
    process(4111, ["python3", "-m", "wrkslots.tests.idle_validation_host", "--project-root", str(root), "remove", "target"], root)
    process(4112, ["python3", "-m", "wrkslots", "--project-root", str(root), "remove", "target"], other)
    # A wrapper argv embedding a whole invocation as data: the FIRST
    # --project-root judges the process (#1778).
    process(4113, ["python3", "-m", "wrkslots", "--project-root", str(other), "remove", "target", "--project-root", str(root)], tmp_path)
    process(4114, ["python3", "-m", "wrkslots", "--project-root", str(root), "remove", "target", "--project-root", str(other)], tmp_path)
    return root, proc


def test_fallback_scan_table(tmp_path: Path) -> None:
    root, proc = _table(tmp_path)

    with_targets = wrkslots._other_live_wrkslots_processes(
        root, proc, targets=("target",)
    )
    assert sorted(with_targets) == [4101, 4103, 4105, 4106, 4108, 4110, 4112, 4114]

    without_targets = wrkslots._other_live_wrkslots_processes(root, proc)
    assert sorted(without_targets) == [
        4101, 4103, 4105, 4106, 4108, 4109, 4110, 4112, 4114,
    ]
