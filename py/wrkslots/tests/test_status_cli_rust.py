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
import sys
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


#: Rewrites the first registered row's task, as a writer landing mid-read would (#1200).
CHANGE_FIRST_TASK = """
import json, sys
from pathlib import Path
shard = Path(sys.argv[1])
document = json.loads(shard.read_text())
document["slots"][0]["task"] = "changed while measuring"
document["revision"] += 1
shard.write_text(json.dumps(document))
"""


def change_mid_read_env(root: Path, tmp_path: Path) -> dict[str, str]:
    """The environment that lands CHANGE_FIRST_TASK after a Rust reader's first measurement."""
    script = tmp_path / "change-first-task.py"
    script.write_text(CHANGE_FIRST_TASK)
    shard = root / "worktrees" / "ACTIVE.node-a.json"
    return {
        **os.environ,
        "WRKSLOTS_TEST_REGISTRY_CHANGE": f"{sys.executable} {script} {shard}",
    }


def test_status_publishes_the_registry_after_a_change_mid_read_like_python(
    tmp_path: Path,
) -> None:
    """#1200 rust-status-consistency: a report never mixes two instants of the registry."""
    root = _root(tmp_path)
    _fixture(root)
    rust_result = subprocess.run(
        [str(rust_wrkslotsd()), "status-text", "--root", str(root), "--format", "json"],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
        env=change_mid_read_env(root, tmp_path),
    )
    assert rust_result.returncode == 0, rust_result.stderr
    rust_document = cast(dict[str, object], json.loads(rust_result.stdout))
    first = cast(list[dict[str, object]], rust_document["active"])[0]
    assert first["task"] == "changed while measuring", first
    # Re-measured from the registry as it is now: the report Python makes of it.
    python_result = _python(root, "--format", "json")
    assert python_result.returncode == 0, python_result.stderr
    python_document = cast(dict[str, object], json.loads(python_result.stdout))
    assert _normalized(rust_document) == _normalized(python_document)
