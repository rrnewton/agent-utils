"""Differential: Rust `wrkviz ingest-claude-history` against the Python oracle.

Both editions ingest the same history fixture into twin
archives; stdout, stderr, exit code, and the resulting file
trees (archive, build store, and snapshot store) must agree.
Run records and manifests compare structurally with timestamps,
run ids, and the snapshot copies' mtimes normalized. The
CPython-vs-serde JSON syntax wording and the CPython-vs-Rust
regex complaint wording are normalized (the cross-slice named
gaps). No live registry or real history file is touched: every
input is a temp fixture.
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path

import pytest

AU_ROOT = Path(__file__).resolve().parents[2]
PYTHON_WRKVIZ = AU_ROOT / "py" / "bin" / "wrkviz"
RUST_WRKVIZ = AU_ROOT / "rs" / "target" / "release" / "wrkviz"
BASE = 1_760_000_000_000


@pytest.fixture(scope="module")
def rust_wrkviz() -> Path:
    subprocess.run(
        ["cargo", "build", "--release", "--manifest-path", "rs/Cargo.toml",
         "-p", "wrkviz"],
        cwd=AU_ROOT, check=True, capture_output=True, text=True, timeout=900,
    )
    return RUST_WRKVIZ


def _history(path: Path, rows: list[dict[str, object]], tail: str = "") -> Path:
    text = "".join(json.dumps(row) + "\n" for row in rows) + tail
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")
    return path


def _basic_rows() -> list[dict[str, object]]:
    return [
        {"display": "Fix the widget parser", "timestamp": BASE,
         "project": "/work/widget", "sessionId": "sess-a"},
        {"display": "Add tests please", "timestamp": BASE + 60_000,
         "project": "/work/widget", "sessionId": "sess-a",
         "pastedContents": {
             "1": {"type": "text", "content": "pasted log line  "},
             "2": {"type": "image", "content": "x"}}},
        {"display": "Other project prompt", "timestamp": BASE + 120_000,
         "project": "/other/place", "sessionId": "sess-b"},
        {"display": "Covered prompt", "timestamp": BASE + 180_000,
         "project": "/work/widget", "sessionId": "sess-covered"},
        {"display": "No session prompt", "timestamp": BASE + 240_000,
         "project": "/work/widget"},
    ]


def _run(command: list[str], args: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [*command, "ingest-claude-history", *args],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=120,
    )


def _normalize(text: str, root: Path) -> str:
    text = text.replace(str(root), "ROOT")
    text = re.sub(r"runs/[0-9A-Za-z-]+\.json", "runs/RUN.json", text)
    text = re.sub(r"invalid JSON: .*", "invalid JSON: <syntax>", text)
    text = re.sub(
        r"invalid project pattern .*?(?=\nwrkviz: |\Z)",
        "invalid project pattern <re>", text, flags=re.S)
    return text


def _scrub(value: object, root: Path) -> object:
    if isinstance(value, dict):
        out: dict[str, object] = {}
        for key, item in value.items():
            if key in ("run_id", "started_at", "completed_at", "created_at",
                       "last_run_at", "last_run_id", "updated_at", "mtime_ns"):
                out[key] = "<ts>"
            elif key == "error" and isinstance(item, str) and (
                    "invalid project pattern" in item or "invalid JSON" in item):
                out[key] = "<error>"
            elif key == "command" and isinstance(item, list):
                out[key] = [
                    ("ROOT" if str(arg) == str(root) or str(arg).startswith(str(root))
                     else str(arg).replace("store/py", "store/ED")
                     .replace("store/rs", "store/ED"))
                    for arg in item
                ]
            else:
                out[key] = _scrub(item, root)
        return out
    if isinstance(value, list):
        return [_scrub(item, root) for item in value]
    if isinstance(value, str):
        return (value.replace(str(root), "ROOT")
                .replace("store/py", "store/ED")
                .replace("store/rs", "store/ED"))
    return value


def _tree(root: Path) -> dict[str, object]:
    result: dict[str, object] = {}
    for path in sorted(root.rglob("*")):
        if not path.is_file():
            continue
        rel = path.relative_to(root).as_posix()
        if rel.startswith("runs/"):
            rel = "runs/RUN.json"
        if path.suffix == ".json":
            result[rel] = _scrub(json.loads(path.read_text()), root)
        else:
            result[rel] = path.read_bytes()
    return result


def _archives(root: Path) -> dict[str, object]:
    combined: dict[str, object] = {}
    for name in ("archive", "archive.build", "archive.sources"):
        tree = _tree(root / name)
        for rel, value in tree.items():
            combined[f"{name}/{rel}"] = value
    return combined


def _pair(tmp_path: Path, rust_wrkviz: Path, args: list[str], *,
          rows: list[dict[str, object]] | None = None,
          tail: str = '{"display":"partial',
          second: bool = False,
          mutate: object = None) -> tuple[subprocess.CompletedProcess[str],
                                           subprocess.CompletedProcess[str]]:
    rows = _basic_rows() if rows is None else rows
    history = _history(tmp_path / "history.jsonl", rows, tail)
    py_root = tmp_path / "py"
    rs_root = tmp_path / "rs"
    py_args = ["--output", str(py_root / "archive"), "--team", "hist-team",
               "--history-file", str(history), *args]
    rs_args = ["--output", str(rs_root / "archive"), "--team", "hist-team",
               "--history-file", str(history), *args]
    py_cmd = [sys.executable, str(PYTHON_WRKVIZ)]
    rs_cmd = [str(rust_wrkviz)]
    oracle = _run(py_cmd, py_args)
    result = _run(rs_cmd, rs_args)
    if mutate is not None:
        mutate(history)  # type: ignore[operator]
        oracle = _run(py_cmd, py_args)
        result = _run(rs_cmd, rs_args)
    if second:
        oracle = _run(py_cmd, py_args)
        result = _run(rs_cmd, rs_args)
    assert result.returncode == oracle.returncode, (args, result.stderr, oracle.stderr)
    assert _normalize(result.stdout, rs_root) == _normalize(oracle.stdout, py_root), args
    assert _normalize(result.stderr, rs_root) == _normalize(oracle.stderr, py_root), args
    assert _archives(rs_root) == _archives(py_root), args
    return oracle, result


def test_history_fresh_and_second_run(tmp_path: Path, rust_wrkviz: Path) -> None:
    _pair(tmp_path / "fresh", rust_wrkviz,
          ["--project-pattern", "/work/widget", "--covered-session", "sess-covered"])
    _pair(tmp_path / "second", rust_wrkviz,
          ["--project-pattern", "/work/widget", "--covered-session", "sess-covered"],
          second=True)


def test_history_identity_and_window(tmp_path: Path, rust_wrkviz: Path) -> None:
    _pair(tmp_path, rust_wrkviz, [
        "--project-pattern", "/work/widget",
        "--project", "Widget=https://github.com/example/widget.git",
        "--source-host", "Host01.Example",
        "--timezone", "UTC",
        "--start-date", "2025-09-01",
        "--end-date", "2025-10-01",
    ])


def test_history_snapshot_root_and_pattern_forms(
        tmp_path: Path, rust_wrkviz: Path) -> None:
    store = tmp_path / "store"
    rows = _basic_rows()
    history = _history(tmp_path / "history.jsonl", rows)
    results = []
    for label, root in (("py", tmp_path / "py"), ("rs", tmp_path / "rs")):
        cmd = ([sys.executable, str(PYTHON_WRKVIZ)] if label == "py"
               else [str(rust_wrkviz)])
        results.append(_run(cmd, [
            "--output", str(root / "archive"), "--team", "hist-team",
            "--history-file", str(history),
            "--project-pattern", "/work/(widget|gadget)$",
            "--snapshot-root", str(store / label),
        ]))
    assert results[0].returncode == results[1].returncode == 0
    assert _tree(tmp_path / "rs" / "archive") == _tree(tmp_path / "py" / "archive")


def test_history_append_and_rewrite_refusals(
        tmp_path: Path, rust_wrkviz: Path) -> None:
    def truncate(history: Path) -> None:
        lines = history.read_text().splitlines(keepends=True)
        history.write_text("".join(lines[:2]))

    oracle, result = _pair(
        tmp_path, rust_wrkviz, ["--project-pattern", "/work/widget"],
        mutate=truncate)
    assert oracle.returncode == result.returncode == 2
    assert "truncated or rewritten" in result.stderr


def test_history_append_growth(tmp_path: Path, rust_wrkviz: Path) -> None:
    def append(history: Path) -> None:
        with history.open("a", encoding="utf-8") as handle:
            handle.write(json.dumps({
                "display": "Appended prompt", "timestamp": BASE + 300_000,
                "project": "/work/widget", "sessionId": "sess-a"}) + "\n")

    oracle, result = _pair(
        tmp_path, rust_wrkviz, ["--project-pattern", "/work/widget"],
        rows=_basic_rows()[:1], tail="", mutate=append)
    assert oracle.returncode == result.returncode == 0
    assert "2 messages/events" in result.stdout


def test_history_pattern_change_refused(tmp_path: Path, rust_wrkviz: Path) -> None:
    rows = _basic_rows()
    history = _history(tmp_path / "history.jsonl", rows)
    for label, root in (("py", tmp_path / "py"), ("rs", tmp_path / "rs")):
        cmd = ([sys.executable, str(PYTHON_WRKVIZ)] if label == "py"
               else [str(rust_wrkviz)])
        first = _run(cmd, [
            "--output", str(root / "archive"), "--team", "hist-team",
            "--history-file", str(history), "--project-pattern", "/work/widget"])
        assert first.returncode == 0
        second = _run(cmd, [
            "--output", str(root / "archive"), "--team", "hist-team",
            "--history-file", str(history), "--project-pattern", "/other"])
        assert second.returncode == 2, (label, second.stderr)
        assert "was ingested with project pattern" in second.stderr


def test_history_selection_errors(tmp_path: Path, rust_wrkviz: Path) -> None:
    oracle, result = _pair(tmp_path / "nomatch", rust_wrkviz,
                           ["--project-pattern", "/nowhere"])
    assert oracle.returncode == result.returncode == 2
    _pair(tmp_path / "empty-pattern", rust_wrkviz, ["--project-pattern", ""])
    _pair(tmp_path / "bad-regex", rust_wrkviz, ["--project-pattern", "([bad"])


def test_history_malformed_lines(tmp_path: Path, rust_wrkviz: Path) -> None:
    bad_json = _basic_rows()
    path = tmp_path / "badjson"
    path.mkdir()
    history = path / "history.jsonl"
    history.write_text(
        json.dumps(bad_json[0]) + "\n{not json\n", encoding="utf-8")
    for label, root in (("py", path / "py"), ("rs", path / "rs")):
        cmd = ([sys.executable, str(PYTHON_WRKVIZ)] if label == "py"
               else [str(rust_wrkviz)])
        done = _run(cmd, [
            "--output", str(root / "archive"), "--team", "hist-team",
            "--history-file", str(history), "--project-pattern", "/work/widget"])
        assert done.returncode == 2, (label, done.stderr)
        assert "invalid JSON" in _normalize(done.stderr, root)

    unsafe = [{
        "display": "x", "timestamp": BASE, "project": "/work/widget",
        "sessionId": "../escape",
    }]
    oracle, result = _pair(tmp_path / "unsafe", rust_wrkviz,
                           ["--project-pattern", "/work/widget"], rows=unsafe)
    assert oracle.returncode == result.returncode == 2
    assert "unsafe session id" in result.stderr


def test_history_url_prompts_make_artifacts(
        tmp_path: Path, rust_wrkviz: Path) -> None:
    rows = [
        {"display": "Please review https://github.com/example/widget/pull/12 today",
         "timestamp": BASE, "project": "/work/widget", "sessionId": "sess-a"},
        {"display": "Also example/widget#34 and PR #5 once it lands",
         "timestamp": BASE + 1000, "project": "/work/widget",
         "sessionId": "sess-a"},
    ]
    _pair(tmp_path, rust_wrkviz, ["--project-pattern", "/work/widget"], rows=rows)


def test_history_legacy_in_archive_layout(
        tmp_path: Path, rust_wrkviz: Path) -> None:
    rows = _basic_rows()
    history = _history(tmp_path / "history.jsonl", rows)
    for label, root in (("py", tmp_path / "py"), ("rs", tmp_path / "rs")):
        legacy = root / "archive" / "teams" / "hist-team" / "source_snapshots"
        legacy.mkdir(parents=True)
        (root / "archive" / ".wrkviz.json").write_text(
            '{"schema_version": 1, "tool": "wrkviz"}\n')
        (legacy / "keep.txt").write_text("old\n")
        cmd = ([sys.executable, str(PYTHON_WRKVIZ)] if label == "py"
               else [str(rust_wrkviz)])
        done = _run(cmd, [
            "--output", str(root / "archive"), "--team", "hist-team",
            "--history-file", str(history), "--project-pattern", "/work/widget"])
        assert done.returncode == 0, (label, done.stderr)
        assert "keeps its vendor source snapshots inside itself" in done.stderr
    assert _archives(tmp_path / "rs") == _archives(tmp_path / "py")


def test_history_damaged_payload_shard_refused(
        tmp_path: Path, rust_wrkviz: Path) -> None:
    rows = _basic_rows()[:1]
    history = _history(tmp_path / "history.jsonl", rows, tail="")
    for label, root in (("py", tmp_path / "py"), ("rs", tmp_path / "rs")):
        cmd = ([sys.executable, str(PYTHON_WRKVIZ)] if label == "py"
               else [str(rust_wrkviz)])
        args = ["--output", str(root / "archive"), "--team", "hist-team",
                "--history-file", str(history),
                "--project-pattern", "/work/widget"]
        assert _run(cmd, args).returncode == 0
        shard = root / "archive.build" / "hist-team" / "payloads" / "ab.jsonl"
        shard.parent.mkdir(parents=True, exist_ok=True)
        shard.write_text('{"sha256":"abc', encoding="utf-8")
        done = _run(cmd, args)
        assert done.returncode == 2, (label, done.stderr)
        assert "payload record does not have the canonical shape" in done.stderr


def test_history_backslash_and_timezone_paths(
        tmp_path: Path, rust_wrkviz: Path) -> None:
    rows = [{
        "display": r"see https://example.com/a\b and /work/widget#7 notes",
        "timestamp": BASE, "project": "/work/widget", "sessionId": "sess-a",
    }]
    _pair(tmp_path / "backslash", rust_wrkviz,
          ["--project-pattern", "/work/widget"], rows=rows)
    oracle, result = _pair(tmp_path / "tzpath", rust_wrkviz, [
        "--project-pattern", "/work/widget", "--timezone", "../UTC"])
    assert oracle.returncode == result.returncode == 2
    assert "TZPATH" in result.stderr


def test_history_missing_file_and_archive_refusal(
        tmp_path: Path, rust_wrkviz: Path) -> None:
    for label, root in (("py", tmp_path / "py"), ("rs", tmp_path / "rs")):
        cmd = ([sys.executable, str(PYTHON_WRKVIZ)] if label == "py"
               else [str(rust_wrkviz)])
        done = _run(cmd, [
            "--output", str(root / "archive"), "--team", "hist-team",
            "--history-file", str(tmp_path / "absent.jsonl"),
            "--project-pattern", "/work/widget"])
        assert done.returncode == 2, (label, done.stderr)
