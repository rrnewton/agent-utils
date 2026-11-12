"""Differential: wrkslotsd limits-show against Python `limits show`.

#1112/#1116 image glue batch. Only `show` is wired (a reader);
`apply` and `clear` are writers and stay refused by name until the
writers phase. Both editions shell out to `systemctl show`, so the
Python oracle fakes that one subprocess call and the Rust side is
compared on a host where the same call answers or fails identically.
"""

from __future__ import annotations

import argparse
import os
import subprocess

import pytest
from wrkslots import imagecmd

from wrkslots.tests.rust_binary import rust_wrkslotsd


def test_limits_apply_and_clear_stay_refused() -> None:
    for action in ("apply", "clear"):
        result = subprocess.run(
            [str(rust_wrkslotsd()), "limits", action],
            capture_output=True, text=True, check=False, timeout=900,
            env={**os.environ, "WRKSLOTS_PYCOMPAT": "1"},
        )
        assert result.returncode == 1
        assert "'limits' stays with the Python edition in this slice" in result.stderr


def test_limits_show_matches_python(capsys: pytest.CaptureFixture[str]) -> None:
    args = argparse.Namespace(action="show", memory_fraction=0.8, cpu_fraction=0.9)
    expected_rc = imagecmd._cmd_limits(args)
    expected_out = capsys.readouterr().out
    result = subprocess.run(
        [str(rust_wrkslotsd()), "limits", "show"],
        capture_output=True, text=True, check=False, timeout=900,
        env={**os.environ, "WRKSLOTS_PYCOMPAT": "1"},
    )
    assert result.returncode == expected_rc
    assert result.stdout == expected_out
