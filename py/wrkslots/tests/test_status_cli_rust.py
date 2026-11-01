"""Differential: wrkslotsd status-text against the Python status command.

#562 rust-primary-wrkslots soak slice: the Python `status` envelope
(schema/project/worktrees/control/layout/journals/registry_storage)
and its human form, at parity, so the soak launcher can run status
in Rust in both formats. Storage inconsistencies come from the real
filesystem/Git censuses on this fixture (missing slot directories).
"""

from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path
from typing import cast

from wrkslots.tests.test_status_state_rust import _fixture, _root

from wrkslots.tests.rust_binary import rust_wrkslotsd


AU_ROOT = Path(__file__).resolve().parents[3]


def _python(root: Path, *args: str) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    env["PYTHONPATH"] = str(AU_ROOT / "py")
    return subprocess.run(
        ["python3", "-m", "wrkslots", "--project-root", str(root), "status", *args],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900, env=env,
    )


def _rust(root: Path, *args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(rust_wrkslotsd()), "status-text", "--root", str(root), *args],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
    )


def _normalized(document: dict[str, object]) -> dict[str, object]:
    active = cast(list[dict[str, object]], document["active"])
    for record in active:
        age = cast(int, record.pop("heartbeat_age_seconds"))
        assert age >= 0
    return document


def test_status_json_envelope_matches_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _fixture(root)
    python_result = _python(root, "--format", "json")
    rust_result = _rust(root, "--format", "json")
    assert python_result.returncode == 0, python_result.stderr
    assert rust_result.returncode == 0, rust_result.stderr
    python_document = cast(dict[str, object], json.loads(python_result.stdout))
    rust_document = cast(dict[str, object], json.loads(rust_result.stdout))
    assert _normalized(rust_document) == _normalized(python_document)
    # The fixture's slot directories do not exist: both editions must
    # report the storage drift, not an empty list.
    assert python_document["registry_storage_state"] == "inconsistent"
    assert rust_document["registry_storage_inconsistencies"]


def test_status_human_form_matches_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _fixture(root)
    python_result = _python(root)
    rust_result = _rust(root)
    assert python_result.returncode == 0, python_result.stderr
    assert rust_result.returncode == 0, rust_result.stderr
    # Heartbeat ages move with the clock; compare everything else.
    def scrub(text: str) -> list[str]:
        lines = []
        for line in text.splitlines():
            if "heartbeat_age=" in line:
                head, _, rest = line.partition("heartbeat_age=")
                _, _, tail = rest.partition("s ")
                line = f"{head}heartbeat_age=<age>s {tail}"
            lines.append(line)
        return lines

    assert scrub(rust_result.stdout) == scrub(python_result.stdout)
