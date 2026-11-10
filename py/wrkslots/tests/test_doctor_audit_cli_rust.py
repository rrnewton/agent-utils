"""Differential: wrkslotsd doctor-text/audit-text against Python.

#562 rust-primary-wrkslots soak slice: the Python `doctor` envelope
(schema/roots/layout/machines/event_counts/active/max_active_slots/
findings) and human form, and the Python `audit` human form and gate
exit codes, at parity, so the soak launcher can run both commands in
Rust in both formats. Findings include the real filesystem/Git
storage census on this fixture (missing slot directories).
"""

from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path
from typing import cast

import pytest

from wrkslots.tests.test_doctor_state_rust import _fixture, _root

from wrkslots.tests.rust_binary import rust_wrkslotsd
from wrkslots.tests.test_status_cli_rust import change_mid_read_env


AU_ROOT = Path(__file__).resolve().parents[3]


def _python(root: Path, command: str, *args: str) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    env["PYTHONPATH"] = str(AU_ROOT / "py")
    return subprocess.run(
        ["python3", "-m", "wrkslots", "--project-root", str(root), command, *args],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900, env=env,
    )


def _rust(root: Path, command: str, *args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(rust_wrkslotsd()), command, "--root", str(root), *args],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
    )


def _normalized(document: dict[str, object]) -> dict[str, object]:
    active = cast(list[dict[str, object]], document["active"])
    for record in active:
        age = cast(int, record.pop("heartbeat_age_seconds"))
        assert age >= 0
    return document


def test_doctor_json_envelope_matches_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _fixture(root)
    python_result = _python(root, "doctor", "--format", "json")
    rust_result = _rust(root, "doctor-text", "--format", "json")
    assert python_result.returncode == 0, python_result.stderr
    assert rust_result.returncode == 0, rust_result.stderr
    python_document = cast(dict[str, object], json.loads(python_result.stdout))
    rust_document = cast(dict[str, object], json.loads(rust_result.stdout))
    assert _normalized(rust_document) == _normalized(python_document)
    kinds = [str(item["kind"]) for item in cast(list[dict[str, object]], rust_document["findings"])]
    # Storage drift (missing slot directories) leads, then the
    # operational findings the scoped parse-doctor already covered.
    assert "directory-without-row" in kinds or "row-without-directory" in kinds
    for kind in (
        "invalid-hold",
        "dead-owner",
        "unfinished-journal",
        "partial-atomic-update",
        "cap-breach",
    ):
        assert kind in kinds


def test_doctor_human_form_matches_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _fixture(root)
    python_result = _python(root, "doctor")
    rust_result = _rust(root, "doctor-text")
    assert python_result.returncode == 0, python_result.stderr
    assert rust_result.returncode == 0, rust_result.stderr
    assert rust_result.stdout == python_result.stdout


def test_doctor_all_machines_json_matches_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _fixture(root)
    python_result = _python(root, "doctor", "--all-machines", "--format", "json")
    rust_result = _rust(root, "doctor-text", "--all-machines", "--format", "json")
    assert python_result.returncode == 0, python_result.stderr
    assert rust_result.returncode == 0, rust_result.stderr
    python_document = cast(dict[str, object], json.loads(python_result.stdout))
    rust_document = cast(dict[str, object], json.loads(rust_result.stdout))
    assert _normalized(rust_document) == _normalized(python_document)


