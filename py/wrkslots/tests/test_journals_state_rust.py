"""Differential: wrkslotsd parse-journals against the Python listing.

#562 rust-primary-wrkslots slice 21. The Python oracle is
`_outstanding_journals` (file names) on fixture projects: control-dir
journal files plus operations still pending in each machine's event
history. Event chains are built with the Python digest helper, so a
corrupt chain refuses identically on both sides.
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

import pytest
from wrkslots import cli as wrkslots_cli


AU_ROOT = Path(__file__).resolve().parents[3]


def _root(tmp_path: Path) -> Path:
    root = tmp_path / "project"
    (root / "worktrees").mkdir(parents=True)
    (root / "live.py").write_text("# probe\n")
    (root / ".wrkslots.yml").write_text(
        "schema: 2\n"
        "machine: node-a\n"
        "worktrees_dir: worktrees\n"
        "default_remote: origin\n"
        "default_landed_ref: refs/remotes/origin/main\n"
        "heartbeat_ttl_seconds: 60\n"
        "liveness_command: live.py\n"
    )
    return root


def _expected(root: Path) -> dict[str, object]:
    config = wrkslots_cli._load_config(str(root), None)
    journals = sorted(path.name for path in wrkslots_cli._outstanding_journals(config))
    machines = sorted(
        path.name.removeprefix("EVENTS.")
        for path in (root / "worktrees").glob("EVENTS.*")
        if path.is_dir() and not path.is_symlink()
    )
    return {"machines": machines, "journals": journals}


def _rust(root: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["cargo", "run", "--quiet", "--release", "--manifest-path", "rs/Cargo.toml",
         "-p", "wrkslotsd", "--", "parse-journals", "--root", str(root)],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
    )


def _events(root: Path, machine: str, payloads: list[dict[str, object]]) -> None:
    events = root / "worktrees" / f"EVENTS.{machine}"
    events.mkdir(exist_ok=True)
    previous = "0" * 64
    for index, payload in enumerate(payloads, start=1):
        core: dict[str, object] = {
            "schema": 1,
            "machine": machine,
            "sequence": index,
            "previous_sha256": previous,
            "recorded_at": "2026-10-26T03:00:00Z",
            "kind": payload.pop("kind"),
            "payload": payload,
        }
        event = {**core, "sha256": wrkslots_cli._event_digest(core)}
        previous = str(event["sha256"])
        (events / f"{index:020d}.json").write_text(json.dumps(event))


def _progress(slot: str, operation: str, machine: str) -> dict[str, object]:
    name = f"{operation.upper()}.{len(machine)}.{machine}.{len(slot)}.{slot}.journal"
    return {
        "kind": "operation-progress-recorded",
        "operation": operation,
        "slot": slot,
        "journal_path": name,
        "journal": {"kind": operation, "slot": slot, "machine": machine},
    }


def test_parse_journals_files_match_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    control = root / "worktrees"
    (control / "FINISH.6.node-a.5.slot-a.journal").write_text("{}")
    (control / "ACTIVE.node-a.journal").write_text("{}")
    (control / "CREATE.6.node-a.5.slot-b.journal").write_text("{}")
    (control / "NOTES.txt").write_text("ignored")
    (control / "EVENTS.node-b").mkdir()
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == _expected(root)


def test_parse_journals_pending_operation_matches_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _events(root, "node-a", [_progress("slot-a", "create", "node-a")])
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == _expected(root)


def test_parse_journals_completed_operation_matches_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    progress = _progress("slot-a", "create", "node-a")
    completion = {
        "kind": "operation-completed",
        "operation": "create",
        "slot": "slot-a",
        "journal_path": progress["journal_path"],
    }
    _events(root, "node-a", [progress, completion])
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == _expected(root)


def test_parse_journals_partial_event_write_matches_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    (root / "worktrees" / "ACTIVE.node-a.journal").write_text("{}")
    events = root / "worktrees" / "EVENTS.node-a"
    events.mkdir()
    (events / "00000000000000000001.json.tmp.abc").write_text("{}")
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == _expected(root)


def test_parse_journals_bare_journal_names_match_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    control = root / "worktrees"
    (control / "ACTIVE.journal").write_text("{}")
    (control / "CREATE.journal").write_text("{}")
    (control / "ACTIVE..journal").write_text("{}")
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == _expected(root)


def test_parse_journals_invalid_event_machine_refuses_like_python(
    tmp_path: Path,
) -> None:
    root = _root(tmp_path)
    (root / "worktrees" / "EVENTS.-bad").mkdir()
    with pytest.raises(Exception) as caught:
        _expected(root)
    assert "invalid machine" in str(caught.value)
    result = _rust(root)
    assert result.returncode == 1
    assert "invalid machine" in result.stderr


@pytest.mark.parametrize(
    ("setup", "fragment"),
    [
        ("corrupt", "append-only event has invalid fields"),
        ("mismatch", "append-only completion does not match its pending operation"),
    ],
)
def test_parse_journals_refusals_name_the_same_problem(
    tmp_path: Path, setup: str, fragment: str
) -> None:
    root = _root(tmp_path)
    if setup == "corrupt":
        events = root / "worktrees" / "EVENTS.node-a"
        events.mkdir()
        (events / "00000000000000000001.json").write_text("{}")
    else:
        progress = _progress("slot-a", "create", "node-a")
        progress["journal_path"] = "ACTIVE.node-a.journal"
        completion: dict[str, object] = {
            "kind": "operation-completed",
            "operation": "finish",
            "slot": "slot-a",
            "journal_path": "ACTIVE.node-a.journal",
        }
        _events(root, "node-a", [progress, completion])
    with pytest.raises(Exception) as caught:
        _expected(root)
    assert fragment in str(caught.value)
    result = _rust(root)
    assert result.returncode == 1
    assert fragment in result.stderr
