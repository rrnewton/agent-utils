"""Execute an admitted command in a pane and collect its real stdout, stderr, and exit code.

**Results are read from FILES, never scraped from the terminal.** The filesystem is shared across
the sandbox boundary, so the pane can redirect into a spool directory this process reads directly.
Scraping would inherit every terminal problem at once — an 80-column pane hard-wraps output
mid-token, the scrollback is finite so long output is silently truncated, ANSI and progress bars
corrupt the text, and there is no exit code anywhere on screen. The wrapper is::

    { cd <cwd> && <command> ; } >stdout 2>stderr; printf '%s\\n' "$?" >exit_code

so ``exit_code`` appearing is both the completion signal and the result, and a failed ``cd`` is
reported as a failure of the run rather than silently running in the wrong directory.

Because ``exit_code`` is written by a separate command after the redirection closes, its EXISTENCE
is what marks completion — but a file can exist while still being written, so a read that does not
parse as an integer is treated as an incomplete write rather than as a corrupt result.
"""

from __future__ import annotations

import json
import os
import time
from collections.abc import Callable
from dataclasses import dataclass

from herdr_run.allowlist import Admission
from herdr_run.client import HerdrClient
from herdr_run.config import Config
from herdr_run.errors import PaneBusy, RunTimeout
from herdr_run.readiness import Readiness, assess, infer_prompt_tail
from herdr_run.retention import prune_runs, runs_root
from herdr_run.session import Target

__all__ = ["RunResult", "execute", "wait_ready", "spool_paths"]


@dataclass(frozen=True)
class SpoolPaths:
    """Where one run's captured output and exit code live on disk."""

    directory: str
    stdout: str
    stderr: str
    exit_code: str


@dataclass(frozen=True)
class RunResult:
    """A completed run: the command's own result plus the evidence it was allowed to run."""

    exit_code: int
    stdout: str
    stderr: str
    run_id: str
    spool: SpoolPaths
    target: Target
    readiness: Readiness
    duration_seconds: float


def spool_paths(config: Config, run_id: str) -> SpoolPaths:
    """Resolve the spool file paths for one run id."""
    root = config.spool_dir
    if not os.path.isabs(root):
        root = os.path.join(config.project_root, root)
    directory = os.path.join(root, "runs", run_id)
    return SpoolPaths(
        directory=directory,
        stdout=os.path.join(directory, "stdout"),
        stderr=os.path.join(directory, "stderr"),
        exit_code=os.path.join(directory, "exit_code"),
    )


def make_run_id(agent: str, *, now: float, pid: int) -> str:
    """A per-run identifier that is unique per (time, process) and readable in a directory listing."""
    stamp = time.strftime("%Y%m%dT%H%M%S", time.gmtime(now))
    safe_agent = "".join(char if char.isalnum() or char in "-_" else "-" for char in agent) or "agent"
    return f"{stamp}-{safe_agent}-{pid}"


def wait_ready(
    client: HerdrClient,
    config: Config,
    target: Target,
    *,
    prompt_tail: str | None,
    timeout: float,
    sleep: Callable[[float], None] = time.sleep,
    monotonic: Callable[[], float] = time.monotonic,
    poll_interval: float = 0.25,
    required_consecutive: int = 2,
) -> Readiness:
    """Poll until the pane is ready, or raise :class:`PaneBusy`.

    ``required_consecutive`` readings are demanded because ``herdr pane run`` submits a line
    asynchronously: for a few milliseconds after a previous command is submitted the kernel has not
    yet moved the foreground process group, so a single sample can catch a busy pane looking idle.
    """
    deadline = monotonic() + max(timeout, 0.0)
    consecutive = 0
    readings = 0
    last: Readiness | None = None
    while True:
        last = assess(client, target.pane_id, config, prompt_tail=prompt_tail)
        readings += 1
        if last.ready:
            consecutive += 1
            if consecutive >= required_consecutive:
                return last
        else:
            consecutive = 0
        # The deadline governs how long we WAIT for a busy pane, but it must never cut the
        # confirmation short: with the default `ready_timeout_seconds: 0` a single reading would
        # otherwise be taken and then rejected for not being two, so an idle pane could never be
        # declared ready. Always allow at least `required_consecutive` readings.
        if readings >= required_consecutive and monotonic() >= deadline:
            break
        sleep(poll_interval)

    assert last is not None  # the loop always takes at least one reading
    if last.ready:
        # Ran out of time while the pane looked ready but had not yet been ready long enough.
        raise PaneBusy(
            f"pane {target.pane_id} ({target.tab_label}) looked idle but not for "
            f"{required_consecutive} consecutive checks within {timeout:g}s: {last.describe()}"
        )
    raise PaneBusy(
        f"pane {target.pane_id} ({target.tab_label}) is not ready after {timeout:g}s: {last.describe()}. "
        "Nothing was executed."
    )


def build_shell_command(admission: Admission, spool: SpoolPaths, cwd: str) -> str:
    """Render the exact line typed into the pane.

    Every interpolated value is shell-quoted: ``admission.rendered`` quotes each argv token, and the
    paths are quoted here. Nothing in this string is attacker-chosen unquoted text.
    """
    import shlex

    return (
        f"{{ cd {shlex.quote(cwd)} && {admission.rendered} ; }} "
        f">{shlex.quote(spool.stdout)} 2>{shlex.quote(spool.stderr)}; "
        f"printf '%s\\n' \"$?\" >{shlex.quote(spool.exit_code)}"
    )


