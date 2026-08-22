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
from herdr_run.cli import (
    SUBCOMMANDS,
    _default_agent,
    help_text,
    main,
    subcommand_help_text,
)
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


def test_help_exits_cleanly(capsys: pytest.CaptureFixture[str]) -> None:
    assert main(["--help"]) == 0
    captured = capsys.readouterr()
    assert captured.out.startswith("usage: herdr-run [-h] [--version]")
    assert captured.err == ""


def test_agent_is_taken_from_the_orc_environment_variable() -> None:
    assert _default_agent({"DG_AGENT_NAME": "widget-coord"}) == "widget-coord"


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

    monkeypatch.setattr(cli_module, "load_config", fail_if_config_is_loaded)
    assert main([]) == 0
    captured = capsys.readouterr()
    assert "usage: herdr-run" in captured.out
    assert "subcommands:" in captured.out
    assert captured.err == ""


@pytest.mark.parametrize("value", ["nan", "inf", "-1", "1e300"])
def test_invalid_cli_timeouts_are_rejected(
    value: str, capsys: pytest.CaptureFixture[str]
) -> None:
    assert main(["run", "--timeout", value, "git status"]) == 2
    assert "argument --timeout" in capsys.readouterr().err


# --- the two levels ------------------------------------------------------------------------------


@pytest.mark.parametrize(
    "argv,fragment",
    [
        (["--cwd", "/tmp", "run", "git status"], "put it AFTER the subcommand"),
        (["--timeout", "1", "run", "git status"], "put it AFTER the subcommand"),
        (["--wait-ready", "1", "run", "git status"], "put it AFTER the subcommand"),
        (["--dry-run", "run", "git status"], "put it AFTER the subcommand"),
        (["--no-cache", "target"], "'run' or 'target'"),
        (["run", "--agent", "a", "git status"], "this is a GLOBAL option"),
        (["run", "--config", "x.yaml", "git status"], "this is a GLOBAL option"),
        (["run", "--json", "git status"], "this is a GLOBAL option"),
        (["check", "--dry-run", "git status"], "a 'run' option, not a 'check' option"),
        (["reap", "--cwd", "/tmp"], "a 'run' option, not a 'reap' option"),
        (["target", "--dry-run"], "a 'run' option, not a 'target' option"),
    ],
)
def test_an_option_at_the_wrong_level_is_refused_and_says_which_level_it_belongs_to(
    argv: list[str], fragment: str, capsys: pytest.CaptureFixture[str]
) -> None:
    """The two levels must not leak into each other, in either direction.

    This is the defect the surface exists to fix, so it is pinned as behaviour rather than left to
    the help text: an accepted `herdr-run --cwd /tmp run ...` would quietly restore the very mixing
    that made the old surface unreadable.
    """
    assert main(argv) == 2
    captured = capsys.readouterr()
    assert fragment in captured.err, captured.err
    assert captured.out == ""


@pytest.mark.parametrize(
    "argv",
    [
        ["--config", "x.yaml", "init"],
        ["--config=x.yaml", "init"],
        ["--config", "x.yaml", "init", "--force"],
        ["--config", "x.yaml", "quickstart"],
        ["--config", "x.yaml", "userguide"],
    ],
)
def test_config_is_refused_by_the_subcommands_that_read_no_configuration(
    argv: list[str],
    capsys: pytest.CaptureFixture[str],
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: object,
) -> None:
    """A global option offered to a subcommand that cannot observe it must not be ignored.

    `--config` is not inert: it reads as an instruction about which configuration file to use, and
    `herdr-run --config P init` reads as an instruction to write `P`. It used to be accepted and
    silently disregarded, which is the same accept-anything defect the two-level surface exists to
    remove.

    Run from a scratch directory, because a regression here means `init` really runs.
    """
    monkeypatch.chdir(str(tmp_path))
    assert main(argv) == 2
    captured = capsys.readouterr()
    assert "argument --config:" in captured.err, captured.err
    assert "reads no configuration file" in captured.err, captured.err
    assert captured.out == ""


