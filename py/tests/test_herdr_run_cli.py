"""CLI surface tests: argument shapes, exit codes, and the audit trail.

Exit codes are part of the contract — a caller must be able to tell "policy refused" from "the
command failed" without parsing prose — so each distinguishable outcome is asserted by code.
"""

from __future__ import annotations

import io
import json
import os
import sys
from types import SimpleNamespace
from typing import cast

import pytest

from herdr_run.audit import audit_path, record
import herdr_run.cli as cli_module
from herdr_run.cli import _default_agent, build_parser, main
from herdr_run.config import Config
from herdr_run.reap import ReapDecision, ReapPlan, Verdict
from herdr_run.errors import (
    EXIT_CONFIG,
    EXIT_REFUSED,
    ConfigError,
    HerdrRunError,
    HerdrUnavailable,
    PaneBusy,
    Refused,
    RunTimeout,
)


# --- argument shapes -------------------------------------------------------------------------------


def test_help_exits_cleanly() -> None:
    with pytest.raises(SystemExit) as excinfo:
        build_parser().parse_args(["--help"])
    assert excinfo.value.code == 0


def test_agent_is_taken_from_the_orc_environment_variable() -> None:
    assert _default_agent({"DG_AGENT_NAME": "hermit-coord"}) == "hermit-coord"


def test_explicit_override_beats_the_environment() -> None:
    assert (
        _default_agent({"HERDR_RUN_AGENT": "chosen", "DG_AGENT_NAME": "ambient"})
        == "chosen"
    )


def test_unknown_agent_when_nothing_is_set() -> None:
    assert _default_agent({}) == "unknown-agent"


