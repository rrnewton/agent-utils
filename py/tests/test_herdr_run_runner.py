"""Execution and result-collection tests.

The fake client runs the generated shell line through a REAL bash, so these are not string
assertions about what we think the shell would do — the quoting, redirection, and exit-code capture
are actually executed.
"""

from __future__ import annotations

import os
from typing import cast

import pytest

from herdr_run.allowlist import admit
from herdr_run.client import HerdrClient
from herdr_run.config import Config
from herdr_run.errors import PaneBusy, RunTimeout
from herdr_run.runner import build_shell_command, execute, spool_paths, wait_ready
from herdr_run.session import resolve_target
from herdr_run.fakeherdr import FakeHerdrClient


def _config(root: str, **kwargs: object) -> Config:
    base: dict[str, object] = {"project_root": root, "spool_dir": "spool", "allow": ("git", "echo", "false", "sleep", "cat")}
    base.update(kwargs)
    return Config(**base)  # type: ignore[arg-type]


def _client(fake: FakeHerdrClient) -> HerdrClient:
    return cast(HerdrClient, fake)


def _run(fake: FakeHerdrClient, config: Config, command: str, **kwargs: object) -> object:
    target = resolve_target(_client(fake), config, "agent")
    return execute(
        _client(fake),
        config,
        target,
        admit(command, config),
        agent="agent",
        cwd=config.project_root,
        ready_timeout=0.0,
        timeout=float(cast(float, kwargs.pop("timeout", 30.0))),
        poll_interval=0.01,
        home="/nonexistent",  # force the prompt signal to abstain; process signal decides
        **kwargs,  # type: ignore[arg-type]
    )


# --- result capture ------------------------------------------------------------------------------


def test_captures_stdout_and_zero_exit_code(tmp_path: object) -> None:
    root = str(tmp_path)
    result = _run(FakeHerdrClient(), _config(root), "echo hello-from-pane")
    assert result.exit_code == 0  # type: ignore[attr-defined]
    assert result.stdout.strip() == "hello-from-pane"  # type: ignore[attr-defined]


def test_captures_nonzero_exit_code(tmp_path: object) -> None:
    """A failing command is a RESULT, not a herdr-run error: the real exit code comes back."""
    result = _run(FakeHerdrClient(), _config(str(tmp_path)), "false")
    assert result.exit_code == 1  # type: ignore[attr-defined]


def test_captures_stderr_separately(tmp_path: object) -> None:
    root = str(tmp_path)
    config = _config(root, allow=("bash",))
    fake = FakeHerdrClient()
    target = resolve_target(_client(fake), config, "agent")
    result = execute(
        _client(fake),
        config,
        target,
        admit("bash -c 'echo OUT; echo ERR >&2'", config),
        agent="agent",
        cwd=root,
        ready_timeout=0.0,
        timeout=30.0,
        poll_interval=0.01,
        home="/nonexistent",
    )
    assert result.stdout.strip() == "OUT"
    assert result.stderr.strip() == "ERR"


def test_arguments_with_spaces_and_metacharacters_survive_intact(tmp_path: object) -> None:
    """End-to-end proof that re-quoting holds through a real shell.

    ``; rm -rf /`` must arrive as ONE literal argument, not as a second command.
    """
    root = str(tmp_path)
    config = _config(root, allow=("printf",))
    fake = FakeHerdrClient()
    target = resolve_target(_client(fake), config, "agent")
    result = execute(
        _client(fake),
        config,
        target,
        admit("printf '[%s]' 'two words' '; rm -rf /' '$(id)' '`id`'", config),
        agent="agent",
        cwd=root,
        ready_timeout=0.0,
        timeout=30.0,
        poll_interval=0.01,
        home="/nonexistent",
    )
    assert result.stdout == "[two words][; rm -rf /][$(id)][`id`]"


def test_runs_in_the_requested_directory(tmp_path: object) -> None:
    root = str(tmp_path)
    workdir = os.path.join(root, "sub dir with spaces")
    os.makedirs(workdir)
    config = _config(root, allow=("pwd",))
    fake = FakeHerdrClient()
    target = resolve_target(_client(fake), config, "agent")
    result = execute(
        _client(fake),
        config,
        target,
        admit("pwd", config),
        agent="agent",
        cwd=workdir,
        ready_timeout=0.0,
        timeout=30.0,
        poll_interval=0.01,
        home="/nonexistent",
    )
    assert result.stdout.strip() == workdir