def test_a_refused_config_before_init_writes_nothing(
    capsys: pytest.CaptureFixture[str],
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: object,
) -> None:
    """The refusal is total: `init` never runs, so no configuration file appears."""
    root = str(tmp_path)
    monkeypatch.chdir(root)
    assert main(["--config", os.path.join(root, "elsewhere.yaml"), "init"]) == 2
    assert "cd DIRECTORY && herdr-run init" in capsys.readouterr().err
    assert os.listdir(root) == []


def test_the_subcommands_own_parse_still_wins_over_the_config_refusal(
    capsys: pytest.CaptureFixture[str],
) -> None:
    """`--help` still helps, and a bad local option is still named as the local option it is."""
    assert main(["--config", "x.yaml", "init", "--help"]) == 0
    assert "usage: herdr-run [GLOBAL OPTIONS] init" in capsys.readouterr().out

    assert main(["--config", "x.yaml", "init", "--nonsense"]) == 2
    assert "--nonsense" in capsys.readouterr().err


def test_the_bare_command_form_is_refused_with_the_run_replacement(
    capsys: pytest.CaptureFixture[str],
) -> None:
    """The bare form is gone, and its removal names the exact replacement."""
    assert main(["git status"]) == 2
    err = capsys.readouterr().err
    assert "unknown subcommand 'git status'" in err
    assert "herdr-run run 'git status'" in err

    assert main(["release-agent", "git status"]) == 2
    err = capsys.readouterr().err
    assert "herdr-run --agent release-agent run 'git status'" in err

    assert main(["stauts"]) == 2
    err = capsys.readouterr().err
    assert "unknown subcommand 'stauts'" in err
    assert "Subcommands: " in err
    # A single word is a mistyped subcommand, not a command line; suggesting `run 'stauts'` would
    # send the reader off to debug the wrong thing.
    assert "run 'stauts'" not in err


def test_run_needs_exactly_one_quoted_command(
    capsys: pytest.CaptureFixture[str],
) -> None:
    assert main(["run"]) == 2
    assert "needs a command to run" in capsys.readouterr().err
    assert main(["run", "git", "status"]) == 2
    err = capsys.readouterr().err
    assert "ONE quoted argument" in err
    assert "herdr-run run 'git status'" in err


