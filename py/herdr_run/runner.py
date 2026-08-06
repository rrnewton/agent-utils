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
import fcntl
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from dataclasses import dataclass

from herdr_run.allowlist import Admission, reject_terminal_controls
from herdr_run.client import HerdrClient
from herdr_run.config import Config
from herdr_run.errors import HerdrUnavailable, PaneBusy, RunTimeout
from herdr_run.readiness import Readiness, assess, infer_prompt_tail
from herdr_run.retention import prune_runs, runs_root
from herdr_run.session import Target
from herdr_run.state import open_lock_file, pane_lock_path

__all__ = ["RunResult", "execute", "wait_ready", "spool_paths", "read_output_bytes"]


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
    root = _spool_root(config)
    directory = os.path.join(root, "runs", run_id)
    return SpoolPaths(
        directory=directory,
        stdout=os.path.join(directory, "stdout"),
        stderr=os.path.join(directory, "stderr"),
        exit_code=os.path.join(directory, "exit_code"),
    )


def _spool_root(config: Config) -> str:
    root = config.spool_dir
    if not os.path.isabs(root):
        root = os.path.join(config.project_root, root)
    return root


def _ensure_private_directory(path: str) -> None:
    os.makedirs(path, mode=0o700, exist_ok=True)
    os.chmod(path, 0o700)


def _create_private_file(path: str, contents: bytes = b"") -> None:
    flags = (
        os.O_CREAT
        | os.O_TRUNC
        | os.O_WRONLY
        | getattr(os, "O_CLOEXEC", 0)
        | getattr(os, "O_NOFOLLOW", 0)
    )
    descriptor = os.open(path, flags, 0o600)
    try:
        os.fchmod(descriptor, 0o600)
        remaining = memoryview(contents)
        while remaining:
            written = os.write(descriptor, remaining)
            if written == 0:
                raise OSError(f"short write while creating {path}")
            remaining = remaining[written:]
    finally:
        os.close(descriptor)


def make_run_id(agent: str, *, now: float, pid: int) -> str:
    """A per-run identifier that is unique per (time, process) and readable in a directory listing."""
    stamp = time.strftime("%Y%m%dT%H%M%S", time.gmtime(now))
    safe_agent = "".join(char if char.isalnum() or char in "-_" else "-" for char in agent) or "agent"
    return f"{stamp}-{safe_agent}-{pid}"


def _allocate_spool(config: Config, base_run_id: str) -> tuple[str, SpoolPaths]:
    """Create a never-reused run directory, adding a bounded suffix on collision.

    A timestamp-to-the-second plus PID is readable but not unique across repeated library calls or
    PID reuse. Reusing a directory could consume its old ``exit_code`` before the new command had
    even started. Atomic directory creation makes the filesystem arbitrate uniqueness.
    """
    root = _spool_root(config)
    runs = os.path.join(root, "runs")
    try:
        _ensure_private_directory(root)
        _ensure_private_directory(runs)
    except OSError as exc:
        raise HerdrUnavailable(f"cannot create private run spool parent {runs}: {exc}") from exc
    for collision in range(10_000):
        run_id = base_run_id if collision == 0 else f"{base_run_id}-{collision}"
        spool = spool_paths(config, run_id)
        try:
            os.mkdir(spool.directory, mode=0o700)
            os.chmod(spool.directory, 0o700)
        except FileExistsError:
            continue
        except OSError as exc:
            raise HerdrUnavailable(f"cannot create run spool {spool.directory}: {exc}") from exc
        return run_id, spool
    raise HerdrUnavailable(
        f"could not allocate a unique run spool after 10000 collisions for {base_run_id}"
    )