def test_doctor_owner_release_void_matches_python(tmp_path: Path) -> None:
    from wrkslots import cli as wrkslots_cli

    root = _root(tmp_path)
    _fixture(root)
    control = root / "worktrees"
    active = json.loads((control / "ACTIVE.node-a.json").read_text())
    record = active["slots"][0]
    core: dict[str, object] = {
        "schema": 1,
        "machine": "node-a",
        "sequence": 1,
        "previous_sha256": "0" * 64,
        "recorded_at": "2026-10-26T04:00:00Z",
        "kind": "owner-released",
        "payload": {
            "slot": record["slot"],
            "generation": record["generation"],
            "heartbeat_at": record["heartbeat_at"],
            "owner": record["owner"],
            "agent": record["agent"],
            "handoff": {
                "path": str(control / "HANDOFF.node-a.slot-a.1.json"),
                "sha256": "c" * 64,
                "source": "write-handoff",
                "intent_sequence": 1,
            },
        },
    }
    # A present event log must hold a complete imported state before
    # the release event, in both editions.
    (control / "HOLD.6.node-a.slot-b.json").unlink()
    imported: dict[str, object] = {
        "schema": 1,
        "machine": "node-a",
        "sequence": 1,
        "previous_sha256": "0" * 64,
        "recorded_at": "2026-10-26T03:00:00Z",
        "kind": "state-imported",
        "payload": {
            "active": active,
            "archive": {"schema": 2, "machine": "node-a", "revision": 3, "records": []},
            "holds": [],
        },
    }
    first = {**imported, "sha256": wrkslots_cli._event_digest(imported)}
    core["sequence"] = 2
    core["previous_sha256"] = first["sha256"]
    event = {**core, "sha256": wrkslots_cli._event_digest(core)}
    events = control / "EVENTS.node-a"
    (events / "00000000000000000001.json").write_text(json.dumps(first))
    (events / "00000000000000000002.json").write_text(json.dumps(event))
    python_result = _python(root, "doctor", "--format", "json")
    rust_result = _rust(root, "doctor-text", "--format", "json")
    assert python_result.returncode == 0, python_result.stderr
    assert rust_result.returncode == 0, rust_result.stderr
    python_document = cast(dict[str, object], json.loads(python_result.stdout))
    rust_document = cast(dict[str, object], json.loads(rust_result.stdout))
    assert _normalized(rust_document) == _normalized(python_document)
    kinds = [str(item["kind"]) for item in cast(list[dict[str, object]], rust_document["findings"])]
    assert "owner-release-void" in kinds
    rows = cast(list[dict[str, object]], rust_document["active"])
    assert rows[0]["owner_release"]["state"] == "void"  # type: ignore[index]


def test_doctor_two_unavailable_repositories_match_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _fixture(root)
    control = root / "worktrees"
    active_path = control / "ACTIVE.node-a.json"
    active = json.loads(active_path.read_text())
    active["slots"][1]["checkouts"][0]["repository"] = "../other-missing-repo"
    active_path.write_text(json.dumps(active))
    python_result = _python(root, "doctor", "--format", "json")
    rust_result = _rust(root, "doctor-text", "--format", "json")
    assert python_result.returncode == 0, python_result.stderr
    assert rust_result.returncode == 0, rust_result.stderr
    python_document = cast(dict[str, object], json.loads(python_result.stdout))
    rust_document = cast(dict[str, object], json.loads(rust_result.stdout))
    assert _normalized(rust_document) == _normalized(python_document)
    kinds = [str(item["kind"]) for item in cast(list[dict[str, object]], rust_document["findings"])]
    assert kinds.count("repository-evidence-unavailable") == 2