def test_failed_cd_is_reported_as_a_failure(tmp_path: object) -> None:
    """A missing working directory must fail the run, not silently run somewhere else."""
    root = str(tmp_path)
    config = _config(root, allow=("pwd",))
    fake = FakeHerdrClient()
    target = resolve_target(_client(fake), config, "agent")
    result = execute(
        _client(fake),
        config,
        target,
        admit("pwd", config),
        agent="agent",
        cwd=os.path.join(root, "does-not-exist"),
        ready_timeout=0.0,
        timeout=30.0,
        poll_interval=0.01,
        home="/nonexistent",
    )
    assert result.exit_code != 0


def test_spool_files_are_written(tmp_path: object) -> None:
    root = str(tmp_path)
    result = _run(FakeHerdrClient(), _config(root), "echo spooled")
    spool = spool_paths(_config(root), result.run_id)  # type: ignore[attr-defined]
    for path in (spool.stdout, spool.stderr, spool.exit_code, os.path.join(spool.directory, "command")):
        assert os.path.isfile(path), path


# --- the shell wrapper ------------------------------------------------------------------------------


def test_shell_wrapper_quotes_every_interpolated_path() -> None:
    config = Config(allow=("git",))
    spool = spool_paths(Config(project_root="/tmp/root x", spool_dir="sp ool"), "run 1")
    line = build_shell_command(admit("git status", config), spool, "/tmp/work dir")
    # Every path with a space must be quoted; an unquoted one would split into extra words.
    assert "'/tmp/work dir'" in line
    assert "'" + spool.stdout + "'" in line


# --- readiness gating ---------------------------------------------------------------------------------


def test_refuses_to_run_in_a_busy_pane(tmp_path: object) -> None:
    root = str(tmp_path)
    config = _config(root)
    fake = FakeHerdrClient()
    target = resolve_target(_client(fake), config, "agent")
    fake.busy_panes.add(target.pane_id)

    with pytest.raises(PaneBusy, match="not ready"):
        execute(
            _client(fake),
            config,
            target,
            admit("echo nope", config),
            agent="agent",
            cwd=root,
            ready_timeout=0.0,
            timeout=5.0,
            poll_interval=0.01,
            home="/nonexistent",
        )
    # The decisive assertion: NOTHING was typed into the pane.
    assert fake.commands == []


def test_refuses_when_the_prompt_line_is_dirty(tmp_path: object) -> None:
    """Process signal says idle; the prompt veto must still stop us."""
    root = str(tmp_path)
    config = _config(root, prompt_tail="$ ")
    fake = FakeHerdrClient(pane_text="[user@host ~]\n$ half-typed-by-a-human\n")
    target = resolve_target(_client(fake), config, "agent")

    with pytest.raises(PaneBusy, match="not ready"):
        execute(
            _client(fake),
            config,
            target,
            admit("echo nope", config),
            agent="agent",
            cwd=root,
            ready_timeout=0.0,
            timeout=5.0,
            poll_interval=0.01,
        )
    assert fake.commands == []


def test_readiness_process_mode_ignores_the_prompt_veto(tmp_path: object) -> None:
    """Positive half of the previous test: the veto is configurable, and dropping it lets it run."""
    root = str(tmp_path)
    config = _config(root, prompt_tail="$ ", readiness="process")
    fake = FakeHerdrClient(pane_text="[user@host ~]\n$ half-typed-by-a-human\n")
    result = _run(fake, config, "echo ran-anyway")
    assert result.exit_code == 0  # type: ignore[attr-defined]


def test_default_zero_ready_timeout_still_confirms_twice(tmp_path: object) -> None:
    """Regression: with ready_timeout 0 the two-consecutive-reading rule must still be satisfiable.

    An earlier version broke out of the poll loop on the deadline before a second reading could be
    taken, so an idle pane could never be declared ready under the default config.
    """
    fake = FakeHerdrClient()
    config = _config(str(tmp_path))
    target = resolve_target(_client(fake), config, "agent")
    readiness = wait_ready(
        _client(fake), config, target, prompt_tail=None, timeout=0.0, poll_interval=0.0
    )
    assert readiness.ready is True