def test_bare_invocation_prints_help_and_succeeds(
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """The `make check-deps` contract: every entrypoint starts cleanly with no arguments."""

    def fail_if_config_is_loaded(*_args: object, **_kwargs: object) -> Config:
        raise AssertionError("a bare help probe must not load ambient project policy")

    monkeypatch.setattr(cli_module, "_load", fail_if_config_is_loaded)
    assert main([]) == 0
    out = capsys.readouterr().out
    assert "usage: herdr-run" in out
    assert "no command given" in out


@pytest.mark.parametrize("value", ["nan", "inf", "-1", "1e300"])
def test_invalid_cli_timeouts_are_rejected(value: str) -> None:
    with pytest.raises(SystemExit) as excinfo:
        build_parser().parse_args(["--timeout", value])
    assert excinfo.value.code == 2


# --- policy-only paths touch no Herdr server ---------------------------------------------------------


def test_check_allows_an_allowlisted_command(
    capsys: pytest.CaptureFixture[str],
) -> None:
    assert main(["check", "with-proxy git ls-remote origin main"]) == 0
    out = capsys.readouterr().out
    assert "ALLOWED" in out
    assert "program=git" in out


def test_check_refuses_a_non_allowlisted_command(
    capsys: pytest.CaptureFixture[str],
) -> None:
    assert main(["check", "curl https://evil.example"]) == EXIT_REFUSED
    assert "REFUSED" in capsys.readouterr().err


def test_dry_run_refusal_never_reaches_a_pane(
    tmp_path: object,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
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
    tmp_path: object,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    monkeypatch.chdir(str(tmp_path))
    assert main(["--dry-run", "agent", "git commit -m 'two words'"]) == 0
    assert "git commit -m" in capsys.readouterr().out


def test_config_subcommand_reports_the_effective_policy(
    tmp_path: object,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    monkeypatch.chdir(str(tmp_path))
    monkeypatch.setenv("DG_AGENT_NAME", "hermit-lander")
    assert main(["config"]) == 0
    document = json.loads(capsys.readouterr().out)
    assert document["workspace"] == "agent-cmds"
    assert document["tab_label"] == "hermit-lander"
    assert document["allow"] == ["git", "gh"]
    assert document["max_panes"] == 64


def test_reap_subcommand_reports_both_sides_and_closes_nothing(
    tmp_path: object,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """The production caller for the reaping policy. Report-only, and counted on both sides."""
    root = str(tmp_path)
    monkeypatch.chdir(root)
    decisions = (
        ReapDecision("w1:p1", "w1:t1", "kvm", Verdict.STALE, "shell is gone"),
        ReapDecision("w1:p2", "w1:t2", "amd", Verdict.SHELL_ALIVE, "still the original process"),
    )
    monkeypatch.setattr(cli_module, "_client", lambda *_args: object())
    monkeypatch.setattr(cli_module, "sweep", lambda *_args, **_kwargs: ReapPlan(decisions))

    assert main(["reap"]) == 0
    document = json.loads(capsys.readouterr().out)
    assert document["counts"]["STALE"] == 1
    assert document["counts"]["SHELL_ALIVE"] == 1
    # Both halves are printed: a report listing only what it would close cannot be audited for
    # the refusals, which are the expensive direction to get wrong.
    assert [entry["pane_id"] for entry in document["reapable"]] == ["w1:p1"]
    assert [entry["pane_id"] for entry in document["declined"]] == ["w1:p2"]
    assert all(entry["reason"] for entry in document["declined"])


class _CapturedText(io.StringIO):
    """Text stream exposing the binary buffer used by a normal terminal stream."""

    def __init__(self) -> None:
        super().__init__()
        self.raw = io.BytesIO()

    @property
    def buffer(self) -> io.BytesIO:
        return self.raw


def test_raw_result_streams_are_emitted_byte_exactly_once(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    stdout = _CapturedText()
    stderr = _CapturedText()
    monkeypatch.setattr(sys, "stdout", stdout)
    monkeypatch.setattr(sys, "stderr", stderr)

    cli_module._emit_raw_output(b"A\x00\xff", b"B\r\n")

    assert stdout.raw.getvalue() == b"A\x00\xff"
    assert stderr.raw.getvalue() == b"B\r\n"


@pytest.mark.parametrize("stream_name", ["stdout", "stderr"])
def test_raw_output_write_failure_is_a_typed_wrapper_error(
    stream_name: str,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    class _BrokenBuffer:
        def write(self, data: bytes) -> int:
            del data
            raise OSError("output pipe closed")

    stream = _CapturedText()
    stream.raw = cast(io.BytesIO, _BrokenBuffer())
    monkeypatch.setattr(sys, stream_name, stream)

    with pytest.raises(HerdrRunError, match=f"cannot write {stream_name}"):
        cli_module._emit_raw_output(b"output", b"error")


def test_timeout_partial_output_write_failure_has_no_traceback(
    tmp_path: object,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    root = str(tmp_path)
    monkeypatch.chdir(root)
    target = SimpleNamespace(pane_id="p1", tab_label="agent")
    timed_out = RunTimeout("still running")
    timed_out.partial_stdout = "partial\n"
    timed_out.partial_stderr = ""
    monkeypatch.setattr(cli_module, "_client", lambda *_args: object())
    monkeypatch.setattr(cli_module, "resolve_target", lambda *_args, **_kwargs: target)
    monkeypatch.setattr(cli_module, "execute", lambda *_args, **_kwargs: _raise(timed_out))
    monkeypatch.setattr(
        cli_module,
        "_emit_raw_output",
        lambda *_args: _raise(HerdrRunError("cannot write stdout: closed")),
    )

    assert main(["agent", "git status"]) == 1
    captured = capsys.readouterr()
    assert "cannot write stdout" in captured.err
    assert "Traceback" not in captured.err


def test_metadata_failure_does_not_mask_completed_command(
    tmp_path: object,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    root = str(tmp_path)
    monkeypatch.chdir(root)
    target = SimpleNamespace(
        pane_id="p1", tab_label="agent", created=(), tab_id="t1", workspace_id="w1"
    )
    result = SimpleNamespace(
        exit_code=23,
        stdout="output",
        stderr="",
        run_id="run-1",
        spool=SimpleNamespace(
            directory=os.path.join(root, ".herdr-run", "runs", "run-1")
        ),
        target=target,
        duration_seconds=0.25,
    )
    monkeypatch.setattr(cli_module, "_client", lambda *_args: object())
    monkeypatch.setattr(cli_module, "resolve_target", lambda *_args, **_kwargs: target)
    monkeypatch.setattr(cli_module, "execute", lambda *_args, **_kwargs: result)
    monkeypatch.setattr(
        cli_module,
        "write_meta",
        lambda *_args: (_ for _ in ()).throw(OSError("read-only metadata")),
    )
    monkeypatch.setattr(
        cli_module, "read_output_bytes", lambda _result: (b"output", b"")
    )

    assert main(["--json", "agent", "git status"]) == 23
    captured = capsys.readouterr()
    assert json.loads(captured.out)["meta"] is None
    assert "WARNING: cannot write run metadata" in captured.err
    with open(audit_path(root, ".herdr-run"), encoding="utf-8") as handle:
        entries = [json.loads(line) for line in handle]
    assert [entry["verdict"] for entry in entries] == ["ADMITTED", "RAN"]
    assert entries[-1]["meta"] is None
    assert "meta_error" in entries[-1]


def _raise(error: Exception) -> object:
    raise error


@pytest.mark.parametrize(
    ("failure_site", "error", "verdict"),
    [
        ("client", HerdrUnavailable("client unavailable"), "HERDRUNAVAILABLE"),
        ("target", HerdrUnavailable("target unavailable"), "HERDRUNAVAILABLE"),
        ("cwd", ConfigError("caller cwd vanished"), "CONFIGERROR"),
        ("execute", PaneBusy("pane stayed busy"), "PANEBUSY"),
    ],
)
def test_doctor_pane_failures_keep_doctor_output_exit_and_audit_shape(
    failure_site: str,
    error: Exception,
    verdict: str,
    tmp_path: object,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """Every failure in the pane half is a failed diagnosis, not a typed early exit."""
    root = str(tmp_path)
    monkeypatch.chdir(root)

    def inside_probe(
        command: object, *, timeout: float
    ) -> SimpleNamespace:
        assert command == [
            "/bin/bash",
            "-lc",
            "with-proxy git ls-remote https://github.com/git/git HEAD",
        ]
        assert timeout == 120
        return SimpleNamespace(returncode=1, stdout="", stderr="blocked in jail\n")

    monkeypatch.setattr(
        cli_module,
        "_bounded_control_command",
        inside_probe,
    )
    target = SimpleNamespace(pane_id="p1", tab_label="agent")
    monkeypatch.setattr(
        cli_module,
        "_client",
        lambda *_args: _raise(error) if failure_site == "client" else object(),
    )
    monkeypatch.setattr(
        cli_module,
        "resolve_target",
        lambda *_args, **_kwargs: (
            _raise(error) if failure_site == "target" else target
        ),
    )
    monkeypatch.setattr(
        cli_module,
        "_resolved_cwd",
        lambda *_args: _raise(error) if failure_site == "cwd" else root,
    )
    monkeypatch.setattr(
        cli_module,
        "execute",
        lambda *_args, **_kwargs: _raise(error),
    )

    assert main(["doctor"]) == 1
    captured = capsys.readouterr()
    assert f"[via pane] FAILED: {error}" in captured.out
    assert "VERDICT: the pane path is NOT working." in captured.out
    assert "Traceback" not in captured.err

    with open(audit_path(root, ".herdr-run"), encoding="utf-8") as handle:
        entries = [json.loads(line) for line in handle]
    assert [entry["verdict"] for entry in entries] == ["ADMITTED", verdict]
    assert entries[-1]["doctor"] is True
    assert entries[-1]["inside_exit_code"] == 1


def test_doctor_pre_admission_refusal_is_marked_as_doctor(
    tmp_path: object,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    root = str(tmp_path)
    monkeypatch.chdir(root)
    monkeypatch.setattr(
        cli_module, "admit", lambda *_args: _raise(Refused("probe refused"))
    )

    assert main(["doctor"]) == EXIT_REFUSED
    assert "herdr-run: probe refused" in capsys.readouterr().err
    with open(audit_path(root, ".herdr-run"), encoding="utf-8") as handle:
        entry = json.loads(handle.read())
    assert entry["verdict"] == "REFUSED"
    assert entry["doctor"] is True


def test_vanished_caller_cwd_is_typed_and_audited_after_admission(
    tmp_path: object,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """Model getcwd(3)'s ENOENT after another process removes the caller's directory."""
    root = str(tmp_path)
    config = Config(project_root=root)
    monkeypatch.setattr(cli_module, "_load", lambda *_args: config)
    monkeypatch.setattr(cli_module, "_client", lambda *_args: object())
    monkeypatch.setattr(
        os,
        "getcwd",
        lambda: _raise(FileNotFoundError("caller directory was removed")),
    )

    assert main(["agent", "git status"]) == EXIT_CONFIG
    captured = capsys.readouterr()
    assert "cannot determine current directory" in captured.err
    assert "Traceback" not in captured.err
    with open(audit_path(root, ".herdr-run"), encoding="utf-8") as handle:
        entries = [json.loads(line) for line in handle]
    assert [entry["verdict"] for entry in entries] == ["ADMITTED", "CONFIGERROR"]


# --- audit trail ---------------------------------------------------------------------------------------


def test_refusals_are_audited(
    tmp_path: object, monkeypatch: pytest.MonkeyPatch
) -> None:
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
    os.chmod(str(tmp_path), 0o755)
    assert record(path, agent="a", command="git status", verdict="RAN", detail="exit 0")
    assert record(path, agent="b", command="curl x", verdict="REFUSED", detail="nope")
    with open(path, encoding="utf-8") as handle:
        entries = [json.loads(line) for line in handle if line.strip()]
    assert [entry["verdict"] for entry in entries] == ["RAN", "REFUSED"]
    assert os.stat(path).st_mode & 0o777 == 0o600
    assert os.stat(str(tmp_path)).st_mode & 0o777 == 0o755


def test_unwritable_audit_log_does_not_raise() -> None:
    """The audit log must never be the reason a run fails."""
    assert not record(
        "/proc/definitely/not/writable/audit.jsonl",
        agent="a",
        command="c",
        verdict="RAN",
        detail="d",
    )


# --- spool hygiene: command output must not land where `git add` can pick it up ---------------------


def test_spool_ignore_probe_uses_bounded_group_runner(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    import herdr_run.audit as audit_module

    def bounded(command: list[str], *, timeout: float) -> SimpleNamespace:
        assert command == [
            "git",
            "-C",
            "/repo",
            "check-ignore",
            "-q",
            "--",
            "/repo/.herdr-run/probe",
        ]
        assert timeout == 10
        return SimpleNamespace(returncode=1, stdout="", stderr="")

    monkeypatch.setattr(audit_module, "_bounded_control_command", bounded)
    assert audit_module.spool_is_ignored("/repo", ".herdr-run") is False


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
    argv: list[str],
    tmp_path: object,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """Regression: `herdr-run agent --cwd DIR 'cmd'` must parse.

    A greedy `nargs="*"` positional plus plain parse_args cannot split around an interleaved option,
    which made the most natural invocation fail with "unrecognized arguments".
    """
    monkeypatch.chdir(str(tmp_path))
    assert main(argv) == 0
    assert "git status" in capsys.readouterr().out


def test_loose_argv_words_are_refused_not_silently_rejoined(
    tmp_path: object, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    """Re-joining loose words would silently destroy quoting; refuse instead.

    `herdr-run agent git commit -m "two words"` would otherwise arrive at git as `-m two words`,
    and nothing in the output would reveal that the caller's quoting had been discarded.
    """
    monkeypatch.chdir(str(tmp_path))
    # Flagless loose words: argparse accepts them as positionals, so OUR guard must refuse.
    assert main(["agent", "git", "status", "--", "path with spaces"]) == 2
    err = capsys.readouterr().err
    assert "ONE quoted argument" in err
    assert "silently change the quoting" in err


def test_loose_argv_containing_an_unknown_flag_is_also_refused(
    tmp_path: object, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The other loose shape: argparse itself rejects it, so it is refused too, just earlier.

    Recorded because the two shapes fail through DIFFERENT paths and both must be non-silent:
    `-m` is not a herdr-run flag, so argparse exits before the command is ever assembled.
    """
    monkeypatch.chdir(str(tmp_path))
    with pytest.raises(SystemExit) as excinfo:
        main(["agent", "git", "commit", "-m", "two words"])
    assert excinfo.value.code == 2


def test_the_documented_two_positional_form_still_works(
    tmp_path: object, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    """Positive control: agent + one quoted command is the supported shape and must not regress."""
    monkeypatch.chdir(str(tmp_path))
    assert main(["--dry-run", "agent", "with-proxy git ls-remote origin main"]) == 0
    assert "with-proxy git ls-remote origin main" in capsys.readouterr().out


def test_explicit_agent_never_swallows_a_leading_wrapper(
    tmp_path: object, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    """`--agent X with-proxy '<cmd>'` must NOT drop `with-proxy`.

    It used to: with two positionals the leading one was discarded from the command even when
    --agent had already supplied the name. The command still ran, minus its proxy wrapper, so `gh`
    dialled GitHub directly and reported "network is unreachable" -- indistinguishable from a real
    egress outage, and the reason a whole fleet concluded `gh` was unusable through the pane.
    Refusing is the safe reading; silently running a DIFFERENT command is not.
    """
    monkeypatch.chdir(str(tmp_path))
    assert main(["--dry-run", "--agent", "someagent", "with-proxy", "git ls-remote origin"]) == 2
    err = capsys.readouterr().err
    assert "ONE quoted argument" in err
    # The suggestion must put the wrapper back INSIDE the quotes, not drop it again.
    assert "'with-proxy git ls-remote origin'" in err


def test_explicit_agent_with_one_quoted_command_still_works(
    tmp_path: object, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    """Positive control for the guard above: the correct shape must still render in full."""
    monkeypatch.chdir(str(tmp_path))
    assert main(["--dry-run", "--agent", "someagent", "with-proxy git ls-remote origin main"]) == 0
    assert "with-proxy git ls-remote origin main" in capsys.readouterr().out


def test_command_runs_in_the_callers_cwd_not_the_config_directory(
    tmp_path: object, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Regression: a config file ABOVE the caller must not relocate where the command runs.

    Config discovery walks up to the nearest .herdr-run.yaml, so a file at the top of a multi-repo
    tree used to make every nested worktree run its command in that top directory instead of the
    one the caller was standing in -- silent mistargeting that reads as the wrong repo answering.
    """
    import argparse
    import os

    from herdr_run.cli import _resolved_cwd
    from herdr_run.config import Config

    root = str(tmp_path)
    nested = os.path.join(root, "worktrees", "slot", "repo")
    os.makedirs(nested)
    config = Config(project_root=root)  # config lives at the top...
    monkeypatch.chdir(nested)  # ...caller stands in the nested worktree

    assert _resolved_cwd(config, argparse.Namespace(cwd=None)) == os.path.realpath(nested) or (
        _resolved_cwd(config, argparse.Namespace(cwd=None)) == nested
    )


def test_explicit_cwd_still_wins(tmp_path: object) -> None:
    """Positive control: --cwd is still honoured over both the config and the caller's cwd."""
    import argparse

    from herdr_run.cli import _resolved_cwd
    from herdr_run.config import Config

    target = str(tmp_path)
    assert _resolved_cwd(Config(project_root="/elsewhere"), argparse.Namespace(cwd=target)) == target


def test_explicit_empty_cwd_means_the_project_root() -> None:
    """An explicitly supplied empty value is not the same as omitting --cwd."""
    import argparse

    from herdr_run.cli import _resolved_cwd
    from herdr_run.config import Config

    assert _resolved_cwd(
        Config(project_root="/project", cwd="configured"), argparse.Namespace(cwd="")
    ) == "/project"
