"""CLI surface tests: argument shapes, exit codes, and the audit trail.

Exit codes are part of the contract — a caller must be able to tell "policy refused" from "the
command failed" without parsing prose — so each distinguishable outcome is asserted by code.
"""

from __future__ import annotations

import json
import os

import pytest

from herdr_run.audit import audit_path, record
from herdr_run.cli import _default_agent, build_parser, main
from herdr_run.errors import EXIT_REFUSED


# --- argument shapes -------------------------------------------------------------------------------


def test_help_exits_cleanly() -> None:
    with pytest.raises(SystemExit) as excinfo:
        build_parser().parse_args(["--help"])
    assert excinfo.value.code == 0


def test_agent_is_taken_from_the_orc_environment_variable() -> None:
    assert _default_agent({"DG_AGENT_NAME": "hermit-coord"}) == "hermit-coord"


def test_explicit_override_beats_the_environment() -> None:
    assert _default_agent({"HERDR_RUN_AGENT": "chosen", "DG_AGENT_NAME": "ambient"}) == "chosen"


def test_unknown_agent_when_nothing_is_set() -> None:
    assert _default_agent({}) == "unknown-agent"


def test_bare_invocation_prints_help_and_succeeds(capsys: pytest.CaptureFixture[str]) -> None:
    """The `make check-deps` contract: every entrypoint starts cleanly with no arguments."""
    assert main([]) == 0
    out = capsys.readouterr().out
    assert "usage: herdr-run" in out
    assert "no command given" in out


# --- policy-only paths touch no Herdr server ---------------------------------------------------------


def test_check_allows_an_allowlisted_command(capsys: pytest.CaptureFixture[str]) -> None:
    assert main(["check", "with-proxy git ls-remote origin main"]) == 0
    out = capsys.readouterr().out
    assert "ALLOWED" in out
    assert "program=git" in out


def test_check_refuses_a_non_allowlisted_command(capsys: pytest.CaptureFixture[str]) -> None:
    assert main(["check", "curl https://evil.example"]) == EXIT_REFUSED
    assert "REFUSED" in capsys.readouterr().err


def test_dry_run_refusal_never_reaches_a_pane(
    tmp_path: object, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    """A refused command must exit 77 without any Herdr interaction at all."""
    root = str(tmp_path)
    with open(os.path.join(root, ".herdr-run.yaml"), "w", encoding="utf-8") as handle:
        handle.write("spool_dir: spool\n")
    monkeypatch.chdir(root)
    pytest.importorskip("yaml")

    assert main(["some-agent", "rm -rf /"]) == EXIT_REFUSED
    assert "REFUSED" in capsys.readouterr().err


def test_dry_run_prints_the_rendered_command(
    tmp_path: object, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    monkeypatch.chdir(str(tmp_path))
    assert main(["--dry-run", "agent", "git commit -m", "two words"]) == 0
    # Positional tokens are re-joined, then re-split and re-quoted by the allowlist.
    assert "git commit -m" in capsys.readouterr().out


def test_config_subcommand_reports_the_effective_policy(
    tmp_path: object, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    monkeypatch.chdir(str(tmp_path))
    monkeypatch.setenv("DG_AGENT_NAME", "hermit-lander")
    assert main(["config"]) == 0
    document = json.loads(capsys.readouterr().out)
    assert document["workspace"] == "agent-cmds"
    assert document["tab_label"] == "hermit-lander"
    assert document["allow"] == ["git", "gh"]


# --- audit trail ---------------------------------------------------------------------------------------


def test_refusals_are_audited(tmp_path: object, monkeypatch: pytest.MonkeyPatch) -> None:
    """A log that recorded only successes would make the allowlist unobservable after the fact."""
    root = str(tmp_path)
    monkeypatch.chdir(root)
    main(["agent", "curl https://evil.example"])

    path = audit_path(root, ".herdr-run")
    with open(path, encoding="utf-8") as handle:
        entries = [json.loads(line) for line in handle if line.strip()]
    assert len(entries) == 1
    assert entries[0]["verdict"] == "REFUSED"
    assert entries[0]["command"] == "curl https://evil.example"
    assert entries[0]["agent"] == "agent"


def test_audit_appends_rather_than_truncating(tmp_path: object) -> None:
    path = os.path.join(str(tmp_path), "audit.jsonl")
    record(path, agent="a", command="git status", verdict="RAN", detail="exit 0")
    record(path, agent="b", command="curl x", verdict="REFUSED", detail="nope")
    with open(path, encoding="utf-8") as handle:
        entries = [json.loads(line) for line in handle if line.strip()]
    assert [entry["verdict"] for entry in entries] == ["RAN", "REFUSED"]


def test_unwritable_audit_log_does_not_raise() -> None:
    """The audit log must never be the reason a run fails."""
    record("/proc/definitely/not/writable/audit.jsonl", agent="a", command="c", verdict="RAN", detail="d")


# --- spool hygiene: command output must not land where `git add` can pick it up ---------------------


def test_warns_when_the_spool_dir_is_not_gitignored(
    tmp_path: object, capsys: pytest.CaptureFixture[str]
) -> None:
    """Negative bracket: an un-ignored spool inside a real repo must produce a warning."""
    import subprocess

    from herdr_run.audit import spool_is_ignored, warn_if_spool_is_tracked

    root = str(tmp_path)
    subprocess.run(["git", "init", "-q", root], check=True, capture_output=True)

    assert spool_is_ignored(root, ".herdr-run") is False
    assert warn_if_spool_is_tracked(root, ".herdr-run") is True
    assert "NOT git-ignored" in capsys.readouterr().err


def test_stays_quiet_when_the_spool_dir_is_gitignored(
    tmp_path: object, capsys: pytest.CaptureFixture[str]
) -> None:
    """Positive bracket: with the ignore rule present the warning must NOT fire (not merely inert)."""
    import os
    import subprocess

    from herdr_run.audit import spool_is_ignored, warn_if_spool_is_tracked

    root = str(tmp_path)
    subprocess.run(["git", "init", "-q", root], check=True, capture_output=True)
    with open(os.path.join(root, ".gitignore"), "w", encoding="utf-8") as handle:
        handle.write(".herdr-run/\n")

    assert spool_is_ignored(root, ".herdr-run") is True
    assert warn_if_spool_is_tracked(root, ".herdr-run") is False
    assert capsys.readouterr().err == ""


def test_no_claim_outside_a_git_work_tree(tmp_path: object) -> None:
    """Outside a repo the question does not apply, so report None rather than inventing a finding."""
    from herdr_run.audit import spool_is_ignored

    assert spool_is_ignored(str(tmp_path), ".herdr-run") is None


@pytest.mark.parametrize(
    "argv",
    [
        ["--dry-run", "agent", "git status"],
        ["agent", "--dry-run", "git status"],
        ["agent", "git status", "--dry-run"],
        ["--dry-run", "--cwd", "/tmp", "agent", "git status"],
        ["agent", "--cwd", "/tmp", "--dry-run", "git status"],
    ],
)
def test_options_may_appear_between_positionals(
    argv: list[str], tmp_path: object, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    """Regression: `herdr-run agent --cwd DIR 'cmd'` must parse.

    A greedy `nargs="*"` positional plus plain parse_args cannot split around an interleaved option,
    which made the most natural invocation fail with "unrecognized arguments".
    """
    monkeypatch.chdir(str(tmp_path))
    assert main(argv) == 0
    assert "git status" in capsys.readouterr().out
