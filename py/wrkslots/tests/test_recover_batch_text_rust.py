"""Differential: wrkslotsd recover-batch human forms against Python.

#562 rust-primary-wrkslots: the soak recover-batch human forms. The
Python oracles are `_cmd_recover_absent_agent_rows` and
`_cmd_recover_absent_validate_rows` on a copy of the fixture
registry; the Rust `*-text` writers run on the original. Stdout must
match byte for byte apart from fixture roots, digests, and the
wall-clock `seconds=` fields. Temporary fixture registries only.
"""

from __future__ import annotations

import argparse
import contextlib
import io
import json
import os
import re
import subprocess
from pathlib import Path

import pytest
from wrkslots import cli as wrkslots_cli

from wrkslots.tests.rust_binary import rust_wrkslotsd
from wrkslots.tests.test_recover_absent_write_rust import (
    EMPTY_SHA,
    MACHINE,
    _fixture,
    _mark_recovered,
    _record_sha,
)


AU_ROOT = Path(__file__).resolve().parents[3]
SECONDS = re.compile(r"seconds=[0-9.]+")
SECONDS_JSON = re.compile(r"\"seconds\": ?[0-9.]+")
HEX64 = re.compile(r"\b[0-9a-f]{64}\b")


def _norm(text: str, root: Path) -> str:
    text = text.replace(str(root), "ROOT").replace(str(root.parent), "BASE")
    text = SECONDS.sub("seconds=S", HEX64.sub("SHA", text))
    return SECONDS_JSON.sub('"seconds": 0', text)


def _python_agent_rows(root: Path, sha: str, *, fmt: str) -> tuple[int, str]:
    args = argparse.Namespace(
        project_root=str(root), machine=None,
        rows=[f"slot-a=1={sha}", f"slot-b=1={EMPTY_SHA}"], input=None,
        apply=False, coordinator_authorized=False, coordinator_pid=None,
        format=fmt, wait_lock=0.0,
    )
    buffer = io.StringIO()
    with contextlib.redirect_stdout(buffer):
        rc = wrkslots_cli._cmd_recover_absent_agent_rows(args)
    return rc, buffer.getvalue()


def _rust_agent_rows_text(
    root: Path, sha: str, *, fmt: str
) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    env["WRKSLOTSD_FIXTURE_WRITER"] = "1"
    return subprocess.run(
        [
            str(rust_wrkslotsd()), "recover-absent-agent-rows-text",
            "--row", f"slot-a=1={sha}", "--row", f"slot-b=1={EMPTY_SHA}",
            "--root", str(root), "--format", fmt,
        ],
        cwd=AU_ROOT, capture_output=True, text=True, check=False,
        timeout=900, env=env,
    )


def _pair(tmp_path: Path) -> tuple[Path, str, Path, str]:
    rust_root, record = _fixture(tmp_path / "rust")
    oracle_root, oracle_record = _fixture(tmp_path / "oracle")
    rust_sha = _record_sha(rust_root, record)
    oracle_sha = _record_sha(oracle_root, oracle_record)
    _mark_recovered(rust_root, record, rust_sha)
    _mark_recovered(oracle_root, oracle_record, oracle_sha)
    return rust_root, rust_sha, oracle_root, oracle_sha


def test_agent_rows_text_human_matches_python(tmp_path: Path) -> None:
    rust_root, rust_sha, oracle_root, oracle_sha = _pair(tmp_path)
    rc, out = _python_agent_rows(oracle_root, oracle_sha, fmt="human")
    assert rc == 1
    result = _rust_agent_rows_text(rust_root, rust_sha, fmt="human")
    assert result.returncode == 1, result.stderr
    assert _norm(result.stdout, rust_root) == _norm(out, oracle_root)


def test_agent_rows_text_json_matches_python(tmp_path: Path) -> None:
    rust_root, rust_sha, oracle_root, oracle_sha = _pair(tmp_path)
    rc, out = _python_agent_rows(oracle_root, oracle_sha, fmt="json")
    assert rc == 1
    result = _rust_agent_rows_text(rust_root, rust_sha, fmt="json")
    assert result.returncode == 1, result.stderr
    assert _norm(result.stdout, rust_root) == _norm(out, oracle_root)


def _validate_input(root: Path, sha: str) -> Path:
    path = root / "rows.json"
    path.write_text(json.dumps({
        "schema": 1,
        "rows": [{
            "machine": MACHINE, "slot": "slot-a",
            "generation": 1, "record_sha256": sha,
        }],
    }))
    return path


def _python_validate_rows(root: Path, sha: str, *, fmt: str) -> str:
    args = argparse.Namespace(
        project_root=str(root), machine=None,
        input=str(_validate_input(root, sha)),
        apply=False, coordinator_authorized=False, coordinator_pid=None,
        format=fmt, wait_lock=0.0,
    )
    buffer = io.StringIO()
    with contextlib.redirect_stdout(buffer), pytest.raises(Exception):
        wrkslots_cli._cmd_recover_absent_validate_rows(args)
    return buffer.getvalue()


def _rust_validate_rows_text(
    root: Path, sha: str, *, fmt: str
) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    env["WRKSLOTSD_FIXTURE_WRITER"] = "1"
    return subprocess.run(
        [
            str(rust_wrkslotsd()), "recover-absent-validate-rows-text",
            "--input", str(_validate_input(root, sha)),
            "--root", str(root), "--format", fmt,
        ],
        cwd=AU_ROOT, capture_output=True, text=True, check=False,
        timeout=900, env=env,
    )


def test_validate_rows_text_human_matches_python(tmp_path: Path) -> None:
    rust_root, rust_sha, oracle_root, oracle_sha = _pair(tmp_path)
    out = _python_validate_rows(oracle_root, oracle_sha, fmt="human")
    result = _rust_validate_rows_text(rust_root, rust_sha, fmt="human")
    assert result.returncode == 1, result.stderr
    assert _norm(result.stdout, rust_root) == _norm(out, oracle_root)


def test_validate_rows_text_json_matches_python(tmp_path: Path) -> None:
    rust_root, rust_sha, oracle_root, oracle_sha = _pair(tmp_path)
    out = _python_validate_rows(oracle_root, oracle_sha, fmt="json")
    result = _rust_validate_rows_text(rust_root, rust_sha, fmt="json")
    assert result.returncode == 1, result.stderr
    assert _norm(result.stdout, rust_root) == _norm(out, oracle_root)