def test_doctor_refuses_duplicate_slot_in_both_editions(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _fixture(root)
    control = root / "worktrees"
    other_path = control / "ACTIVE.node-b.json"
    other = json.loads(other_path.read_text())
    other["slots"][0]["slot"] = "slot-a"
    other_path.write_text(json.dumps(other))
    python_result = _python(root, "doctor", "--format", "json")
    rust_result = _rust(root, "doctor-text", "--format", "json")
    assert python_result.returncode != 0
    assert rust_result.returncode != 0


@pytest.mark.load_quarantine  # #1267 wrkslots-load-robust
def test_audit_human_and_gate_match_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _fixture(root)
    # The audit refuses an interrupted registry in both editions;
    # remove the fixture's partial-update artefacts so the audit runs.
    control = root / "worktrees"
    (control / "ACTIVE.node-a.journal").unlink()
    (control / "ACTIVE.node-a.json.tmp.1").unlink()
    (control / "EVENTS.node-a" / "00000000000000000001.json.tmp.x").unlink()
    (control / "HOLD.6.node-a.slot-b.json").unlink()
    # The records name ../upstream-repo as their source repository;
    # give the audit a real repository to resolve.
    upstream = tmp_path / "upstream-repo"
    upstream.mkdir()
    subprocess.run(["git", "init", "-q", str(upstream)], check=True, timeout=60)
    subprocess.run(
        ["git", "-C", str(upstream), "commit", "-q", "--allow-empty", "-m", "seed"],
        check=True, timeout=60,
        env={**os.environ, "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@t",
             "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@t"},
    )
    python_result = _python(root, "audit", "--gate")
    rust_result = _rust(root, "audit-text", "--gate")
    assert rust_result.returncode == python_result.returncode, (
        rust_result.stdout,
        rust_result.stderr,
        python_result.stdout,
        python_result.stderr,
    )
    # Disk free space and heartbeat-derived verdicts can move between
    # the two runs; the verdict lines and gate block must agree once
    # the header's free-space figure is scrubbed.
    def scrub(text: str) -> list[str]:
        lines = []
        for line in text.splitlines():
            if line.startswith("worktrees="):
                head, _, _tail = line.partition(" free=")
                line = f"{head} free=<free>"
            lines.append(line)
        return lines

    assert scrub(rust_result.stdout) == scrub(python_result.stdout)


# #1097 wrkslots-doctor-identity: effective Git identity against configuration.identity.

_IDENTITY = "identity: {name: Slot Bot, email: slot-bot@example.invalid}\n"


def _identity_project(tmp_path: Path, *, identity: bool = True) -> tuple[Path, Path, Path]:
    from wrkslots.tests.test_remove_write_rust import _fixture as checkout_fixture

    root = checkout_fixture(tmp_path)
    if identity:
        config = root / ".wrkslots.yml"
        config.write_text(config.read_text() + _IDENTITY)
    home = tmp_path / "home"
    home.mkdir()
    return root, root / "worktrees" / "slot-a" / "repo", home


def _identity_git(cwd: Path, *args: str) -> None:
    from wrkslots.tests.test_remove_write_rust import _git

    _git(cwd, *args)


def _identity_findings(root: Path, home: Path) -> list[dict[str, object]]:
    """Both editions' identity-drift findings, which must agree."""

    env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    env.update({"HOME": str(home), "XDG_CONFIG_HOME": str(home / ".config")})
    python_env = dict(env, PYTHONPATH=str(AU_ROOT / "py"))
    python_result = subprocess.run(
        ["python3", "-m", "wrkslots", "--project-root", str(root), "doctor", "--format", "json"],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900, env=python_env,
    )
    rust_result = subprocess.run(
        [str(rust_wrkslotsd()), "doctor-text", "--root", str(root), "--format", "json"],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900, env=env,
    )
    assert python_result.returncode == 0, python_result.stderr
    assert rust_result.returncode == 0, rust_result.stderr

    def drift(text: str) -> list[dict[str, object]]:
        findings = cast(list[dict[str, object]], json.loads(text)["findings"])
        return [item for item in findings if item["kind"] == "identity-drift"]

    python_drift = drift(python_result.stdout)
    assert drift(rust_result.stdout) == python_drift
    return python_drift


def test_doctor_is_silent_for_a_checkout_with_the_configured_identity(tmp_path: Path) -> None:
    root, checkout, home = _identity_project(tmp_path)
    _identity_git(tmp_path / "upstream-repo", "config", "extensions.worktreeConfig", "true")
    _identity_git(checkout, "config", "--worktree", "user.name", "Slot Bot")
    _identity_git(checkout, "config", "--worktree", "user.email", "slot-bot@example.invalid")
    assert _identity_findings(root, home) == []


def test_doctor_is_silent_without_a_configured_identity(tmp_path: Path) -> None:
    root, _checkout, home = _identity_project(tmp_path, identity=False)
    assert _identity_findings(root, home) == []


def test_doctor_names_a_worktree_override_and_its_origin(tmp_path: Path) -> None:
    root, checkout, home = _identity_project(tmp_path)
    _identity_git(tmp_path / "upstream-repo", "config", "extensions.worktreeConfig", "true")
    _identity_git(checkout, "config", "--worktree", "user.name", "Slot Bot")
    _identity_git(checkout, "config", "--worktree", "user.email", "agent@example.invalid")
    findings = _identity_findings(root, home)
    assert len(findings) == 1
    finding = findings[0]
    assert (finding["slot"], finding["checkout"], finding["scope"]) == ("slot-a", "repo", "row")
    detail = str(finding["detail"])
    assert detail.startswith("checkout repo: user.email is 'agent@example.invalid' from file:")
    assert "config.worktree" in detail
    assert detail.endswith(", configured 'slot-bot@example.invalid'")
    assert "user.name" not in detail


def test_doctor_names_a_global_fall_through(tmp_path: Path) -> None:
    root, _checkout, home = _identity_project(tmp_path)
    (home / ".gitconfig").write_text(
        "[user]\n\tname = Slot Bot\n\temail = person@example.invalid\n"
    )
    findings = _identity_findings(root, home)
    assert len(findings) == 1
    assert str(findings[0]["detail"]) == (
        f"checkout repo: user.email is 'person@example.invalid' from file:{home / '.gitconfig'}, "
        "configured 'slot-bot@example.invalid'"
    )


def test_doctor_names_a_shared_config_write(tmp_path: Path) -> None:
    root, checkout, home = _identity_project(tmp_path)
    _identity_git(checkout, "config", "user.name", "Slot Bot")
    _identity_git(checkout, "config", "user.email", "shared@example.invalid")
    findings = _identity_findings(root, home)
    assert len(findings) == 1
    detail = str(findings[0]["detail"])
    assert "user.email is 'shared@example.invalid' from file:" in detail
    assert "upstream-repo/.git/config," in detail


def test_doctor_names_an_unset_identity(tmp_path: Path) -> None:
    root, _checkout, home = _identity_project(tmp_path)
    findings = _identity_findings(root, home)
    assert [str(item["detail"]) for item in findings] == [
        "checkout repo: user.name is unset, configured 'Slot Bot'; "
        "user.email is unset, configured 'slot-bot@example.invalid'"
    ]

def test_both_editions_find_the_harness_root_from_a_slot_through_symlinks(
    tmp_path: Path,
) -> None:
    """#984: called through symlinks from a slot, each edition finds the harness root.

    The executables are reached through a harness's own symlinks, and the
    working directory through a symlinked path; both editions search from the
    working directory upward, never from the executable's location.
    """

    from wrkslots.tests.test_remove_write_rust import _fixture as checkout_fixture

    root = checkout_fixture(tmp_path / "real")
    via = tmp_path / "via"
    via.symlink_to(root, target_is_directory=True)
    slot = via / "worktrees" / "slot-a" / "repo"
    links = tmp_path / "harness-bin"
    links.mkdir()
    (links / "wrkslots").symlink_to(AU_ROOT / "py" / "bin" / "wrkslots")
    (links / "wrkslotsd").symlink_to(rust_wrkslotsd())
    for command in (
        [str(links / "wrkslots"), "doctor", "--format", "json"],
        [str(links / "wrkslotsd"), "doctor-text", "--format", "json"],
    ):
        result = subprocess.run(
            command, cwd=slot, capture_output=True, text=True, check=False, timeout=900,
        )
        assert result.returncode == 0, (command[0], result.stderr)
        assert json.loads(result.stdout)["project_root"] == str(root.resolve()), command[0]


def test_doctor_publishes_the_registry_after_a_change_mid_read_like_python(
    tmp_path: Path,
) -> None:
    """#1200 rust-status-consistency: a report never mixes two instants of the registry."""
    root = _root(tmp_path)
    _fixture(root)
    rust_result = subprocess.run(
        [str(rust_wrkslotsd()), "doctor-text", "--root", str(root), "--format", "json"],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
        env=change_mid_read_env(root, tmp_path),
    )
    python_result = _python(root, "doctor", "--format", "json")
    assert rust_result.returncode == python_result.returncode, (
        rust_result.stderr, python_result.stderr,
    )
    rust_document = cast(dict[str, object], json.loads(rust_result.stdout))
    first = cast(list[dict[str, object]], rust_document["active"])[0]
    assert first["task"] == "changed while measuring", first
    python_document = cast(dict[str, object], json.loads(python_result.stdout))
    assert _normalized(rust_document) == _normalized(python_document)


def test_doctor_worktrees_image_scope_matches_python(tmp_path: Path) -> None:
    for label, machines in (("listed", "[node-a]"), ("unlisted", "[other-host]")):
        base = tmp_path / label
        base.mkdir()
        root = _root(base)
        _fixture(root)
        config_path = root / ".wrkslots.yml"
        config_path.write_text(
            config_path.read_text()
            + f"worktrees_image: {{path: wt.img, size_bytes: 2147483648, machines: {machines}}}\n"
        )
        python = _python(root, "doctor", "--format", "json")
        rust = _rust(root, "doctor-text", "--format", "json")
        # stderr may carry Python's once-per-process load warning
        # (#1148 follow-up); the scope field itself must match.
        assert rust.returncode == python.returncode
        assert _normalized(json.loads(rust.stdout)) == _normalized(json.loads(python.stdout))
        assert json.loads(rust.stdout)["worktrees_image"].startswith(
            "applies:" if machines == "[node-a]" else "skipped:"
        )
        text_python = _python(root, "doctor")
        text_rust = _rust(root, "doctor-text")
        assert (text_rust.returncode, text_rust.stdout) == (
            text_python.returncode,
            text_python.stdout,
        )
        assert "worktrees-image:" in text_rust.stdout