def test_waits_for_a_pane_that_becomes_free(tmp_path: object) -> None:
    root = str(tmp_path)
    config = _config(root)
    fake = FakeHerdrClient()
    target = resolve_target(_client(fake), config, "agent")
    fake.busy_panes.add(target.pane_id)

    calls = {"n": 0}

    def fake_sleep(_seconds: float) -> None:
        calls["n"] += 1
        if calls["n"] >= 3:
            fake.busy_panes.discard(target.pane_id)

    readiness = wait_ready(
        _client(fake), config, target, prompt_tail=None, timeout=10.0, sleep=fake_sleep, poll_interval=0.0
    )
    assert readiness.ready is True


# --- timeout -------------------------------------------------------------------------------------------


def test_timeout_reports_that_the_command_is_still_running(tmp_path: object) -> None:
    root = str(tmp_path)
    config = _config(root)
    # execute_locally=False: the line is never run, so the exit-code file never appears.
    fake = FakeHerdrClient(execute_locally=False)
    target = resolve_target(_client(fake), config, "agent")

    with pytest.raises(RunTimeout, match="STILL RUNNING"):
        execute(
            _client(fake),
            config,
            target,
            admit("echo never-completes", config),
            agent="agent",
            cwd=root,
            ready_timeout=0.0,
            timeout=0.05,
            poll_interval=0.01,
            home="/nonexistent",
        )
    # It WAS launched — the timeout is about collection, not about admission.
    assert len(fake.commands) == 1


def test_partially_written_exit_code_is_not_read_as_a_result(tmp_path: object) -> None:
    """A non-integer exit-code file means "still being written", never a corrupt result."""
    root = str(tmp_path)
    config = _config(root)
    fake = FakeHerdrClient(execute_locally=False)
    target = resolve_target(_client(fake), config, "agent")

    original_run = fake.run

    def run_and_write_garbage(pane_id: str, command: str) -> None:
        original_run(pane_id, command)
        run_id = os.listdir(os.path.join(root, "spool", "runs"))[0]
        with open(os.path.join(root, "spool", "runs", run_id, "exit_code"), "w", encoding="utf-8") as handle:
            handle.write("")  # created but empty, as during a torn write

    fake.run = run_and_write_garbage  # type: ignore[method-assign]

    with pytest.raises(RunTimeout):
        execute(
            _client(fake),
            config,
            target,
            admit("echo x", config),
            agent="agent",
            cwd=root,
            ready_timeout=0.0,
            timeout=0.05,
            poll_interval=0.01,
            home="/nonexistent",
        )


# --- the timeout path must not swallow what the command already printed --------------------------


def test_timeout_carries_the_partial_output(tmp_path: object) -> None:
    """A timed-out run that printed something must not look like one that printed nothing."""
    root = str(tmp_path)
    config = _config(root)
    fake = FakeHerdrClient(execute_locally=False)
    target = resolve_target(_client(fake), config, "agent")

    original_run = fake.run

    def run_and_emit_partial(pane_id: str, command: str) -> None:
        original_run(pane_id, command)
        run_id = os.listdir(os.path.join(root, "spool", "runs"))[0]
        base = os.path.join(root, "spool", "runs", run_id)
        with open(os.path.join(base, "stdout"), "w", encoding="utf-8") as handle:
            handle.write("PARTIAL-STDOUT\n")
        with open(os.path.join(base, "stderr"), "w", encoding="utf-8") as handle:
            handle.write("PARTIAL-STDERR\n")

    fake.run = run_and_emit_partial  # type: ignore[method-assign]

    with pytest.raises(RunTimeout) as excinfo:
        execute(
            _client(fake), config, target, admit("echo x", config), agent="agent", cwd=root,
            ready_timeout=0.0, timeout=0.05, poll_interval=0.01, home="/nonexistent",
        )
    assert excinfo.value.partial_stdout == "PARTIAL-STDOUT\n"
    assert excinfo.value.partial_stderr == "PARTIAL-STDERR\n"


def test_retention_runs_on_write(tmp_path: object) -> None:
    """Prune-on-write: an old run directory is gone after the next run, with no timer involved."""
    import time as _time

    root = str(tmp_path)
    config = _config(root)
    runs = os.path.join(root, "spool", "runs")
    os.makedirs(runs)
    stale = os.path.join(runs, "20260101T000000-old-1")
    os.makedirs(stale)
    old = _time.time() - 9 * 86400
    os.utime(stale, (old, old))

    result = _run(FakeHerdrClient(), config, "echo fresh")

    assert result.exit_code == 0  # type: ignore[attr-defined]
    assert not os.path.exists(stale), "writing a new run must prune the stale one"
    assert os.path.isdir(result.spool.directory)  # type: ignore[attr-defined]