def _read_exit_code(path: str) -> int | None:
    """Return the recorded exit code, or ``None`` while it is absent or incompletely written."""
    try:
        with open(path, encoding="utf-8") as handle:
            text = handle.read()
    except OSError:
        return None
    stripped = text.strip()
    if not stripped:
        return None
    try:
        return int(stripped)
    except ValueError:
        return None


def _read_text(path: str) -> str:
    try:
        with open(path, encoding="utf-8", errors="replace") as handle:
            return handle.read()
    except OSError:
        return ""


def execute(
    client: HerdrClient,
    config: Config,
    target: Target,
    admission: Admission,
    *,
    agent: str,
    cwd: str,
    ready_timeout: float,
    timeout: float,
    now: Callable[[], float] = time.time,
    monotonic: Callable[[], float] = time.monotonic,
    sleep: Callable[[float], None] = time.sleep,
    poll_interval: float = 0.25,
    home: str | None = None,
) -> RunResult:
    """Run ``admission`` in ``target``'s pane and return its real result."""
    prompt_tail = infer_prompt_tail(config, home=home)
    readiness = wait_ready(
        client,
        config,
        target,
        prompt_tail=prompt_tail,
        timeout=ready_timeout,
        sleep=sleep,
        monotonic=monotonic,
        poll_interval=poll_interval,
    )

    run_id = make_run_id(agent, now=now(), pid=os.getpid())
    spool = spool_paths(config, run_id)
    os.makedirs(spool.directory, exist_ok=True)
    # Retention is applied HERE, as a side effect of writing a new run, so it cannot silently stop
    # the way a cron or timer can. It never raises; see herdr_run.retention.
    prune_runs(runs_root(config.spool_dir, config.project_root), retention_days=config.retention_days)
    # Pre-create the output files so a reader never has to distinguish "not created yet" from
    # "created and empty"; the exit-code file is deliberately NOT pre-created, since its appearance
    # is the completion signal.
    for path in (spool.stdout, spool.stderr):
        with open(path, "w", encoding="utf-8"):
            pass
    with open(os.path.join(spool.directory, "command"), "w", encoding="utf-8") as handle:
        handle.write(admission.rendered + "\n")

    started = monotonic()
    client.run(target.pane_id, build_shell_command(admission, spool, cwd))

    deadline = started + max(timeout, 0.0)
    while True:
        code = _read_exit_code(spool.exit_code)
        if code is not None:
            return RunResult(
                exit_code=code,
                stdout=_read_text(spool.stdout),
                stderr=_read_text(spool.stderr),
                run_id=run_id,
                spool=spool,
                target=target,
                readiness=readiness,
                duration_seconds=monotonic() - started,
            )
        if monotonic() >= deadline:
            break
        sleep(poll_interval)

    # Carry the partial output OUT with the failure. Raising a bare message would leave whatever
    # the command already printed visible only on disk, so a caller scraping stdout would read a
    # timed-out run as having produced nothing -- a false empty result, which is exactly the class
    # of mistake a landing decision must not be built on.
    timed_out = RunTimeout(
        f"command did not finish within {timeout:g}s. It is STILL RUNNING in pane "
        f"{target.pane_id} ({target.tab_label}) and was not killed. Partial output is in "
        f"{spool.directory}; the exit code will appear in {spool.exit_code} when it finishes."
    )
    timed_out.partial_stdout = _read_text(spool.stdout)
    timed_out.partial_stderr = _read_text(spool.stderr)
    timed_out.spool_directory = spool.directory
    raise timed_out


def write_meta(result: RunResult, admission: Admission, config: Config, agent: str) -> str:
    """Record what ran, where, and on what evidence it was allowed to. Returns the file path."""
    path = os.path.join(result.spool.directory, "meta.json")
    document = {
        "run_id": result.run_id,
        "agent": agent,
        "argv": list(admission.argv),
        "program": admission.program,
        "prefix": list(admission.prefix),
        "subcommand": admission.subcommand,
        "rendered": admission.rendered,
        "exit_code": result.exit_code,
        "duration_seconds": round(result.duration_seconds, 3),
        "workspace": {"label": result.target.workspace_label, "id": result.target.workspace_id},
        "tab": {"label": result.target.tab_label, "id": result.target.tab_id},
        "pane_id": result.target.pane_id,
        "created": list(result.target.created),
        "from_cache": result.target.from_cache,
        "readiness": {
            "process_idle": result.readiness.process.idle,
            "process_reason": result.readiness.process.reason,
            "shell_pid": result.readiness.process.shell_pid,
            "foreground_pgid": result.readiness.process.foreground_pgid,
            "prompt_verdict": result.readiness.prompt.verdict,
            "prompt_reason": result.readiness.prompt.reason,
            "prompt_tail": result.readiness.prompt.tail,
        },
        "config_source": config.source_path,
    }
    with open(path, "w", encoding="utf-8") as handle:
        json.dump(document, handle, indent=2, sort_keys=True)
        handle.write("\n")
    return path
