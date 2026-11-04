"""Differential: wrkslotsd reap-stray-worktrees against Python (#775)."""

from __future__ import annotations

import os
import re
import subprocess
from pathlib import Path

import pytest

from wrkslots.tests.rust_binary import rust_wrkslotsd
from wrkslots.tests.test_reap_stray_worktrees import _run, _stray
from wrkslots.tests.test_create_text_rust import _fixture, _git


AU_ROOT = Path(__file__).resolve().parents[3]
_HEX40 = re.compile(r"\b[0-9a-f]{40}\b")


def _rust(root: Path, upstream: Path, *, apply: bool, fmt: str) -> subprocess.CompletedProcess[str]:
    args = [
        str(rust_wrkslotsd()), "reap-stray-worktrees",
        "--repository", str(upstream), "--root", str(root), "--format", fmt,
    ]
    if apply:
        args.append("--apply")
    return subprocess.run(args, cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900, env=dict(os.environ))


def _norm(text: str, root: Path) -> str:
    # The two fixture repositories are committed a moment apart, so their HEAD
    # hashes differ whenever creation crosses a second boundary (#827).
    return _HEX40.sub("HEAD", text.replace(str(root), "ROOT").replace(str(root.parent), "BASE"))


def _assert_output_head(text: str, upstream: Path) -> None:
    # Normalizing every 40-hex token to HEAD would also erase a wrong HEAD,
    # so first pin the identity: the plan must name exactly this fixture's
    # own HEAD and nothing else.
    head = _git(upstream, "rev-parse", "HEAD").strip()
    tokens = set(_HEX40.findall(text))
    assert tokens == {head}, f"plan names {sorted(tokens)}, fixture HEAD is {head}"


def test_reap_stray_rust_plan_matches_python(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust")
    oracle_root = _fixture(tmp_path / "oracle")
    rust_stray = _stray(tmp_path / "rust")
    _stray(tmp_path / "oracle")
    rc, out = _run(oracle_root, apply=False)
    assert rc == 0
    result = _rust(rust_root, tmp_path / "rust" / "upstream-repo", apply=False, fmt="human")
    assert result.returncode == 0, result.stderr
    _assert_output_head(result.stdout, tmp_path / "rust" / "upstream-repo")
    _assert_output_head(out, tmp_path / "oracle" / "upstream-repo")
    assert _norm(result.stdout, rust_root) == _norm(out, oracle_root)
    assert rust_stray.exists()


def test_reap_stray_plan_with_wrong_head_is_caught(tmp_path: Path) -> None:
    # Negative control: a plan naming a different valid HEAD must fail the
    # identity assertion even though normalization alone compares it equal.
    rust_root = _fixture(tmp_path / "rust")
    _stray(tmp_path / "rust")
    upstream = tmp_path / "rust" / "upstream-repo"
    result = _rust(rust_root, upstream, apply=False, fmt="human")
    assert result.returncode == 0, result.stderr
    head = _git(upstream, "rev-parse", "HEAD").strip()
    wrong = "0" * 40 if head != "0" * 40 else "1" * 40
    forged = _HEX40.sub(wrong, result.stdout)
    assert _norm(forged, rust_root) == _norm(result.stdout, rust_root)
    with pytest.raises(AssertionError):
        _assert_output_head(forged, upstream)


def test_reap_stray_rust_apply_reaps(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust")
    stray = _stray(tmp_path / "rust")
    upstream = tmp_path / "rust" / "upstream-repo"
    result = _rust(rust_root, upstream, apply=True, fmt="json")
    assert result.returncode == 0, result.stderr
    assert '"reaped"' in result.stdout
    assert not stray.exists()
