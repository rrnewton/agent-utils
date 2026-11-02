"""Differential: wrkslotsd shell-command against Python.

#562 rust-primary-wrkslots soak slice: the Python `shell-command`
printed line (text form) and its {command, slot_path, isolation}
JSON envelope, plus the box form and the refusal cases, at parity,
so the soak launcher can run the command in Rust. The printed line
names the Python `run`/`box` entry in both editions: `run` and `box`
are not ported yet, so the boxed step stays Python until they are.

Both editions probe the systemd user D-Bus on the test host; when it
is unavailable both refuse with the same text, and the comparison
follows the Python edition either way.
"""

from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path
from typing import cast

from wrkslots.tests.test_doctor_state_rust import _fixture, _root

from wrkslots.tests.rust_binary import rust_wrkslotsd


AU_ROOT = Path(__file__).resolve().parents[3]


def _upstream(tmp_path: Path) -> None:
    # The fixture records name ../upstream-repo as their source
    # repository; the Python loader resolves it, so give it a real one.
    upstream = tmp_path / "upstream-repo"
    upstream.mkdir(exist_ok=True)
    subprocess.run(["git", "init", "-q", str(upstream)], check=True, timeout=60)
    subprocess.run(
        ["git", "-C", str(upstream), "commit", "-q", "--allow-empty", "-m", "seed"],
        check=True, timeout=60,
        env={**os.environ, "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@t",
             "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@t"},
    )


def _python(root: Path, *args: str) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    env["PYTHONPATH"] = str(AU_ROOT / "py")
    return subprocess.run(
        ["python3", "-m", "wrkslots", "--project-root", str(root), "shell-command", *args],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=300, env=env,
    )


def _rust(root: Path, *args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(rust_wrkslotsd()), "shell-command", "--root", str(root), *args],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=300,
    )


def _both(root: Path, *args: str) -> tuple[subprocess.CompletedProcess[str], subprocess.CompletedProcess[str]]:
    python_result = _python(root, *args)
    rust_result = _rust(root, *args)
    if python_result.returncode != 0:
        # A refusal (most likely the host has no user D-Bus): the Rust
        # edition must refuse too, with the same core text. The exact
        # refusal framing is the wrkslotsd-wide convention and differs.
        assert rust_result.returncode != 0, rust_result.stdout
        core = python_result.stderr.splitlines()[0].removeprefix("REFUSED: ") if python_result.stderr else ""
        if core:
            assert core.split(" (")[0] in rust_result.stderr, rust_result.stderr
    return python_result, rust_result


def test_shell_command_json_and_text_match_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _upstream(tmp_path)
    _fixture(root)
    python_result, rust_result = _both(root, "slot-a", "--format", "json")
    if python_result.returncode != 0:
        return
    assert rust_result.returncode == 0, rust_result.stderr
    python_document = cast(dict[str, object], json.loads(python_result.stdout))
    rust_document = cast(dict[str, object], json.loads(rust_result.stdout))
    assert rust_document == python_document
    assert str(python_document["command"]).endswith(" -i")
    python_text, rust_text = _both(root, "slot-a")
    assert rust_text.returncode == 0, rust_text.stderr
    assert rust_text.stdout == python_text.stdout


def test_shell_command_with_command_matches_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _upstream(tmp_path)
    _fixture(root)
    args = ("slot-a", "--isolation", "root", "--format", "json", "--", "/bin/echo", "a b")
    python_result, rust_result = _both(root, *args)
    if python_result.returncode != 0:
        return
    assert rust_result.returncode == 0, rust_result.stderr
    python_document = cast(dict[str, object], json.loads(python_result.stdout))
    rust_document = cast(dict[str, object], json.loads(rust_result.stdout))
    assert rust_document == python_document
    assert python_document["isolation"] == "root"
    assert str(python_document["command"]).endswith("--isolation root -- /bin/echo 'a b'")


def test_shell_command_box_matches_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _upstream(tmp_path)
    _fixture(root)
    args = ("--box", "--name", "lead", "--writable", "worktrees", "--format", "json", "--", "/bin/echo", "x")
    python_result, rust_result = _both(root, *args)
    if python_result.returncode != 0:
        return
    assert rust_result.returncode == 0, rust_result.stderr
    python_document = cast(dict[str, object], json.loads(python_result.stdout))
    rust_document = cast(dict[str, object], json.loads(rust_result.stdout))
    assert rust_document == python_document
    assert python_document["box"] == "lead"
    assert python_document["slot_path"] == str(root)
    assert f" box --name lead --cwd {root} --writable worktrees -- /bin/echo x" in str(python_document["command"])


def test_shell_command_missing_repository_refuses_in_both(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _fixture(root)
    python_result = _python(root, "slot-a", "--format", "json")
    rust_result = _rust(root, "slot-a", "--format", "json")
    assert python_result.returncode != 0
    assert rust_result.returncode != 0
    core = python_result.stderr.splitlines()[0].removeprefix("REFUSED: ")
    assert core in rust_result.stderr, rust_result.stderr


def test_shell_command_refusals_match_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _upstream(tmp_path)
    _fixture(root)
    for args in (
        ("no-such-slot", "--format", "json"),
        ("--box", "slot-a", "--format", "json"),
        ("--format", "json"),
    ):
        python_result = _python(root, *args)
        rust_result = _rust(root, *args)
        assert python_result.returncode != 0, args
        assert rust_result.returncode != 0, args
        core = python_result.stderr.splitlines()[0].removeprefix("REFUSED: ")
        assert core.split(" (")[0] in rust_result.stderr, (args, rust_result.stderr)