def test_a_double_dash_hands_the_rest_to_the_command(
    tmp_path: object,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    monkeypatch.chdir(str(tmp_path))
    assert main(["run", "--dry-run", "--", "git --help"]) == 0
    assert capsys.readouterr().out == "git --help\n"
    assert main(["check", "--", "--version"]) == EXIT_REFUSED


def test_option_values_cannot_be_stolen_by_help_or_version(
    capsys: pytest.CaptureFixture[str],
) -> None:
    assert main(["--agent", "--help", "run", "x"]) == 2
    assert "argument --agent: expected one value" in capsys.readouterr().err
    assert main(["run", "--cwd", "--version", "x"]) == 2
    assert "argument --cwd: expected one value" in capsys.readouterr().err


def test_every_subcommand_is_reachable_and_uniquely_named() -> None:
    assert sorted(name for name, _summary in SUBCOMMANDS) == [
        "check",
        "config",
        "init",
        "net-doctor",
        "quickstart",
        "reap",
        "run",
        "status",
        "target",
        "userguide",
    ]
    assert all(summary for _name, summary in SUBCOMMANDS)


def _option_block(text: str, header: str) -> str:
    """Return the contiguous indented lines that follow the first line starting with `header`.

    Only the option BLOCK is inspected: an example line naming a global option is teaching where
    that option goes, which is the opposite of the mixing being guarded against.
    """
    lines = text.splitlines()
    for index, line in enumerate(lines):
        if line.startswith(header):
            body: list[str] = []
            for candidate in lines[index + 1 :]:
                if not candidate.strip():
                    break
                body.append(candidate)
            return "\n".join(body)
    return ""


def test_each_help_level_documents_exactly_its_own_options() -> None:
    """Neither help level may document the other's options.

    The complaint that produced this surface was that every flag was global whether or not it meant
    anything to the thing being invoked, and help that lists the wrong options is how that state of
    affairs comes back without anybody deciding to bring it back.
    """
    top = help_text()
    for name, summary in SUBCOMMANDS:
        assert name in top and summary in top, name
    top_options = _option_block(top, "global options")
    for option in ("--config", "--agent", "--json", "--version", "--help"):
        assert option in top_options, option
    for local in ("--cwd", "--timeout", "--wait-ready", "--force", "--dry-run", "--no-cache"):
        assert local not in top_options, local

    run_options = _option_block(subcommand_help_text("run"), "options:")
    for local in ("--cwd", "--timeout", "--wait-ready", "--no-cache", "--dry-run"):
        assert local in run_options, local
    for global_option in ("--config", "--agent", "--json", "--version"):
        assert global_option not in run_options, global_option

    assert "--force" in _option_block(subcommand_help_text("init"), "options:")
    assert "--no-cache" in _option_block(subcommand_help_text("target"), "options:")
    for bare in ("check", "config", "reap", "net-doctor", "quickstart", "userguide", "status"):
        block = _option_block(subcommand_help_text(bare), "options:")
        assert "-h, --help" in block, bare
        for local in ("--cwd", "--timeout", "--dry-run", "--force", "--no-cache"):
            assert local not in block, f"{bare}: {local}"


def test_help_distinguishes_the_quickstart_from_the_userguide() -> None:
    """The top-level help must say what each of the two documentation commands is FOR."""
    top = help_text()
    assert "Two documentation commands" in top
    assert "one screen" in top
    assert "the complete reference" in top


def test_each_documentation_subcommand_prints_its_own_document(
    capsys: pytest.CaptureFixture[str],
) -> None:
    assert main(["quickstart"]) == 0
    quickstart = capsys.readouterr().out
    assert quickstart.startswith("# herdr-run — quickstart\n")
    assert main(["userguide"]) == 0
    guide = capsys.readouterr().out
    assert guide.startswith("# herdr-run — user guide\n")
    # Two documentation commands printing the same text would be one command with two names.
    assert quickstart != guide
    assert 2 * quickstart.count("\n") < guide.count("\n")
    assert "herdr-run userguide" in quickstart


def test_documentation_prints_even_when_the_configuration_cannot_be_parsed(
    tmp_path: object,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """A broken config must not be able to withhold the instructions for fixing it."""
    root = str(tmp_path)
    with open(os.path.join(root, ".herdr-run.yaml"), "w", encoding="utf-8") as handle:
        handle.write("allow: [\n")
    monkeypatch.chdir(root)
    for subcommand in ("quickstart", "userguide"):
        assert main([subcommand]) == 0, subcommand
        assert capsys.readouterr().out.startswith("# herdr-run"), subcommand


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

    assert main(["--agent", "some-agent", "run", "rm -rf /"]) == EXIT_REFUSED
    assert "REFUSED" in capsys.readouterr().err


def test_dry_run_prints_the_rendered_command(
    tmp_path: object,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    monkeypatch.chdir(str(tmp_path))
    assert main(["--agent", "agent", "run", "--dry-run", "git commit -m 'two words'"]) == 0
    assert "git commit -m" in capsys.readouterr().out


def test_config_subcommand_reports_the_effective_policy(
    tmp_path: object,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    monkeypatch.chdir(str(tmp_path))
    monkeypatch.setenv("DG_AGENT_NAME", "widget-lander")
    assert main(["config"]) == 0
    document = json.loads(capsys.readouterr().out)
    assert document["workspace"] == "agent-cmds"
    assert document["tab_label"] == "widget-lander"
    assert document["allow"] == ["git", "gh"]
    assert document["max_panes"] == 32


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
    # The report also has to state the bound on what it could POSSIBLY have considered. Candidates
    # are the panes named by surviving run records, and herdr-run's own retention deletes those
    # records -- so the oldest leaked tabs, the ones the pane cap exists to bound, are exactly the
    # ones missing from this count. Printing the window keeps "considered: 2" from implying more.
    assert document["candidate_source"]["retention_days"] == 4
    assert "retention_days" in document["candidate_source"]["note"]


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

    assert main(["--agent", "agent", "run", "git status"]) == 1
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

    assert main(["--json", "--agent", "agent", "run", "git status"]) == 23
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
def test_net_doctor_pane_failures_keep_its_output_exit_and_audit_shape(
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

    def direct_probe(
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
        direct_probe,
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

    assert main(["net-doctor"]) == 1
    captured = capsys.readouterr()
    assert f"[via pane] FAILED: {error}" in captured.out
    assert "VERDICT: the pane path is NOT working." in captured.out
    assert "Traceback" not in captured.err
    # The scope disclaimer leads, so a reader whose interest is something else can stop at line
    # one instead of reading a verdict about a scenario they are not in.
    assert captured.out.startswith("herdr-run net-doctor  (agent=")
    assert "This is a smoke test for ONE scenario" in captured.out
    assert captured.out.index("ONE scenario") < captured.out.index("[direct  ]")
    assert "[in-jail ]" not in captured.out

    with open(audit_path(root, ".herdr-run"), encoding="utf-8") as handle:
        entries = [json.loads(line) for line in handle]
    assert [entry["verdict"] for entry in entries] == ["ADMITTED", verdict]
    assert entries[-1]["net_doctor"] is True
    assert entries[-1]["direct_exit_code"] == 1


def test_net_doctor_pre_admission_refusal_is_marked_as_net_doctor(
    tmp_path: object,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    root = str(tmp_path)
    monkeypatch.chdir(root)
    monkeypatch.setattr(
        cli_module, "admit", lambda *_args: _raise(Refused("probe refused"))
    )

    assert main(["net-doctor"]) == EXIT_REFUSED
    assert "herdr-run: probe refused" in capsys.readouterr().err
    with open(audit_path(root, ".herdr-run"), encoding="utf-8") as handle:
        entry = json.loads(handle.read())
    assert entry["verdict"] == "REFUSED"
    assert entry["net_doctor"] is True


def test_vanished_caller_cwd_is_a_typed_config_error(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    """Model getcwd(3)'s ENOENT after another process removes the caller's directory."""
    monkeypatch.setattr(
        os,
        "getcwd",
        lambda: _raise(FileNotFoundError("caller directory was removed")),
    )

    assert main(["--agent", "agent", "run", "git status"]) == EXIT_CONFIG
    captured = capsys.readouterr()
    assert "cannot determine current directory" in captured.err
    assert "Traceback" not in captured.err


def test_a_configuration_failure_after_admission_is_audited(
    tmp_path: object,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """A run that dies AFTER the policy said yes must still leave both halves in the log.

    An audit that recorded only the admission would say a command was authorised and never say
    what became of it, which is the one thing the log exists to answer.
    """
    root = str(tmp_path)
    config = Config(project_root=root)
    monkeypatch.setattr(cli_module, "load_config", lambda **_kwargs: config)
    monkeypatch.setattr(cli_module, "_client", lambda *_args: object())
    monkeypatch.setattr(
        cli_module,
        "_resolved_cwd",
        lambda *_args: _raise(ConfigError("caller directory was removed")),
    )

    assert main(["--agent", "agent", "run", "git status"]) == EXIT_CONFIG
    captured = capsys.readouterr()
    assert "caller directory was removed" in captured.err
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
    main(["--agent", "agent", "run", "curl https://evil.example"])

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


def test_run_options_are_accepted_after_the_subcommand(
    tmp_path: object,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """Every `run` option belongs after `run`, in any order, including attached-value forms."""
    monkeypatch.chdir(str(tmp_path))
    assert (
        main(
            [
                "--agent",
                "agent",
                "run",
                "--cwd",
                "/tmp",
                "--timeout=12.5",
                "--wait-ready",
                "3",
                "--no-cache",
                "--dry-run",
                "git status",
            ]
        )
        == 0
    )
    assert capsys.readouterr().out == "git status\n"


def test_loose_argv_words_are_refused_not_silently_rejoined(
    tmp_path: object, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    """Re-joining loose words would silently destroy quoting; refuse instead.

    `herdr-run run git commit -m "two words"` would otherwise arrive at git as `-m two words`,
    and nothing in the output would reveal that the caller's quoting had been discarded.
    """
    monkeypatch.chdir(str(tmp_path))
    assert main(["run", "git", "status", "--", "path with spaces"]) == 2
    err = capsys.readouterr().err
    assert "ONE quoted argument" in err
    assert "silently change the quoting" in err


def test_loose_argv_containing_an_unknown_flag_is_also_refused(
    tmp_path: object, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    """The other loose shape: the flag is refused before the command is ever assembled.

    Recorded because the two shapes fail through DIFFERENT paths and both must be non-silent:
    `-m` is not a `run` option, so parsing stops there.
    """
    monkeypatch.chdir(str(tmp_path))
    assert main(["run", "git", "commit", "-m", "two words"]) == 2
    assert "run: unrecognized arguments: -m" in capsys.readouterr().err


def test_the_documented_form_still_works(
    tmp_path: object, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    """Positive control: a global agent plus one quoted command is the shape, and must not regress."""
    monkeypatch.chdir(str(tmp_path))
    assert (
        main(
            [
                "--agent",
                "someagent",
                "run",
                "--dry-run",
                "with-proxy git ls-remote origin main",
            ]
        )
        == 0
    )
    assert "with-proxy git ls-remote origin main" in capsys.readouterr().out


def test_a_leading_wrapper_is_never_swallowed_as_an_agent_name(
    tmp_path: object, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    """`run with-proxy '<cmd>'` must NOT drop `with-proxy`.

    The old surface did: with two positionals the leading one was discarded from the command even
    when --agent had already supplied the name. The command still ran, minus its proxy wrapper, so
    `gh` dialled GitHub directly and reported "network is unreachable" -- indistinguishable from a
    real egress outage, and the reason a whole fleet concluded `gh` was unusable through the pane.
    Now nothing before the command can be read as an agent name at all, and two positionals are
    refused with the wrapper put back INSIDE the quotes.
    """
    monkeypatch.chdir(str(tmp_path))
    assert (
        main(
            [
                "--agent",
                "someagent",
                "run",
                "--dry-run",
                "with-proxy",
                "git ls-remote origin",
            ]
        )
        == 2
    )
    err = capsys.readouterr().err
    assert "ONE quoted argument" in err
    assert "'with-proxy git ls-remote origin'" in err


def test_command_runs_in_the_callers_cwd_not_the_config_directory(
    tmp_path: object, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Regression: a config file ABOVE the caller must not relocate where the command runs.

    Config discovery walks up to the nearest .herdr-run.yaml, so a file at the top of a multi-repo
    tree used to make every nested worktree run its command in that top directory instead of the
    one the caller was standing in -- silent mistargeting that reads as the wrong repo answering.
    """
    import os

    from herdr_run.cli import _resolved_cwd
    from herdr_run.config import Config

    root = str(tmp_path)
    nested = os.path.join(root, "worktrees", "slot", "repo")
    os.makedirs(nested)
    config = Config(project_root=root)  # config lives at the top...
    monkeypatch.chdir(nested)  # ...caller stands in the nested worktree

    assert _resolved_cwd(config, None) == os.path.realpath(nested) or (
        _resolved_cwd(config, None) == nested
    )


def test_explicit_cwd_still_wins(tmp_path: object) -> None:
    """Positive control: --cwd is still honoured over both the config and the caller's cwd."""
    from herdr_run.cli import _resolved_cwd
    from herdr_run.config import Config

    target = str(tmp_path)
    assert _resolved_cwd(Config(project_root="/elsewhere"), target) == target


def test_explicit_empty_cwd_means_the_project_root() -> None:
    """An explicitly supplied empty value is not the same as omitting --cwd."""
    from herdr_run.cli import _resolved_cwd
    from herdr_run.config import Config

    assert _resolved_cwd(Config(project_root="/project", cwd="configured"), "") == "/project"