@contextmanager
def _pane_lock(
    target: Target,
    *,
    timeout: float,
    monotonic: Callable[[], float],
    sleep: Callable[[float], None],
    poll_interval: float,
) -> Iterator[None]:
    """Hold the per-pane launch/result lock, bounded by the readiness wait budget."""
    path = pane_lock_path(target.pane_id)
    handle = open_lock_file(path)
    deadline = monotonic() + max(timeout, 0.0)
    try:
        while True:
            try:
                fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except BlockingIOError:
                if monotonic() >= deadline:
                    raise PaneBusy(
                        f"pane {target.pane_id} ({target.tab_label}) is already reserved by "
                        "another herdr-run. Nothing was executed."
                    )
                sleep(poll_interval)
            except OSError as exc:
                raise HerdrUnavailable(f"cannot lock pane {target.pane_id}: {exc}") from exc
        yield
    finally:
        # Closing releases `flock` even when readiness, launch, or collection raised.
        handle.close()


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

    for description, value in (
        ("working directory", cwd),
        ("stdout spool path", spool.stdout),
        ("stderr spool path", spool.stderr),
        ("exit-code spool path", spool.exit_code),
    ):
        try:
            reject_terminal_controls(value, description)
        except Exception as exc:
            raise HerdrUnavailable(str(exc)) from exc

    inner = (
        f"umask 077; {{ cd {shlex.quote(cwd)} && {admission.rendered} ; }} "
        f">{shlex.quote(spool.stdout)} 2>{shlex.quote(spool.stderr)}; "
        f"printf '%s\\n' \"$?\" >{shlex.quote(spool.exit_code)}"
    )
    # The target may be fish or another non-POSIX interactive shell. Keep the text typed into that
    # shell to a portable command invocation and run the actual wrapper under sh.
    return f"command sh -c {shlex.quote(inner)}"


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
        with open(path, "rb") as handle:
            return handle.read().decode("utf-8", errors="replace")
    except OSError:
        return ""


def read_output_bytes(result: RunResult) -> tuple[bytes, bytes]:
    """Read captured stdout/stderr without decoding, for byte-preserving CLI passthrough."""
    output: list[bytes] = []
    for path in (result.spool.stdout, result.spool.stderr):
        try:
            with open(path, "rb") as handle:
                output.append(handle.read())
        except OSError:
            output.append(b"")
    return output[0], output[1]


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
    lock_started = monotonic()
    with _pane_lock(
        target,
        timeout=ready_timeout,
        monotonic=monotonic,
        sleep=sleep,
        poll_interval=poll_interval,
    ):
        elapsed_for_lock = max(0.0, monotonic() - lock_started)
        remaining_ready = max(0.0, ready_timeout - elapsed_for_lock)
        prompt_tail = infer_prompt_tail(config, home=home)
        readiness = wait_ready(
            client,
            config,
            target,
            prompt_tail=prompt_tail,
            timeout=remaining_ready,
            sleep=sleep,
            monotonic=monotonic,
            poll_interval=poll_interval,
        )

        base_run_id = make_run_id(agent, now=now(), pid=os.getpid())
        run_id, spool = _allocate_spool(config, base_run_id)
        # Retention is applied as a side effect of writing a new run, so it cannot silently stop
        # the way a cron or timer can. It never raises; see :mod:`herdr_run.retention`.
        prune_runs(
            runs_root(config.spool_dir, config.project_root),
            retention_days=config.retention_days,
        )
        # Pre-create output files; exit_code deliberately remains absent as the completion signal.
        try:
            for path in (spool.stdout, spool.stderr):
                _create_private_file(path)
            _create_private_file(
                os.path.join(spool.directory, "command"),
                (admission.rendered + "\n").encode("utf-8"),
            )
        except OSError as exc:
            raise HerdrUnavailable(f"cannot initialize run spool {spool.directory}: {exc}") from exc

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

        # Carry partial output with the typed failure. A bare message would make a timed-out run
        # look falsely empty to a caller that does not inspect the spool directory.
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
    encoded = (json.dumps(document, indent=2, sort_keys=True, ensure_ascii=False) + "\n").encode(
        "utf-8"
    )
    _create_private_file(path, encoded)
    return path
