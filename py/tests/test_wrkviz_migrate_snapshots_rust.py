"""Differential: Rust `wrkviz migrate-snapshots` vs the oracle.

Hand-built twin archives hold teams/<slug>/source_snapshots trees; the
plan (text and JSON), the move, an idempotent re-run, and a refusal case
must agree byte-for-byte, including the store trees. Fixtures only.
"""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

import pytest

AU_ROOT = Path(__file__).resolve().parents[2]
PYTHON_WRKVIZ = AU_ROOT / "py" / "bin" / "wrkviz"
RUST_WRKVIZ = AU_ROOT / "rs" / "target" / "release" / "wrkviz"


@pytest.fixture(scope="module")
def rust_wrkviz() -> Path:
    subprocess.run(
        ["cargo", "build", "--release", "--manifest-path", "rs/Cargo.toml",
         "-p", "wrkviz"],
        cwd=AU_ROOT, check=True, capture_output=True, text=True, timeout=900,
    )
    return RUST_WRKVIZ


def _archive(base: Path) -> Path:
    archive = base / "archive"
    for team, files in (
        ("alpha", {"vendor.log": b"alpha log\n", "sub/db.sqlite": b"\x00\x01"}),
        ("beta", {"notes.txt": b"beta\n"}),
    ):
        root = archive / "teams" / team / "source_snapshots"
        for rel, data in files.items():
            path = root / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
    (archive / "teams" / "alpha" / "summaries").mkdir(parents=True, exist_ok=True)
    (archive / "teams" / "alpha" / "summaries" / "keep.json").write_text("{}\n")
    return archive


def _run(binary: Path, args: list[str]) -> tuple[int, str, str]:
    done = subprocess.run(
        [str(binary), *args], capture_output=True, text=True, timeout=120,
    )
    return done.returncode, done.stdout, done.stderr


def _tree(root: Path) -> dict[str, bytes | str]:
    out: dict[str, bytes | str] = {}
    if not root.exists():
        return out
    for path in sorted(root.rglob("*")):
        rel = str(path.relative_to(root))
        if path.is_symlink():
            out[rel] = f"link->{path.readlink()}"
        elif path.is_dir():
            out[rel] = "<dir>"
        else:
            out[rel] = path.read_bytes()
    return out


def _scrub(text: str, base: Path) -> str:
    return text.replace(str(base), "BASE").replace(str(base.resolve()), "BASE")


_SEQUENCE = [0]


def _pair(tmp_path: Path, rust_wrkviz: Path, args: list[str],
          setup=None) -> tuple[int, str, str, Path, Path]:
    _SEQUENCE[0] += 1
    py_base = tmp_path / f"py{_SEQUENCE[0]}"
    rs_base = tmp_path / f"rs{_SEQUENCE[0]}"
    py_archive = _archive(py_base)
    rs_archive = _archive(rs_base)
    if setup is not None:
        setup(py_base)
        setup(rs_base)
    py = _run(PYTHON_WRKVIZ, [a.format(base=str(py_base)) for a in args])
    rs = _run(rust_wrkviz, [a.format(base=str(rs_base)) for a in args])
    assert rs[0] == py[0], (rs, py)
    assert _scrub(rs[1], rs_base) == _scrub(py[1], py_base)
    assert _scrub(rs[2], rs_base) == _scrub(py[2], py_base)
    return py[0], _scrub(py[1], py_base), _scrub(py[2], py_base), py_base, rs_base


def _assert_trees(py_base: Path, rs_base: Path) -> None:
    assert _tree(py_base) == _tree(rs_base)


def test_plan_move_and_rerun(tmp_path: Path, rust_wrkviz: Path) -> None:
    py_base = tmp_path / "py"
    rs_base = tmp_path / "rs"
    _archive(py_base)
    _archive(rs_base)

    def both(args: list[str]) -> tuple[int, str, str]:
        py = _run(PYTHON_WRKVIZ, [a.format(base=str(py_base)) for a in args])
        rs = _run(rust_wrkviz, [a.format(base=str(rs_base)) for a in args])
        assert rs[0] == py[0], (rs, py)
        assert _scrub(rs[1], rs_base) == _scrub(py[1], py_base)
        assert _scrub(rs[2], rs_base) == _scrub(py[2], py_base)
        return py[0], _scrub(py[1], py_base), _scrub(py[2], py_base)

    code, out, _err = both(
        ["migrate-snapshots", "--output", "{base}/archive"])
    assert code == 0
    assert "dry run: 3 file(s)" in out
    code, out, _err = both(
        ["migrate-snapshots", "--output", "{base}/archive", "--format", "json"])
    assert code == 0
    plan = json.loads(out)["plan"]
    assert plan["already_external"] is False
    assert [team["team"] for team in plan["teams"]] == ["alpha", "beta"]
    _assert_trees(py_base, rs_base)
    code, out, _err = both(
        ["migrate-snapshots", "--output", "{base}/archive", "--move"])
    assert code == 0
    assert "moved 2 team(s), copied 0" in out
    assert (py_base / "archive.sources" / "alpha" / "vendor.log").exists()
    _assert_trees(py_base, rs_base)
    code, out, _err = both(
        ["migrate-snapshots", "--output", "{base}/archive", "--move"])
    assert code == 0
    assert "No team keeps snapshots inside the archive." in out
    _assert_trees(py_base, rs_base)
    pointer = json.loads((py_base / "archive" / "snapshot-root.json").read_text())
    assert pointer["layout"] == "external"


def test_populated_store_refuses(tmp_path: Path, rust_wrkviz: Path) -> None:
    def setup(base: Path) -> None:
        target = base / "archive.sources" / "alpha"
        target.mkdir(parents=True, exist_ok=True)
        (target / "existing.txt").write_text("keep\n")

    code, _out, err, py_base, rs_base = _pair(
        tmp_path, rust_wrkviz,
        ["migrate-snapshots", "--output", "{base}/archive", "--move"],
        setup=setup,
    )
    assert code == 2
    assert "refusing to migrate; nothing was moved" in err
    _assert_trees(py_base, rs_base)


def test_explicit_store_root(tmp_path: Path, rust_wrkviz: Path) -> None:
    code, out, _err, py_base, rs_base = _pair(
        tmp_path, rust_wrkviz,
        ["migrate-snapshots", "--output", "{base}/archive",
         "--snapshot-root", "{base}/store", "--move", "--format", "json"],
    )
    assert code == 0
    result = json.loads(out)
    assert result["moved"] == ["alpha", "beta"]
    assert (py_base / "store" / "beta" / "notes.txt").exists()
    _assert_trees(py_base, rs_base)
