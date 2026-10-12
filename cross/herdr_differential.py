#!/usr/bin/env python3
"""Black-box Python/Rust differential for :command:`herdr-run`.

The commands under test are supplied by the caller as complete argv vectors.  This keeps the
harness independent of both packaging layouts and, importantly, never invokes a shell to locate or
compose either implementation.  The comparison covers bootstrap behavior, every help screen and
document, the structured option surface, the command-policy boundary, strict configuration
loading/discovery with every configuration message, argument grammar, agent inference, bytes that
are not UTF-8, locales, output failures and signals, OS error text, the audited dry-run path, the
spool, reap over planted records, and the documented launchers.

Output is compared as BYTES: both streams are decoded with ``surrogateescape``, so a byte that is
not UTF-8 survives the comparison instead of being replaced by U+FFFD in both editions alike.

Real pane execution is deliberately absent.  Both implementations resolve ``herdr`` only from
trusted installation locations and ignore caller-controlled ``PATH``.  There is consequently no
safe black-box way to inject a fake server without either weakening that production contract or
overwriting a user's real installation.  The Herdr protocol, readiness, the session cache, and run
records are instead pinned by the golden corpora under ``rs/herdr-run/testdata/``, which both
editions' unit tests load.  The report calls this out explicitly.

Only values which are intrinsically different between two processes are normalized: temporary
roots, run IDs, timestamps, process IDs, and measured durations.  Policy text, quoting, exit codes,
JSON formatting, and diagnostics otherwise remain observable.
"""

from __future__ import annotations

import argparse
import errno
import fcntl
import json
import os
import re
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import termios
import time
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path
from typing import cast

REPO_ROOT = Path(__file__).resolve().parent.parent
TIMEOUT_SECONDS = 30

#: The documented launcher every engine is reached through.
LAUNCHER = REPO_ROOT / "common" / "bin" / "herdr-run"

#: Placeholder for each edition's own project root inside arguments and environment values.
ROOT_TOKEN = "<ROOT>"

_RUN_ID_VALUE = re.compile(r'(?P<prefix>"run_id"\s*:\s*)"[^"]*"')
_RUN_ID_PATH = re.compile(r"\b\d{8}T\d{6}-[A-Za-z0-9_.-]+-\d+(?:-\d+)?\b")
_TIME_VALUE = re.compile(
    r'(?P<prefix>"(?:time|timestamp|started_at|finished_at)"\s*:\s*)"[^"]*"'
)
_PID_VALUE = re.compile(r'(?P<prefix>"(?:pid|shell_pid)"\s*:\s*)-?\d+')
_DURATION_VALUE = re.compile(
    r'(?P<prefix>"(?:duration|duration_seconds)"\s*:\s*)-?(?:\d+(?:\.\d*)?|\.\d+)'
)
_ENGINE_LINE = re.compile(r"^\[herdr-run\] engine=(?:python|rust) [^\n]*\n", re.MULTILINE)

#: Agent variables the editions read. Removed from the inherited environment so a developer's own
#: session name cannot leak into a case that is about something else.
_AGENT_VARIABLES = ("HERDR_RUN_AGENT", "DG_AGENT_NAME", "ORC_AGENT_NAME")


@dataclass(frozen=True)
class Outcome:
    """One observable CLI result; both streams decoded losslessly."""

    returncode: int
    stdout: str
    stderr: str


@dataclass
class Report:
    """Accumulated paired checks and actionable divergence descriptions."""

    checks: int = 0
    failures: list[str] = field(default_factory=list)
    notes: list[str] = field(default_factory=list)

    def require(self, label: str, condition: bool, detail: str) -> None:
        self.checks += 1
        if not condition:
            self.failures.append(f"{label}: {detail}")

    def exact(
        self, label: str, python: Outcome, rust: Outcome, *, expected_rc: int
    ) -> None:
        self.checks += 1
        if (
            python.returncode != expected_rc
            or rust.returncode != expected_rc
            or python != rust
        ):
            self.failures.append(
                f"{label}: expected rc {expected_rc}; "
                f"python={_describe(python)} rust={_describe(rust)}"
            )

    def same(
        self, label: str, python: Outcome, rust: Outcome, *, allowed: frozenset[int]
    ) -> None:
        """Byte-identical outcomes whose exit status is one of ``allowed``, and no crash text."""
        self.checks += 1
        crashed = _crashed(python) or _crashed(rust)
        if python != rust or python.returncode not in allowed or crashed:
            self.failures.append(
                f"{label}: expected identical outcomes with rc in {sorted(allowed)} and no "
                f"traceback/panic; python={_describe(python)} rust={_describe(rust)}"
            )


@dataclass(frozen=True)
class PairCase:
    """Equivalent isolated projects, one for each implementation."""

    python_root: Path
    rust_root: Path


Setup = Callable[[Path], None]


class Harness:
    """Create paired fixtures and invoke both supplied argv vectors hermetically."""

    def __init__(
        self,
        root: Path,
        python_command: Sequence[str],
        rust_command: Sequence[str],
    ) -> None:
        self.root = root
        self.python_command = _prepare_command(python_command)
        self.rust_command = _prepare_command(rust_command)
        self._serial = 0

    def case(
        self,
        label: str,
        files: Mapping[str, str | bytes] | None = None,
        *,
        setup: Setup | None = None,
    ) -> PairCase:
        self._serial += 1
        safe_label = re.sub(r"[^A-Za-z0-9_.-]+", "-", label).strip("-") or "case"
        case_root = self.root / f"{self._serial:03d}-{safe_label}"
        # Keep the project basename identical: ``{project}`` is intentionally observable in tab
        # labels, so naming the two roots "python" and "rust" would manufacture a divergence.
        python_root = case_root / "python" / "project"
        rust_root = case_root / "rust" / "project"
        for project in (python_root, rust_root):
            project.mkdir(parents=True)
            (project / ".home").mkdir()
            if files is not None:
                for relative, content in files.items():
                    path = project / relative
                    path.parent.mkdir(parents=True, exist_ok=True)
                    if isinstance(content, bytes):
                        path.write_bytes(content)
                    else:
                        path.write_text(content, encoding="utf-8")
            if setup is not None:
                setup(project)
        return PairCase(python_root, rust_root)

    def invoke(
        self,
        case: PairCase,
        args: Sequence[str],
        *,
        cwd: str = ".",
        extra_env: Mapping[str, str] | None = None,
        mode: str = "capture",
    ) -> tuple[Outcome, Outcome]:
        return self.invoke_commands(
            case,
            self.python_command,
            self.rust_command,
            args,
            cwd=cwd,
            extra_env=extra_env,
            mode=mode,
        )

    def invoke_commands(
        self,
        case: PairCase,
        python_command: Sequence[str],
        rust_command: Sequence[str],
        args: Sequence[str],
        *,
        cwd: str = ".",
        extra_env: Mapping[str, str] | None = None,
        rust_env: Mapping[str, str] | None = None,
        mode: str = "capture",
    ) -> tuple[Outcome, Outcome]:
        outcomes: list[Outcome] = []
        for command, root, own_env in (
            (python_command, case.python_root, None),
            (rust_command, case.rust_root, rust_env),
        ):
            working = root / cwd
            if mode != "deleted-cwd":
                working.mkdir(parents=True, exist_ok=True)
            environment = dict(extra_env or {})
            environment.update(own_env or {})
            outcome = _run(
                command,
                [_expand(argument, root) for argument in args],
                cwd=working,
                home=root / ".home",
                extra_env={key: _expand(value, root) for key, value in environment.items()},
                mode=mode,
            )
            outcomes.append(_normalize_outcome(outcome, root))
        return outcomes[0], outcomes[1]


def _expand(value: str, root: Path) -> str:
    return value.replace(ROOT_TOKEN, str(root))


def _prepare_command(command: Sequence[str]) -> tuple[str, ...]:
    if not command:
        raise ValueError("implementation command argv must not be empty")
    prepared = list(command)
    executable = prepared[0]
    if os.sep in executable and not os.path.isabs(executable):
        prepared[0] = str(Path(executable).resolve())
    return tuple(prepared)


def _environment(home: Path, extra: Mapping[str, str] | None) -> dict[str, str]:
    environment = dict(os.environ)
    for key in _AGENT_VARIABLES:
        environment.pop(key, None)
    original_home = environment.get("HOME")
    existing_pythonpath = environment.get("PYTHONPATH", "")
    local_pythonpath = str(REPO_ROOT / "py")
    environment["PYTHONPATH"] = (
        local_pythonpath
        if not existing_pythonpath
        else local_pythonpath + os.pathsep + existing_pythonpath
    )
    environment.update(
        {
            "HOME": str(home),
            "LANG": "C.UTF-8",
            "LC_ALL": "C.UTF-8",
            "NO_COLOR": "1",
            "TZ": "UTC",
        }
    )
    environment.pop("DAGRUN_ENGINE", None)
    # A development interpreter may have declared dependencies in its normal user site.  Changing
    # HOME for deterministic tilde expansion must not make that already-installed site disappear,
    # and the Rust launcher's toolchain must stay where the developer installed it.
    if original_home:
        environment.setdefault("PYTHONUSERBASE", str(Path(original_home) / ".local"))
        environment.setdefault("CARGO_HOME", str(Path(original_home) / ".cargo"))
        environment.setdefault("RUSTUP_HOME", str(Path(original_home) / ".rustup"))
    if extra is not None:
        environment.update(extra)
    return environment


def _decode_output(value: bytes | str | None) -> str:
    if value is None:
        return ""
    if isinstance(value, bytes):
        return value.decode("utf-8", errors="surrogateescape")
    return value


def _crashed(outcome: Outcome) -> bool:
    lowered = outcome.stderr.lower()
    return "traceback" in lowered or "panicked at" in lowered


def _run(
    command: Sequence[str],
    args: Sequence[str],
    *,
    cwd: Path,
    home: Path,
    extra_env: Mapping[str, str] | None,
    mode: str = "capture",
) -> Outcome:
    environment = _environment(home, extra_env)
    argv = [*command, *args]
    if mode == "capture":
        return _run_captured(argv, cwd, environment)
    if mode == "closed-stdout":
        return _run_captured(argv, cwd, environment, preexec=lambda: os.close(1))
    if mode == "deleted-cwd":
        doomed = str(cwd)

        def enter_and_delete() -> None:
            os.mkdir(doomed)
            os.chdir(doomed)
            os.rmdir(doomed)

        return _run_captured(argv, cwd.parent, environment, preexec=enter_and_delete)
    if mode == "dev-full":
        with open("/dev/full", "wb") as full:
            return _run_redirected(argv, cwd, environment, full.fileno())
    if mode == "broken-pipe":
        reader, writer = os.pipe()
        os.close(reader)
        try:
            return _run_redirected(argv, cwd, environment, writer)
        finally:
            os.close(writer)
    if mode == "sigint-while-writing":
        return _run_interrupted(argv, cwd, environment)
    raise ValueError(f"unknown invocation mode {mode}")


def _run_captured(
    argv: Sequence[str],
    cwd: Path,
    environment: Mapping[str, str],
    *,
    preexec: Callable[[], None] | None = None,
) -> Outcome:
    try:
        completed = subprocess.run(
            list(argv),
            cwd=cwd,
            env=dict(environment),
            stdin=subprocess.DEVNULL,
            capture_output=True,
            check=False,
            start_new_session=True,
            timeout=TIMEOUT_SECONDS,
            preexec_fn=preexec,
        )
    except subprocess.TimeoutExpired as error:
        return Outcome(
            124,
            _decode_output(error.stdout),
            _decode_output(error.stderr)
            + f"\nTIMEOUT after {TIMEOUT_SECONDS} seconds\n",
        )
    except (OSError, subprocess.SubprocessError) as error:
        return Outcome(127, "", f"could not invoke implementation: {error}\n")
    return Outcome(
        completed.returncode,
        _decode_output(completed.stdout),
        _decode_output(completed.stderr),
    )


def _run_redirected(
    argv: Sequence[str], cwd: Path, environment: Mapping[str, str], stdout: int
) -> Outcome:
    """Run with stdout on a chosen descriptor; only stderr is observed."""
    try:
        completed = subprocess.run(
            list(argv),
            cwd=cwd,
            env=dict(environment),
            stdin=subprocess.DEVNULL,
            stdout=stdout,
            stderr=subprocess.PIPE,
            check=False,
            start_new_session=True,
            timeout=TIMEOUT_SECONDS,
        )
    except subprocess.TimeoutExpired as error:
        return Outcome(124, "", _decode_output(error.stderr) + "\nTIMEOUT\n")
    except OSError as error:
        return Outcome(127, "", f"could not invoke implementation: {error}\n")
    return Outcome(completed.returncode, "", _decode_output(completed.stderr))


#: Linux's ``F_SETPIPE_SZ``; named here because older Pythons do not export it.
_F_SETPIPE_SZ = 1031


def _run_interrupted(argv: Sequence[str], cwd: Path, environment: Mapping[str, str]) -> Outcome:
    """Interrupt the process while it is blocked writing a long document to a full pipe.

    The pipe is shrunk to one page and never read until the process has filled it, so the process
    is provably parked inside its stdout write -- long after start-up -- when SIGINT arrives. The
    observable result is the exit status and stderr; the bytes already written are discarded.
    """
    reader, writer = os.pipe()
    try:
        try:
            fcntl.fcntl(writer, _F_SETPIPE_SZ, 4096)
        except OSError:
            pass
        process = subprocess.Popen(
            list(argv),
            cwd=cwd,
            env=dict(environment),
            stdin=subprocess.DEVNULL,
            stdout=writer,
            stderr=subprocess.PIPE,
            start_new_session=True,
        )
        os.close(writer)
        writer = -1
        deadline = time.monotonic() + TIMEOUT_SECONDS
        capacity = fcntl.fcntl(reader, 1032)  # F_GETPIPE_SZ
        while time.monotonic() < deadline and process.poll() is None:
            buffered = bytearray(4)
            fcntl.ioctl(reader, termios.FIONREAD, buffered)
            if int.from_bytes(buffered, sys.byteorder) >= capacity:
                break
            time.sleep(0.01)
        if process.poll() is None:
            # Give the writer a moment to be parked in write(2) on the now-full pipe.
            time.sleep(0.1)
            process.send_signal(signal.SIGINT)
        try:
            _, stderr = process.communicate(timeout=TIMEOUT_SECONDS)
        except subprocess.TimeoutExpired:
            process.kill()
            _, stderr = process.communicate()
            return Outcome(124, "", _decode_output(stderr) + "\nTIMEOUT\n")
        return Outcome(process.returncode, "", _decode_output(stderr))
    finally:
        os.close(reader)
        if writer >= 0:
            os.close(writer)


def _normalize_text(text: str, root: Path) -> str:
    normalized = text.replace(str(root), ROOT_TOKEN)
    normalized = _RUN_ID_VALUE.sub(r'\g<prefix>"<RUN_ID>"', normalized)
    normalized = _RUN_ID_PATH.sub("<RUN_ID>", normalized)
    normalized = _TIME_VALUE.sub(r'\g<prefix>"<TIME>"', normalized)
    normalized = _PID_VALUE.sub(r"\g<prefix><PID>", normalized)
    normalized = _DURATION_VALUE.sub(r"\g<prefix><DURATION>", normalized)
    return normalized


def _normalize_outcome(outcome: Outcome, root: Path) -> Outcome:
    return Outcome(
        outcome.returncode,
        _normalize_text(outcome.stdout, root),
        _normalize_text(outcome.stderr, root),
    )


def _describe(outcome: Outcome) -> str:
    return (
        f"Outcome(rc={outcome.returncode}, stdout={outcome.stdout!r}, "
        f"stderr={outcome.stderr!r})"
    )


#: Every subcommand the surface promises, at both levels.
#:
#: Named literally rather than scraped from either edition, so an edition that quietly loses a
#: subcommand fails here instead of teaching the harness its own smaller idea of the surface.
_SUBCOMMANDS: tuple[str, ...] = (
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
)

#: Long options accepted BEFORE the subcommand, and nowhere else.
_GLOBAL_OPTIONS = frozenset(
    {"--help", "--version", "--config", "--agent", "--json", "--dump-surface"}
)

#: Long options accepted AFTER each subcommand, and nowhere else.
_LOCAL_OPTIONS: dict[str, frozenset[str]] = {
    "run": frozenset(
        {"--help", "--cwd", "--timeout", "--wait-ready", "--no-cache", "--dry-run"}
    ),
    "check": frozenset({"--help"}),
    "config": frozenset({"--help"}),
    "init": frozenset({"--help", "--force"}),
    "status": frozenset({"--help"}),
    "target": frozenset({"--help", "--no-cache"}),
    "reap": frozenset({"--help"}),
    "net-doctor": frozenset({"--help"}),
    "quickstart": frozenset({"--help"}),
    "userguide": frozenset({"--help"}),
}

#: The metavariable each value-taking option names; every other option is a flag.
_OPTION_VALUES: dict[str, str] = {
    "--agent": "NAME",
    "--config": "PATH",
    "--cwd": "PATH",
    "--timeout": "SECONDS",
    "--wait-ready": "SECONDS",
}

#: Subcommands that take the one quoted command line as their positional argument.
_POSITIONAL: dict[str, tuple[str, ...]] = {"check": ("<command>",), "run": ("<command>",)}


def _expected_surface() -> dict[str, object]:
    """The ``--dump-surface`` document both editions must print, written out from the tables."""

    def options(names: frozenset[str]) -> dict[str, object]:
        return {name: _OPTION_VALUES.get(name) for name in sorted(names | {"-h"})}

    return {
        "global_options": options(_GLOBAL_OPTIONS),
        "subcommands": {
            name: {
                "options": options(_LOCAL_OPTIONS[name]),
                "positional": list(_POSITIONAL.get(name, ())),
            }
            for name in sorted(_SUBCOMMANDS)
        },
    }


#: Every configuration key the two editions accept, and therefore every key `init` must write.
_CONFIG_KEYS: tuple[str, ...] = (
    "allow",
    "allow_subcommand",
    "broker",
    "cwd",
    "deny_anywhere",
    "deny_global",
    "deny_subcommand",
    "max_panes",
    "prefixes",
    "probe_remote",
    "prompt_tail",
    "readiness",
    "ready_timeout_seconds",
    "retention_days",
    "shells",
    "spool_dir",
    "tab_name",
    "timeout_seconds",
    "value_options",
    "workspace",
)

_OPTION_TOKEN = re.compile(r"--[a-z][a-z0-9-]*")


def _block(text: str, header_prefix: str) -> list[str]:
    """Return the contiguous indented lines that follow the first line starting with a header."""
    lines = text.splitlines()
    for index, line in enumerate(lines):
        if line.startswith(header_prefix):
            body: list[str] = []
            for candidate in lines[index + 1 :]:
                if not candidate.strip():
                    break
                body.append(candidate)
            return body
    return []


def _declared_subcommands(text: str) -> tuple[str, ...]:
    return tuple(
        sorted(line.split()[0] for line in _block(text, "subcommands:") if line.split())
    )


def _declared_options(text: str, header_prefix: str) -> frozenset[str]:
    return frozenset(
        token
        for line in _block(text, header_prefix)
        for token in _OPTION_TOKEN.findall(line)
    )


def _help_surface(harness: Harness, report: Report, case: PairCase) -> None:
    """Compare every help screen byte for byte, and the whole two-level schema.

    ``report.exact`` on every help text already makes one edition growing a subcommand or an option
    the other lacks a divergence.  The literal expectations below add the case ``exact`` cannot
    catch: both editions dropping the same thing at once, which is agreement about a surface that
    no longer matches what the tool promises.
    """

    for flag in ("--help", "-h"):
        python, rust = harness.invoke(case, (flag,))
        report.exact(f"help/top-level{flag}", python, rust, expected_rc=0)
    python, rust = harness.invoke(case, ("--help",))
    report.require(
        "help/top-level-subcommands",
        _declared_subcommands(python.stdout) == tuple(sorted(_SUBCOMMANDS)),
        f"top-level help does not list exactly {sorted(_SUBCOMMANDS)}: {_describe(python)}",
    )
    report.require(
        "help/top-level-global-options",
        _declared_options(python.stdout, "global options") == _GLOBAL_OPTIONS,
        f"global option list changed: {_describe(python)}",
    )
    # The two levels must not document each other. This is the defect the surface exists to fix.
    top_level_local = _declared_options(python.stdout, "global options") & {
        option for options in _LOCAL_OPTIONS.values() for option in options
    } - {"--help"}
    report.require(
        "help/top-level-omits-local-options",
        not top_level_local,
        f"top-level help documents subcommand options {sorted(top_level_local)}",
    )

    for subcommand in _SUBCOMMANDS:
        for flag in ("--help", "-h"):
            python, rust = harness.invoke(case, (subcommand, flag))
            report.exact(f"help/{subcommand}{flag}", python, rust, expected_rc=0)
        python, rust = harness.invoke(case, (subcommand, "--help"))
        declared = _declared_options(python.stdout, "options:")
        report.require(
            f"help/{subcommand}-options",
            declared == _LOCAL_OPTIONS[subcommand],
            f"'{subcommand} --help' declares {sorted(declared)}, "
            f"expected {sorted(_LOCAL_OPTIONS[subcommand])}: {_describe(python)}",
        )
        leaked = declared & (_GLOBAL_OPTIONS - {"--help"})
        report.require(
            f"help/{subcommand}-omits-global-options",
            not leaked,
            f"'{subcommand} --help' documents global options {sorted(leaked)}",
        )
        # Help that is reached with globals in front is the same help.
        python, rust = harness.invoke(case, ("--json", subcommand, "--help"))
        report.exact(f"help/{subcommand}-after-a-global", python, rust, expected_rc=0)


def _surface_dump(harness: Harness, report: Report, case: PairCase) -> None:
    """``--dump-surface`` is the structured surface: identical bytes, and exactly the promise."""

    python, rust = harness.invoke(case, ("--dump-surface",))
    report.exact("surface/dump", python, rust, expected_rc=0)
    try:
        document = cast(dict[str, object], json.loads(python.stdout))
    except json.JSONDecodeError as error:
        report.require("surface/dump-is-json", False, f"{error}: {_describe(python)}")
        return
    version = document.pop("version", None)
    expected = _expected_surface()
    report.require(
        "surface/dump-matches-the-promised-surface",
        document == expected,
        f"--dump-surface printed {json.dumps(document, sort_keys=True)}, expected "
        f"{json.dumps(expected, sort_keys=True)}",
    )
    version_python, _ = harness.invoke(case, ("--version",))
    report.require(
        "surface/dump-version-is-the-version",
        version_python.stdout == f"herdr-run {version}\n",
        f"--dump-surface says version {version!r}; --version says {version_python.stdout!r}",
    )
    for label, args in (
        ("dump-surface-after-subcommand", ("check", "--dump-surface")),
        ("dump-surface-with-value", ("--dump-surface=x",)),
    ):
        python, rust = harness.invoke(case, args)
        report.exact(f"surface/{label}", python, rust, expected_rc=2)


def _levels(harness: Harness, report: Report, case: PairCase) -> None:
    """An option offered at the wrong level must be refused, identically, by both editions."""

    misplaced: tuple[tuple[str, tuple[str, ...]], ...] = (
        ("global-cwd", ("--cwd", "/tmp", "run", "git status")),
        ("global-timeout", ("--timeout", "5", "run", "git status")),
        ("global-wait-ready", ("--wait-ready", "5", "run", "git status")),
        ("global-no-cache", ("--no-cache", "target")),
        ("global-dry-run", ("--dry-run", "run", "git status")),
        ("local-agent", ("run", "--agent", "fixture-agent", "git status")),
        ("local-config", ("run", "--config", "x.yaml", "git status")),
        ("local-json", ("run", "--json", "git status")),
        ("foreign-dry-run", ("check", "--dry-run", "git status")),
        ("foreign-cwd", ("reap", "--cwd", "/tmp")),
        # Right level, wrong subcommand: `--config` is global, but these three read no
        # configuration file, so accepting it would be accepting an instruction and then
        # disregarding it. `--config P init` is the acute one: it reads as "write P".
        ("config-blind-init", ("--config", "x.yaml", "init")),
        ("config-blind-init-inline", ("--config=x.yaml", "init")),
        ("config-blind-init-force", ("--config", "x.yaml", "init", "--force")),
        ("config-blind-quickstart", ("--config", "x.yaml", "quickstart")),
        ("config-blind-userguide", ("--config", "x.yaml", "userguide")),
        # `--agent NAME` says who this invocation speaks for, and the one thing that does is name
        # a tab. These five resolve no tab, so accepting it would be accepting an instruction and
        # then doing nothing about it.
        ("agent-blind-check", ("--agent", "a", "check", "git status")),
        ("agent-blind-check-inline", ("--agent=a", "check", "git status")),
        ("agent-blind-init", ("--agent", "a", "init")),
        ("agent-blind-reap", ("--agent", "a", "reap")),
        ("agent-blind-quickstart", ("--agent", "a", "quickstart")),
        ("agent-blind-userguide", ("--agent", "a", "userguide")),
        # `--json` promises machine-readable output where a subcommand has it; these three have
        # only prose to give, so a caller who was humoured would fail at the parse of the output
        # rather than at the flag.
        ("json-blind-net-doctor", ("--json", "net-doctor")),
        ("json-blind-quickstart", ("--json", "quickstart")),
        ("json-blind-userguide", ("--json", "userguide")),
        # Unknown subcommands, unknown options, missing values, and repeated arguments.
        ("unknown-subcommand", ("frobnicate",)),
        ("unknown-global", ("--frobnicate", "config")),
        ("unknown-local", ("config", "--frobnicate")),
        ("missing-config-value", ("--config",)),
        ("missing-agent-value", ("config", "--agent")),
        ("missing-timeout-value", ("run", "--dry-run", "git status", "--timeout")),
        ("missing-command", ("run", "--dry-run")),
        ("extra-positional", ("check", "git status", "extra")),
        ("config-positional", ("config", "extra")),
        ("empty-subcommand", ("",)),
        ("double-dash-only", ("--",)),
        ("version-after-subcommand", ("config", "--version")),
    )
    for label, args in misplaced:
        python, rust = harness.invoke(case, args)
        report.exact(f"levels/{label}", python, rust, expected_rc=2)

    # Two blind globals at once must produce ONE refusal, and the same one in both editions.
    # `report.exact` already pins the agreement; naming --config pins which of the two it is, so
    # the editions cannot agree by both drifting to the other.
    python, rust = harness.invoke(case, ("--config", "x.yaml", "--agent", "a", "init"))
    report.exact("levels/two-blind-globals-give-one-refusal", python, rust, expected_rc=2)
    report.require(
        "levels/two-blind-globals-report-the-first",
        "argument --config:" in python.stderr and "argument --agent:" not in python.stderr,
        f"a --config/--agent pair did not report --config alone: {_describe(python)}",
    )

    # The globals a subcommand CAN observe stay accepted: the refusals are exactly as wide as the
    # blindness. `check` is the one that both refuses a global and accepts another.
    for label, args in (
        ("agent-sighted-run", ("--agent", "a", "run", "--dry-run", "git status")),
        ("json-sighted-check", ("--json", "check", "git status")),
        ("json-sighted-config", ("--json", "config")),
        ("repeated-agent-last-wins", ("--agent", "a", "--agent", "b", "config")),
        ("inline-agent", ("--agent=a", "config")),
        ("double-dash-command", ("check", "--", "git status")),
    ):
        python, rust = harness.invoke(case, args)
        report.exact(f"levels/{label}", python, rust, expected_rc=0)

    # The subcommand's own parse still wins, so `--help` and a bad local option are unaffected.
    python, rust = harness.invoke(case, ("--config", "x.yaml", "init", "--help"))
    report.exact("levels/config-blind-init-still-helps", python, rust, expected_rc=0)
    python, rust = harness.invoke(case, ("--config", "x.yaml", "init", "--nonsense"))
    report.exact("levels/config-blind-init-names-local-option", python, rust, expected_rc=2)
    report.require(
        "levels/config-blind-init-names-local-option-first",
        "--nonsense" in python.stderr,
        f"the local option error was replaced by the --config refusal: {_describe(python)}",
    )
    python, rust = harness.invoke(case, ("--agent", "a", "quickstart", "--nonsense"))
    report.exact("levels/agent-blind-names-local-option", python, rust, expected_rc=2)
    report.require(
        "levels/agent-blind-names-local-option-first",
        "--nonsense" in python.stderr,
        f"the local option error was replaced by the --agent refusal: {_describe(python)}",
    )


def _removed_bare_form(harness: Harness, report: Report, case: PairCase) -> None:
    """Running a command with no subcommand must fail, and must name what to type instead."""

    python, rust = harness.invoke(case, ("git status",))
    report.exact("bare-form/command-only", python, rust, expected_rc=2)
    report.require(
        "bare-form/command-only-names-run",
        "herdr-run run 'git status'" in python.stderr,
        f"the removed bare form did not name its replacement: {_describe(python)}",
    )

    python, rust = harness.invoke(case, ("release-agent", "git status"))
    report.exact("bare-form/agent-and-command", python, rust, expected_rc=2)
    report.require(
        "bare-form/agent-and-command-names-run",
        "herdr-run --agent release-agent run 'git status'" in python.stderr,
        f"the removed agent form did not name its replacement: {_describe(python)}",
    )

    python, rust = harness.invoke(case, ("run", "git", "status"))
    report.exact("bare-form/loose-words", python, rust, expected_rc=2)
    report.require(
        "bare-form/loose-words-refuse-rejoining",
        "ONE quoted argument" in python.stderr,
        f"loose command words were not refused: {_describe(python)}",
    )


def _bootstrap(harness: Harness, report: Report) -> None:
    case = harness.case("bootstrap")
    python, rust = harness.invoke(case, ("--version",))
    report.exact("bootstrap/version", python, rust, expected_rc=0)
    version_pattern = re.compile(
        r"herdr-run (?P<version>\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?)\n"
    )
    python_version = version_pattern.fullmatch(python.stdout)
    rust_version = version_pattern.fullmatch(rust.stdout)
    report.require(
        "bootstrap/version-shape",
        python_version is not None
        and rust_version is not None
        and python.stderr == "",
        f"unexpected version output: python={_describe(python)} rust={_describe(rust)}",
    )
    report.require(
        "bootstrap/version-source-parity",
        python_version is not None
        and rust_version is not None
        and python_version.group("version") == rust_version.group("version"),
        "Python package release version and Rust Cargo package version differ: "
        f"python={_describe(python)} rust={_describe(rust)}",
    )

    _help_surface(harness, report, case)
    _surface_dump(harness, report, case)
    _levels(harness, report, case)
    _removed_bare_form(harness, report, case)

    bare_python, bare_rust = harness.invoke(case, ())
    report.exact("bootstrap/bare", bare_python, bare_rust, expected_rc=0)
    for edition, outcome in (("python", bare_python), ("rust", bare_rust)):
        report.require(
            f"bootstrap/bare/{edition}",
            outcome.returncode == 0
            and outcome.stderr == ""
            and "subcommands:" in outcome.stdout,
            f"a bare invocation must print the subcommand list: {_describe(outcome)}",
        )

    python, rust = harness.invoke(
        case,
        ("--agent", "fixture-agent", "run", "--dry-run", "--", "git --help"),
    )
    report.exact("bootstrap/double-dash-command-help", python, rust, expected_rc=0)
    report.require(
        "bootstrap/double-dash-command-help-shape",
        python.stdout == "git --help\n" and python.stderr == "",
        f"command-local --help was intercepted: {_describe(python)}",
    )

    python, rust = harness.invoke(
        case,
        ("--ag", "fixture-agent", "run", "--dry", "git status"),
    )
    report.exact("bootstrap/no-option-abbreviations", python, rust, expected_rc=2)


def _policy(harness: Harness, report: Report) -> None:
    allowed = (
        ("git-status", "git status"),
        ("git-remote", "git ls-remote origin main"),
        ("prefix", "with-proxy git push origin HEAD:refs/heads/feature"),
        ("gh", "gh pr list --state open"),
        ("global-value", "git -C /tmp/repo log --oneline -5"),
        ("quoted-spaces", "git commit -m 'two words'"),
        ("empty-argument", "git commit -m ''"),
        ("mixed-quotes", "git commit -m pre\"mid\"'post'"),
        ("hash-literal", "gh issue view #12"),
        (
            "metacharacters-are-data",
            "git commit -m ';' '&&' '|' '$(id)' '`id`'",
        ),
        ("tilde", "git -C ~/work/repo status"),
        ("tilde-user", "git -C ~root/x status"),
        ("non-ascii-argument", "git commit -m 'caf\u00e9 \u202e \U0001f600'"),
        ("apostrophe-argument", "git commit -m \"it's\""),
        # An empty subcommand is still a subcommand: allowed unless a list names it.
        ("git-empty-subcommand", "git ''"),
        ("gh-empty-subcommand", "gh '' x"),
    )
    for label, command in allowed:
        case = harness.case(f"policy-allow-{label}")
        python, rust = harness.invoke(case, ("check", command))
        report.exact(f"policy/allowed/{label}", python, rust, expected_rc=0)
        python, rust = harness.invoke(case, ("--json", "check", command))
        report.exact(f"policy/allowed-json/{label}", python, rust, expected_rc=0)

    refused = (
        ("empty", ""),
        ("blank", "   "),
        ("unbalanced-quote", "git status '"),
        ("trailing-escape", "git status \\"),
        ("curl", "curl https://example.invalid"),
        ("bash", "bash -lc true"),
        ("sh", "sh -c true"),
        ("python", "python3 -c pass"),
        ("rm", "rm -rf /tmp/example"),
        ("wrapper-only", "with-proxy"),
        ("wrapper-repeated", "with-proxy with-proxy git status"),
        ("absolute-program", "/bin/git status"),
        ("dot-program", "./git status"),
        ("parent-program", "../bin/gh pr list"),
        ("tilde-program", "~/bin/git status"),
        ("tilde-user-program", "~root status"),
        ("git-config", "git -c alias.pwn='!sh' pwn"),
        ("git-exec-path", "git --exec-path=/tmp log"),
        ("git-config-env", "git --config-env=x=y status"),
        ("git-namespace", "git --namespace=x status"),
        ("gh-alias", "gh alias set pwn '!sh'"),
        ("gh-extension", "gh extension exec pwn"),
        ("gh-ext", "gh ext exec pwn"),
        ("gh-codespace", "gh codespace ssh"),
        ("upload-pack", "git fetch --upload-pack=/tmp/evil origin"),
        ("receive-pack", "git push --receive-pack /tmp/evil origin"),
        ("control-bel", "git status\x07"),
        ("control-tab", "git\tstatus"),
        ("control-line-feed", "git status\nwhoami"),
        ("control-carriage-return", "git status\rwhoami"),
        ("control-escape", "git status\x1b[2J"),
        ("control-del", "git status\x7f"),
        ("control-c1", "git status\x85"),
        ("control-separator", "git status\x1c"),
        ("program-bidi-override", "curl\u202ex"),
        ("program-no-break-space", "curl\u00a0x"),
        ("program-line-separator", "curl\u2028x"),
        ("program-byte-order-mark", "curl\ufeffx"),
        ("program-private-use", "curl\ue000x"),
        ("program-combining-mark", "cur\u0301l x"),
        ("program-unassigned", "curl\u0378"),
        ("program-tag-character", "curl\U000e0001"),
        ("program-emoji", "\U0001f600 x"),
        ("program-apostrophe", "it's x"),
        ("program-quote-and-backslash", 'say"\\ x'),
        ("cargo-bare", "cargo"),
        ("cargo-fetch-default", "cargo fetch"),
        ("cargo-build", "cargo build --release"),
        ("cargo-unknown", "cargo something-new"),
        ("cargo-config", "cargo --config build.rustc-wrapper=/tmp/evil fetch"),
    )
    for label, command in refused:
        case = harness.case(f"policy-refuse-{label}")
        python, rust = harness.invoke(case, ("check", command))
        report.exact(f"policy/refused/{label}", python, rust, expected_rc=77)
        python, rust = harness.invoke(case, ("--json", "check", command))
        report.exact(f"policy/refused-json/{label}", python, rust, expected_rc=77)

    # A refused run is audited; the REFUSED line must carry the same reason in both editions.
    audited = {".herdr-run.yaml": "spool_dir: .audit-spool\n"}
    for label, command in (
        ("bidi", "curl\u202ex"),
        ("control", "git status\x1b[2J"),
        ("tilde-program", "~root x"),
    ):
        case = harness.case(f"policy-refused-run-{label}", audited)
        python, rust = harness.invoke(case, ("--agent", "fixture-agent", "run", "--dry-run", command))
        report.exact(f"policy/refused-run/{label}", python, rust, expected_rc=77)
        report.exact(
            f"policy/refused-run/{label}/audit",
            _read_normalized(case.python_root / ".audit-spool" / "audit.jsonl", case.python_root),
            _read_normalized(case.rust_root / ".audit-spool" / "audit.jsonl", case.rust_root),
            expected_rc=0,
        )

    wildcard = {".herdr-run.yaml": 'allow: ["*"]\nprefixes: []\n'}
    for label, command, expected in (
        ("tilde-user-program", "~root x", 77),
        ("tilde-program", "~ x", 77),
        ("tilde-path-program", "~/bin/x y", 77),
        ("any-program", "curl https://example.invalid", 0),
    ):
        case = harness.case(f"policy-wildcard-{label}", wildcard)
        python, rust = harness.invoke(case, ("check", command))
        report.exact(f"policy/wildcard/{label}", python, rust, expected_rc=expected)

    double_dash = {
        ".herdr-run.yaml": 'allow: [git]\nprefixes: []\ndeny_global: {git: ["--", "-x"]}\n'
    }
    for label, command, expected in (
        ("clustered-short", "git -x-y log", 77),
        ("double-dash-token", "git -- log", 77),
        ("long-option", "git --x log", 0),
    ):
        case = harness.case(f"policy-deny-double-dash-{label}", double_dash)
        python, rust = harness.invoke(case, ("check", command))
        report.exact(f"policy/deny-double-dash/{label}", python, rust, expected_rc=expected)

    widened = {
        ".herdr-run.yaml": "allow: [git, cargo]\nprefixes: []\n",
    }
    for label, command in (
        ("fetch", "cargo fetch"),
        ("update", "cargo update -p serde"),
    ):
        case = harness.case(f"policy-custom-widened-{label}", widened)
        python, rust = harness.invoke(case, ("check", command))
        report.exact(f"policy/custom-widened/{label}", python, rust, expected_rc=0)

    for label, command in (
        ("config-before", "cargo --config build.rustc-wrapper=/tmp/evil fetch"),
        ("config-after", "cargo fetch --config build.rustc-wrapper=/tmp/evil"),
        ("config-attached-after", "cargo fetch --config=build.rustc-wrapper=/tmp/evil"),
        ("z-before-attached", "cargo -Zunstable-options fetch"),
        ("z-after", "cargo fetch -Z unstable-options"),
        ("z-after-attached", "cargo fetch -Zunstable-options"),
        ("z-after-clustered", "cargo fetch -qZunstable-options"),
    ):
        case = harness.case(f"policy-custom-widened-refuse-{label}", widened)
        python, rust = harness.invoke(case, ("check", command))
        report.exact(f"policy/custom-widened/refused/{label}", python, rust, expected_rc=77)

    minimum_cargo_guards = {
        ".herdr-run.yaml": (
            "allow: [cargo]\n"
            "deny_global: {}\n"
            "allow_subcommand:\n  cargo: [fetch]\n"
        ),
    }
    for label, command in (
        ("config-attached", "cargo fetch --config=x"),
        ("unstable-attached", "cargo fetch -Zunstable-options"),
    ):
        case = harness.case(f"policy-cargo-minimum-guard-{label}", minimum_cargo_guards)
        python, rust = harness.invoke(case, ("check", command))
        report.exact(f"policy/cargo-minimum-guard/{label}", python, rust, expected_rc=77)

    narrowed = {
        ".herdr-run.yaml": "allow: [git]\nprefixes: []\n",
    }
    case = harness.case("policy-custom-narrowed", narrowed)
    python, rust = harness.invoke(case, ("check", "gh pr list"))
    report.exact("policy/custom-narrowed", python, rust, expected_rc=77)


def _cli_grammar(harness: Harness, report: Report) -> None:
    """``--timeout`` and ``--wait-ready`` share one grammar: ASCII digits, whole milliseconds."""

    case = harness.case("cli-seconds")
    values = (
        "5",
        "0",
        "+5",
        ".5",
        "5.",
        "1e2",
        "1E1",
        "0.001",
        "0.0005",
        "0.0001",
        "31536000",
        "31536001",
        "1e300",
        "-1",
        "inf",
        "nan",
        "1_0",
        " 5",
        "5 ",
        "0x10",
        "",
        "\u0661\u0660",
        "1e\u0663",
        "5\u202e",
        "\u0665",
        "\uff15",
        "it's",
        "\x1b",
    )
    for value in values:
        for option in ("--timeout", "--wait-ready"):
            python, rust = harness.invoke(
                case, ("run", f"{option}={value}", "--dry-run", "git status")
            )
            report.same(
                f"grammar/{option}={value!r}", python, rust, allowed=frozenset({0, 2})
            )
    python, rust = harness.invoke(case, ("run", "--timeout", "-5", "--dry-run", "git status"))
    report.same("grammar/--timeout-separate-negative", python, rust, allowed=frozenset({2}))


def _agents(harness: Harness, report: Report) -> None:
    """Agent inference: one variable order, ASCII-whitespace trimming, non-UTF-8 refused."""

    case = harness.case("agents")
    environments: tuple[tuple[str, dict[str, str]], ...] = (
        ("none", {}),
        ("herdr", {"HERDR_RUN_AGENT": "herdr-agent"}),
        ("orc-only", {"ORC_AGENT_NAME": "orc"}),
        ("blank-then-dg", {"HERDR_RUN_AGENT": " \t ", "DG_AGENT_NAME": "  dg  "}),
        ("empty-then-orc", {"HERDR_RUN_AGENT": "", "DG_AGENT_NAME": "", "ORC_AGENT_NAME": "o"}),
        ("ascii-whitespace", {"HERDR_RUN_AGENT": "\x0b\x0cagent\r\n"}),
        ("separator-not-trimmed", {"HERDR_RUN_AGENT": "\x1cagent\x1c"}),
        ("ideographic-space", {"HERDR_RUN_AGENT": "\u3000agent\u3000"}),
        ("no-break-space", {"HERDR_RUN_AGENT": "\u00a0agent"}),
        ("inner-tab", {"HERDR_RUN_AGENT": "a\tb"}),
        ("non-ascii", {"HERDR_RUN_AGENT": "caf\u00e9"}),
        ("non-utf8", {"HERDR_RUN_AGENT": "\udcff"}),
        ("non-utf8-later", {"HERDR_RUN_AGENT": "", "DG_AGENT_NAME": "\udcfe"}),
        ("non-utf8-shadowed", {"HERDR_RUN_AGENT": "first", "DG_AGENT_NAME": "\udcfe"}),
    )
    for label, environment in environments:
        python, rust = harness.invoke(case, ("config",), extra_env=environment)
        report.same(f"agents/env/{label}", python, rust, allowed=frozenset({0, 78}))
    # A subcommand that names no tab never reads the agent, so a bad variable cannot break it.
    python, rust = harness.invoke(case, ("check", "git status"), extra_env={"HERDR_RUN_AGENT": "\udcff"})
    report.exact("agents/env/unread-by-check", python, rust, expected_rc=0)
    for label, value in (
        ("tab", "a\tb"),
        ("empty", ""),
        ("padded", " x "),
        ("non-utf8", "\udcff"),
        ("non-ascii", "caf\u00e9"),
        ("brace", "{agent}"),
    ):
        python, rust = harness.invoke(case, ("--agent", value, "config"))
        report.same(f"agents/flag/{label}", python, rust, allowed=frozenset({0, 2, 78}))


def _config_success(harness: Harness, report: Report) -> None:
    case = harness.case("config-default")
    python, rust = harness.invoke(case, ("--agent", "fixture-agent", "config"))
    report.exact("config/default", python, rust, expected_rc=0)
    report.require(
        "config/default-fields",
        '"source": "(built-in defaults)"' in python.stdout
        and '"project_root": "<ROOT>"' in python.stdout
        and '"tab_label": "fixture-agent"' in python.stdout
        and '"max_panes": 32' in python.stdout,
        f"default config omitted resolved fields: {_describe(python)}",
    )

    case = harness.case("config-trimmed-environment-agent")
    python, rust = harness.invoke(
        case,
        ("config",),
        extra_env={
            "HERDR_RUN_AGENT": "   ",
            "DG_AGENT_NAME": "  fixture-agent  ",
            "ORC_AGENT_NAME": "ignored-agent",
        },
    )
    report.exact("config/trimmed-environment-agent", python, rust, expected_rc=0)
    report.require(
        "config/trimmed-environment-agent-value",
        '"tab_label": "fixture-agent"' in python.stdout,
        f"environment agent was not trimmed: {_describe(python)}",
    )

    full = """\
workspace: command-deck
tab_name: "{project}-{agent}"
cwd: work/tree
allow: [git, gh, cargo]
prefixes: [with-proxy, envwrap]
deny_global:
  git: [-c]
  gh: []
  cargo: []
deny_subcommand:
  git: [daemon]
  gh: [alias]
  cargo: [install]
deny_anywhere: [--upload-pack]
allow_subcommand:
  cargo: [fetch, update]
value_options:
  git: [-C]
  gh: [-R]
  cargo: [--manifest-path]
spool_dir: .state/herdr
timeout_seconds: 12.5
retention_days: 7
max_panes: 12
ready_timeout_seconds: 3
readiness: process
prompt_tail: "$ "
shells: [bash, zsh]
probe_remote: https://example.invalid/repository
broker: systemd-run
"""
    case = harness.case("config-full", {".herdr-run.yaml": full})
    python, rust = harness.invoke(case, ("--agent", "fixture-agent", "config"))
    report.exact("config/full", python, rust, expected_rc=0)
    python, rust = harness.invoke(case, ("--json", "--agent", "fixture-agent", "config"))
    report.exact("config/full-json", python, rust, expected_rc=0)

    discovery = {
        ".herdr-run.yaml": "workspace: outer\n",
        "slot/.herdr-run.yaml": "workspace: inner\ntab_name: '{project}-{agent}'\n",
    }
    case = harness.case("config-nearest", discovery)
    python, rust = harness.invoke(
        case,
        ("--agent", "fixture-agent", "config"),
        cwd="slot/deep",
    )
    report.exact("config/discovery-nearest", python, rust, expected_rc=0)
    report.require(
        "config/discovery-nearest-value",
        '"workspace": "inner"' in python.stdout
        and '"project_root": "<ROOT>/slot"' in python.stdout,
        f"nearest config was not selected: {_describe(python)}",
    )

    precedence = {
        ".herdr-run.yaml": "workspace: yaml-wins\n",
        ".herdr-run.yml": "workspace: yml-loses\n",
    }
    case = harness.case("config-extension-precedence", precedence)
    python, rust = harness.invoke(case, ("config",))
    report.exact("config/yaml-before-yml", python, rust, expected_rc=0)
    report.require(
        "config/yaml-before-yml-value",
        '"workspace": "yaml-wins"' in python.stdout,
        f".yaml did not win over .yml: {_describe(python)}",
    )

    yaml_12 = """\
workspace: yes
allow: [git, on, off]
timeout_seconds: 0o10
"""
    case = harness.case("config-yaml-12", {".herdr-run.yaml": yaml_12})
    python, rust = harness.invoke(case, ("config",))
    report.exact("config/yaml-1.2-scalars", python, rust, expected_rc=0)
    report.require(
        "config/yaml-1.2-scalar-values",
        '"workspace": "yes"' in python.stdout
        and '"on"' in python.stdout
        and '"off"' in python.stdout
        and '"timeout_seconds": 8.0' in python.stdout,
        f"YAML 1.2 core scalars resolved incorrectly: {_describe(python)}",
    )

    # An explicit configuration path, spelled every way a caller might spell it.
    case = harness.case("config-explicit-paths", {"conf/settings.yaml": "workspace: explicit\n"})
    for label, path in (
        ("absolute", "<ROOT>/conf/settings.yaml"),
        ("double-slash-prefix", "/<ROOT>/conf/settings.yaml"),
        ("double-slash-inside", "<ROOT>//conf//settings.yaml"),
        ("relative", "conf/settings.yaml"),
        ("relative-dot-dot", "conf/../conf/settings.yaml"),
        ("relative-dot", "./conf/./settings.yaml"),
    ):
        python, rust = harness.invoke(case, ("--config", path, "config"))
        report.exact(f"config/explicit-path/{label}", python, rust, expected_rc=0)
        python, rust = harness.invoke(case, ("--config", path, "status"))
        report.exact(f"config/explicit-path/{label}/status", python, rust, expected_rc=0)


def _malformed_contract(outcome: Outcome) -> bool:
    return (
        outcome.returncode == 78
        and outcome.stdout == ""
        and bool(outcome.stderr.strip())
        and outcome.stderr.startswith("herdr-run:")
        and not _crashed(outcome)
    )


def _config_malformed(harness: Harness, report: Report) -> None:
    malformed = (
        ("invalid-yaml", "workspace: [\n"),
        ("duplicate-top-key", "workspace: one\nworkspace: two\n"),
        (
            "duplicate-nested-key",
            "deny_global:\n  git: []\n  git: [--exec-path]\n",
        ),
        ("merge-key", "deny_global:\n  <<: {git: []}\n"),
        ("nonfinite-inf", "timeout_seconds: .inf\n"),
        ("nonfinite-nan", "timeout_seconds: .nan\n"),
        ("unknown-key", "allowlist: [git]\n"),
        ("unknown-keys-sorted", "zzz: 1\naaa: 2\n"),
        ("allow-scalar", "allow: git\n"),
        ("allow-non-string", "allow: [git, true]\n"),
        ("empty-allow", "allow: []\n"),
        (
            "cargo-positive-list-omitted",
            "allow: [git, cargo]\nallow_subcommand:\n  custom-tool: [inspect]\n",
        ),
        ("negative-timeout", "timeout_seconds: -1\n"),
        ("excessive-timeout", "timeout_seconds: 1e300\n"),
        ("timeout-just-over", "timeout_seconds: 31536001\n"),
        ("timeout-sub-millisecond", "timeout_seconds: 0.00001\n"),
        ("timeout-half-millisecond", "timeout_seconds: 0.0005\n"),
        ("ready-timeout-sub-millisecond", "ready_timeout_seconds: 0.0001\n"),
        ("negative-retention", "retention_days: -1\n"),
        ("fractional-retention", "retention_days: 1.5\n"),
        ("negative-max-panes", "max_panes: -1\n"),
        ("fractional-max-panes", "max_panes: 1.5\n"),
        ("boolean-max-panes", "max_panes: true\n"),
        ("excessive-max-panes", "max_panes: 1000001\n"),
        ("malformed-tab-template", 'tab_name: "{agent"\n'),
        ("unmatched-close-tab-template", 'tab_name: "agent}"\n'),
        ("unknown-tab-placeholder", 'tab_name: "{x}"\n'),
        ("attribute-tab-template", 'tab_name: "{agent.__class__}"\n'),
        ("format-tab-template", 'tab_name: "{agent:>10}"\n'),
        ("conversion-tab-template", 'tab_name: "{agent!r}"\n'),
        ("positional-tab-template", 'tab_name: "{}"\n'),
        ("quoted-tab-template", "tab_name: \"it's {\"\n"),
        ("config-control", 'workspace: "bad\\x1bname"\n'),
        ("config-c1-control", 'workspace: "bad\\u0085name"\n'),
        ("custom-tag", "workspace: !evil nope\n"),
        ("bad-readiness", "readiness: maybe\n"),
        ("bad-broker", "broker: shell\n"),
        ("huge-integer", "timeout_seconds: " + "9" * 5_000 + "\n"),
        ("unicode-surrogate", 'workspace: "\\uD800"\n'),
        ("workspace-number", "workspace: 5\n"),
        ("workspace-list", "workspace: [a]\n"),
        ("workspace-mapping", "workspace: {a: b}\n"),
        ("workspace-bool", "workspace: true\n"),
        ("workspace-float", "workspace: 1.5\n"),
        ("workspace-empty", "workspace: ''\n"),
        ("non-string-mapping-key", "allow: [git]\nallow_subcommand: {true: [x]}\n"),
        ("order-tab-before-allow", "tab_name: '{x}'\nallow: 5\n"),
        ("order-deny-anywhere-before-allow-subcommand", "deny_anywhere: 5\nallow_subcommand: 5\n"),
        ("order-broker-before-probe-remote", "probe_remote: 5\nbroker: 5\n"),
        ("order-unknown-before-type", "zzz: 1\nallow: 5\n"),
        ("deny-global-non-string", "deny_global: {git: [5]}\n"),
        ("deny-global-scalar", "deny_global: {git: x}\n"),
        ("deny-global-list", "deny_global: [git]\n"),
        ("value-options-control", 'value_options: {git: ["-\\tC"]}\n'),
        ("prompt-tail-number", "prompt_tail: 5\n"),
        ("shells-scalar", "shells: bash\n"),
        ("probe-remote-control", 'probe_remote: "a\\nb"\n'),
        ("workspace-null", "workspace: ~\n"),
        ("cwd-number", "cwd: 5\n"),
        ("retention-huge", "retention_days: " + "9" * 40 + "\n"),
        ("root-sequence", "- a\n- b\n"),
        ("root-scalar", "just text\n"),
        ("not-utf8", b"workspace: caf\xe9\n"),
        ("not-utf8-in-comment", b"# \xff\nworkspace: x\n"),
        ("nul-byte", b"workspace: a\x00b\n"),
    )
    for label, document in malformed:
        case = harness.case(f"config-malformed-{label}", {".herdr-run.yaml": document})
        python, rust = harness.invoke(case, ("config",))
        report.exact(f"config/malformed/{label}", python, rust, expected_rc=78)
        report.require(
            f"config/malformed/{label}/contract",
            _malformed_contract(python),
            "expected a fail-closed rc 78 with a herdr-run diagnostic and no stdout; "
            f"python={_describe(python)}",
        )

    accepted = (
        ("empty-file", ""),
        ("comment-only", "# nothing\n"),
        ("document-markers", "---\nworkspace: marked\n...\n"),
        ("byte-order-mark", "\ufeffworkspace: bom\n"),
        ("crlf", "workspace: crlf\r\nallow: [git]\r\n"),
        ("float-formats", "timeout_seconds: 12.5\nready_timeout_seconds: 3\n"),
        ("float-exponent", "timeout_seconds: 1e3\nready_timeout_seconds: 1e-3\n"),
        ("float-large", "timeout_seconds: 31536000\nready_timeout_seconds: 100000.001\n"),
        ("integer-forms", "max_panes: 0x10\nretention_days: 0o7\n"),
        ("zero-timeout", "timeout_seconds: 0\n"),
        ("explicit-tags", "workspace: !!str 12\nmax_panes: !!int '5'\ntimeout_seconds: !!float 1\n"),
        ("leading-zero-string", "workspace: 007\n"),
        ("format-character", 'workspace: "a\\u202eb"\n'),
        ("non-ascii", "workspace: caf\u00e9\n"),
        ("doubled-braces", "tab_name: '{{agent}}-{agent}'\n"),
        ("cwd-absolute-dot-dot", "cwd: /a/../b\n"),
        ("cwd-relative-dot-dot", "cwd: ./x/../y\n"),
        ("cwd-double-slash", "cwd: //srv/x\n"),
        ("cwd-relative-double-slash", "cwd: x//y/\n"),
        ("spool-dir-absolute", "spool_dir: /var/tmp/herdr-spool\n"),
        ("null-values", "cwd: null\nprompt_tail:\n"),
        ("spool-dir-empty", "spool_dir: ''\n"),
        ("allow-any", 'allow: ["*"]\n'),
        ("max-panes-limit", "max_panes: 1000000\n"),
        ("retention-zero", "retention_days: 0\n"),
    )
    for label, document in accepted:
        case = harness.case(f"config-accepted-{label}", {".herdr-run.yaml": document})
        python, rust = harness.invoke(case, ("--agent", "fixture-agent", "config"))
        report.exact(f"config/accepted/{label}", python, rust, expected_rc=0)

    # Every document in the shared YAML corpus, read as a configuration file: the accepted ones
    # meet the configuration rules, the refused ones carry their positioned YAML message.
    corpus = cast(
        dict[str, list[dict[str, str]]],
        json.loads(
            (REPO_ROOT / "rs" / "herdr-run" / "testdata" / "yaml_cases.json").read_text(
                encoding="utf-8"
            )
        ),
    )
    for entry in corpus["cases"]:
        case = harness.case(f"config-yaml-{entry['name']}", {".herdr-run.yaml": entry["text"]})
        python, rust = harness.invoke(case, ("config",))
        report.same(
            f"config/yaml-corpus/{entry['name']}", python, rust, allowed=frozenset({0, 78})
        )

    case = harness.case("config-missing-explicit")
    python, rust = harness.invoke(case, ("--config", "missing.yaml", "config"))
    report.exact("config/malformed/missing-explicit", python, rust, expected_rc=78)
    report.require(
        "config/malformed/missing-explicit/contract",
        _malformed_contract(python),
        f"missing explicit config did not fail as configuration error; python={_describe(python)}",
    )


def _os_errors(harness: Harness, report: Report) -> None:
    """Every OS error reads ``<strerror> (os error N)`` in both editions, at the same moment."""

    case = harness.case("os-directory-as-config", setup=lambda root: (root / ".herdr-run.yaml").mkdir())
    python, rust = harness.invoke(case, ("config",))
    report.same("os/config-is-a-directory/discovered", python, rust, allowed=frozenset({0, 78}))
    python, rust = harness.invoke(case, ("--config", "<ROOT>/.herdr-run.yaml", "config"))
    report.exact("os/config-is-a-directory/explicit", python, rust, expected_rc=78)
    python, rust = harness.invoke(case, ("init", "--force"))
    report.same("os/init-over-a-directory", python, rust, allowed=frozenset({78}))

    case = harness.case(
        "os-dangling-symlink",
        setup=lambda root: (root / ".herdr-run.yaml").symlink_to(root / "missing-target.yaml"),
    )
    python, rust = harness.invoke(case, ("config",))
    report.same("os/dangling-symlink/discovered", python, rust, allowed=frozenset({0, 78}))
    python, rust = harness.invoke(case, ("--config", ".herdr-run.yaml", "config"))
    report.exact("os/dangling-symlink/explicit", python, rust, expected_rc=78)
    python, rust = harness.invoke(case, ("init",))
    report.same("os/dangling-symlink/init", python, rust, allowed=frozenset({0, 78}))
    python, rust = harness.invoke(case, ("init", "--force"))
    report.same("os/dangling-symlink/init-force", python, rust, allowed=frozenset({0, 78}))

    case = harness.case("os-missing-parent")
    python, rust = harness.invoke(case, ("--config", "no/such/dir/x.yaml", "config"))
    report.exact("os/missing-parent", python, rust, expected_rc=78)
    python, rust = harness.invoke(case, ("--config", "", "config"))
    report.same("os/empty-config-path", python, rust, allowed=frozenset({2, 78}))

    if os.geteuid() == 0:
        report.notes.append(
            "permission-denied cases were not run: the harness is running as root, which reads "
            "and writes regardless of mode bits"
        )
        return

    unreadable = harness.case(
        "os-unreadable-config",
        {".herdr-run.yaml": "workspace: x\n"},
        setup=lambda root: (root / ".herdr-run.yaml").chmod(0),
    )
    python, rust = harness.invoke(unreadable, ("config",))
    report.exact("os/unreadable-config", python, rust, expected_rc=78)

    read_only = harness.case("os-read-only-directory")
    for root in (read_only.python_root, read_only.rust_root):
        root.chmod(0o555)
    try:
        python, rust = harness.invoke(read_only, ("init",))
        report.exact("os/init-in-read-only-directory", python, rust, expected_rc=78)
    finally:
        for root in (read_only.python_root, read_only.rust_root):
            root.chmod(0o755)

    spool_blocked = harness.case(
        "os-spool-parent-is-a-file",
        {".herdr-run.yaml": "spool_dir: blocker/spool\n", "blocker": "not a directory\n"},
    )
    python, rust = harness.invoke(
        spool_blocked, ("--agent", "fixture-agent", "run", "--dry-run", "git status")
    )
    report.exact("os/spool-parent-is-a-file", python, rust, expected_rc=0)


def _non_utf8(harness: Harness, report: Report) -> None:
    """Bytes that are not UTF-8 in argv, the environment, and paths are handled identically."""

    case = harness.case("bytes-argv", {".herdr-run.yaml": "allow: [git, printf]\nprefixes: []\n"})
    for label, args in (
        ("check-argument", ("check", "git \udcff")),
        ("check-program", ("check", "\udcff x")),
        ("agent", ("--agent", "\udcff", "config")),
        ("config-path", ("--config", "\udcff.yaml", "config")),
        ("cwd", ("--agent", "a", "run", "--cwd", "\udcff", "--dry-run", "git status")),
        ("timeout", ("run", "--timeout", "\udcff", "--dry-run", "git status")),
        ("subcommand", ("\udcff",)),
        ("option", ("--\udcff",)),
        ("dry-run-argument", ("--agent", "a", "run", "--dry-run", "printf '\udce9'")),
    ):
        python, rust = harness.invoke(case, args)
        report.same(f"bytes/argv/{label}", python, rust, allowed=frozenset({0, 2, 77, 78}))

    def non_utf8_directory(root: Path) -> None:
        directory = root / "\udcff"
        directory.mkdir()
        (directory / ".herdr-run.yaml").write_text("workspace: bytes\n", encoding="utf-8")
        (directory / "deeper").mkdir()

    case = harness.case("bytes-project", setup=non_utf8_directory)
    for label, cwd, args in (
        ("project-root", "\udcff", ("config",)),
        ("below-project-root", "\udcff/deeper", ("config",)),
        ("check", "\udcff", ("check", "git status")),
        ("dry-run", "\udcff", ("--agent", "a", "run", "--dry-run", "git status")),
        ("status", "\udcff", ("--agent", "a", "status")),
        ("init", "\udcff/deeper", ("init",)),
    ):
        # Every subcommand that reads the directory refuses one that is not valid UTF-8.
        python, rust = harness.invoke(case, args, cwd=cwd)
        report.same(f"bytes/project/{label}", python, rust, allowed=frozenset({0, 78}))

    case = harness.case("bytes-home", setup=lambda root: (root / ".home" / "\udcff").mkdir())
    for label, command in (
        ("tilde-argument", "git -C ~/x status"),
        ("no-tilde", "git status"),
        ("tilde-user", "git -C ~root/x status"),
    ):
        python, rust = harness.invoke(
            case, ("check", command), extra_env={"HOME": "<ROOT>/.home/\udcff"}
        )
        report.same(f"bytes/home/{label}", python, rust, allowed=frozenset({0, 77, 78}))


def _locales(harness: Harness, report: Report) -> None:
    """The locale changes nothing: argv is read and output is written as UTF-8 bytes."""

    config = {".herdr-run.yaml": "allow: [printf, git]\nprefixes: []\nworkspace: caf\u00e9\n"}
    case = harness.case("locales", config)
    for locale_name in ("C", "POSIX", "en_US", "en_US.iso885915", "C.UTF-8", "xx_XX.nonexistent"):
        environment = {"LC_ALL": locale_name, "LANG": locale_name}
        for label, args in (
            ("dry-run", ("--agent", "caf\u00e9", "run", "--dry-run", "printf '\u00e9 \u202e'")),
            ("dry-run-json", ("--json", "--agent", "a", "run", "--dry-run", "printf '\u00e9'")),
            ("refusal", ("check", "curl \u00e9")),
            ("config", ("--agent", "\u00e9", "config")),
            ("status", ("--agent", "\u00e9", "status")),
            ("non-utf8-argument", ("check", "git \udce9")),
            ("help", ("--help",)),
            ("quickstart", ("quickstart",)),
        ):
            python, rust = harness.invoke(case, args, extra_env=environment)
            report.same(
                f"locale/{locale_name}/{label}", python, rust, allowed=frozenset({0, 2, 77, 78})
            )


def _output_failures(harness: Harness, report: Report) -> None:
    """A stdout that cannot be written, or a SIGINT, ends both editions the same way."""

    case = harness.case("output-failures", {".herdr-run.yaml": "allow: [git]\nprefixes: []\n"})
    commands: tuple[tuple[str, tuple[str, ...]], ...] = (
        ("version", ("--version",)),
        ("help", ("--help",)),
        ("surface", ("--dump-surface",)),
        ("quickstart", ("quickstart",)),
        ("config", ("--agent", "a", "config")),
        ("check", ("check", "git status")),
        ("check-json", ("--json", "check", "git status")),
        ("dry-run", ("--agent", "a", "run", "--dry-run", "git status")),
        ("refusal", ("check", "curl x")),
        ("usage-error", ("--frobnicate",)),
    )
    for mode, allowed in (
        ("closed-stdout", frozenset({0, 2, 77})),
        ("dev-full", frozenset({1, 2, 77})),
        ("broken-pipe", frozenset({1, 2, 77})),
    ):
        for label, args in commands:
            python, rust = harness.invoke(case, args, mode=mode)
            report.same(f"output/{mode}/{label}", python, rust, allowed=allowed)
    python, rust = harness.invoke(case, ("--version",), mode="dev-full")
    report.require(
        "output/dev-full-names-the-failure",
        python.returncode == 1
        and python.stderr
        == "herdr-run: cannot write stdout: No space left on device (os error 28)\n",
        f"a full device must be reported as one line with the OS error: {_describe(python)}",
    )
    # The user guide is many pages long, so it is still being written when SIGINT arrives.
    python, rust = harness.invoke(case, ("userguide",), mode="sigint-while-writing")
    report.exact("output/sigint/userguide", python, rust, expected_rc=-signal.SIGINT)


def _dry_run(harness: Harness, report: Report) -> None:
    config = """\
allow: [printf]
prefixes: []
spool_dir: .audit-spool
"""
    case = harness.case("dry-run", {".herdr-run.yaml": config})
    command = "printf '[%s]' 'two words' '; rm -rf /' '$(id)'"
    python, rust = harness.invoke(
        case,
        ("--json", "--agent", "fixture-agent", "run", "--dry-run", command),
    )
    report.exact("dry-run/success-json", python, rust, expected_rc=0)
    expected = (
        "{\n"
        '  "program": "printf",\n'
        "  \"rendered\": \"printf '[%s]' 'two words' '; rm -rf /' '$(id)'\",\n"
        '  "verdict": "allowed"\n'
        "}\n"
    )
    report.require(
        "dry-run/success-json-shape",
        python.stdout == expected and python.stderr == "",
        f"dry-run JSON/rendering changed: {_describe(python)}",
    )
    for label, args in (
        ("text", ("--agent", "fixture-agent", "run", "--dry-run", command)),
        ("non-ascii", ("--agent", "caf\u00e9", "run", "--dry-run", "printf 'caf\u00e9 \u202e'")),
        ("tilde", ("--agent", "fixture-agent", "run", "--dry-run", "printf ~/x ~root/y")),
        ("cwd-absolute", ("--agent", "a", "run", "--cwd", "/a/../b", "--dry-run", "printf x")),
        ("cwd-relative", ("--agent", "a", "run", "--cwd", "x/../y", "--dry-run", "printf x")),
        ("timeouts", ("--agent", "a", "run", "--timeout", "1.5", "--wait-ready", "0.25", "--dry-run", "printf x")),
    ):
        python, rust = harness.invoke(case, args)
        report.exact(f"dry-run/{label}", python, rust, expected_rc=0)

    python_audit = _read_normalized(
        case.python_root / ".audit-spool" / "audit.jsonl",
        case.python_root,
    )
    rust_audit = _read_normalized(
        case.rust_root / ".audit-spool" / "audit.jsonl",
        case.rust_root,
    )
    report.exact("dry-run/audit-jsonl", python_audit, rust_audit, expected_rc=0)


def _modes(root: Path, relative: Sequence[str]) -> Outcome:
    lines = []
    for name in relative:
        try:
            mode = stat.S_IMODE((root / name).lstat().st_mode)
        except OSError as error:
            lines.append(f"{name} missing ({errno.errorcode.get(error.errno or 0, '?')})")
            continue
        lines.append(f"{name} {mode:04o}")
    return Outcome(0, "\n".join(lines) + "\n", "")


def _spool(harness: Harness, report: Report) -> None:
    """Spool directories are private at every level, and an un-ignored spool is named exactly."""

    nested = harness.case("spool-nested", {".herdr-run.yaml": "spool_dir: a/b/c\n"})
    python, rust = harness.invoke(nested, ("--agent", "a", "run", "--dry-run", "git status"))
    report.exact("spool/nested-dry-run", python, rust, expected_rc=0)
    created = ("a", "a/b", "a/b/c", "a/b/c/audit.jsonl")
    python_modes = _modes(nested.python_root, created)
    rust_modes = _modes(nested.rust_root, created)
    report.exact("spool/nested-modes", python_modes, rust_modes, expected_rc=0)
    report.require(
        "spool/nested-modes-are-private",
        python_modes.stdout == "a 0700\na/b 0700\na/b/c 0700\na/b/c/audit.jsonl 0600\n",
        f"spool directories must be 0700 and the audit log 0600: {python_modes.stdout!r}",
    )

    git = shutil.which("git")
    if git is None:
        report.notes.append("the un-ignored spool warning was not compared: git is not installed")
        return

    def work_tree(root: Path) -> None:
        subprocess.run(
            [git, "init", "-q", str(root)],
            check=True,
            capture_output=True,
            env={**os.environ, "HOME": str(root / ".home"), "GIT_CONFIG_NOSYSTEM": "1"},
        )

    for label, spool_dir in (
        ("plain", ".herdr-run"),
        ("apostrophe-and-space", "out dir/it's"),
        ("non-ascii", "caf\u00e9/spool"),
    ):
        case = harness.case(
            f"spool-tracked-{label}",
            {".herdr-run.yaml": f"spool_dir: {json.dumps(spool_dir)}\n"},
            setup=work_tree,
        )
        environment = {"GIT_CONFIG_NOSYSTEM": "1"}
        python, rust = harness.invoke(
            case, ("--agent", "a", "run", "--dry-run", "git status"), extra_env=environment
        )
        report.exact(f"spool/tracked-warning/{label}", python, rust, expected_rc=0)
        report.require(
            f"spool/tracked-warning/{label}/present",
            "is NOT git-ignored" in python.stderr,
            f"an un-ignored spool in a git work tree was not reported: {_describe(python)}",
        )


def _net_doctor(harness: Harness, report: Report) -> None:
    """net-doctor refuses a probe the policy does not admit, before it runs anything."""

    for label, probe in (
        ("tilde", "~/repo"),
        ("tilde-user", "~nosuchuser-herdr/repo"),
        ("apostrophe", "/srv/it's repo"),
        ("control", "a\\u001bb"),
    ):
        config = f'allow: [gh]\nprobe_remote: "{probe}"\nspool_dir: .audit-spool\n'
        case = harness.case(f"net-doctor-{label}", {".herdr-run.yaml": config})
        python, rust = harness.invoke(case, ("--agent", "fixture-agent", "net-doctor"))
        report.same(f"net-doctor/refused/{label}", python, rust, allowed=frozenset({77, 78}))
        report.exact(
            f"net-doctor/refused/{label}/audit",
            _read_normalized(case.python_root / ".audit-spool" / "audit.jsonl", case.python_root),
            _read_normalized(case.rust_root / ".audit-spool" / "audit.jsonl", case.rust_root),
            expected_rc=0,
        )


def _read_normalized(path: Path, root: Path) -> Outcome:
    try:
        text = path.read_bytes().decode("utf-8", errors="surrogateescape")
    except FileNotFoundError:
        return Outcome(0, "(absent)\n", "")
    except OSError as error:
        return Outcome(1, "", f"cannot read {path.relative_to(root)}: {error.strerror}")
    return Outcome(0, _normalize_text(text, root), "")


#: Workspace label used by the reap fixtures. Deliberately not a name any real session would carry,
#: so a reachable Herdr server cannot resolve it. A host with no reachable server reports the
#: safety-equivalent unavailable-evidence reason instead; both paths must keep the pane UNKNOWN.
_REAP_WORKSPACE = "cross-differential-fixture"

_REAP_CONFIG = f"""\
workspace: {_REAP_WORKSPACE}
allow: [printf]
prefixes: []
spool_dir: .herdr-run
retention_days: 7
"""


def _reap_record(
    pane_id: str,
    workspace: object,
    exit_code: str = "0",
    *,
    agent: object = "kvm",
    tab: object = None,
) -> str:
    """One planted ``meta.json``, shaped exactly as the runner writes it."""
    return json.dumps(
        {
            "agent": agent,
            "exit_code": None if exit_code == "null" else int(exit_code),
            "pane_id": pane_id,
            "readiness": {
                "boot_id": "3f2b1c8e-0000-4000-8000-000000000001",
                "shell_pid": 4242,
                "shell_start_ticks": 900,
            },
            "run_id": "20260819T000000-kvm-1",
            "tab": {"id": "w1:t1", "label": "kvm"} if tab is None else tab,
            "workspace": {"id": "w1", "label": workspace}
            if isinstance(workspace, str)
            else workspace,
        },
        indent=2,
        sort_keys=True,
    )


def _workspace_is_safely_unresolvable(stdout: str) -> bool:
    """The three safe reasons a planted workspace cannot establish live-pane evidence."""

    return (
        "no workspace labelled" in stdout
        or "evidence unavailable: workspace list:" in stdout
        or "evidence unavailable: Herdr executable not found in fixed install locations:" in stdout
    )


def _reap(harness: Harness, report: Report) -> None:
    """Compare the REAPING VERDICTS, not just that both editions list the subcommand.

    ``reap`` is the one subcommand whose output can authorise destroying someone's running work, so
    "both help texts contain the word reap" is not a cross-edition guarantee of anything. These
    cases plant a run spool and compare the whole document byte for byte: counts, per-pane verdicts,
    reason wording, key ordering, and the retention window the candidate set is bounded by.

    Only the host-independent verdicts can be paired here. STALE and SHELL_ALIVE need a pane a live
    Herdr server still lists, and this harness deliberately cannot stand one up; those two are
    covered by the planted-population unit tests in both editions instead.
    """
    empty = harness.case("reap-empty", {".herdr-run.yaml": _REAP_CONFIG})
    python, rust = harness.invoke(empty, ("reap",))
    report.exact("reap/empty-spool", python, rust, expected_rc=0)
    report.require(
        "reap/empty-spool-shape",
        python.stderr == ""
        and '"considered": 0' in python.stdout
        and all(
            f'"{verdict}": 0' in python.stdout
            for verdict in ("STALE", "IN_FLIGHT", "SHELL_ALIVE", "GONE", "UNKNOWN", "OUT_OF_SCOPE")
        )
        and '"retention_days": 7' in python.stdout
        and f'"workspace": "{_REAP_WORKSPACE}"' in python.stdout
        and all(f'"{key}"' in python.stdout for key in ("occupancy", "max_panes", "without_record")),
        "an inert sweep must still print its own shape, the window bounding it, and what holds "
        "max_panes: "
        f"{_describe(python)}",
    )

    scoped = harness.case(
        "reap-in-scope",
        {
            ".herdr-run.yaml": _REAP_CONFIG,
            ".herdr-run/runs/20260819T000000-kvm-1/meta.json": _reap_record(
                "w1:p1", _REAP_WORKSPACE
            ),
        },
    )
    python, rust = harness.invoke(scoped, ("reap",))
    report.exact("reap/unresolvable-workspace", python, rust, expected_rc=0)
    report.require(
        "reap/unresolvable-workspace-shape",
        '"STALE": 0' in python.stdout
        and '"UNKNOWN": 1' in python.stdout
        and '"reapable": []' in python.stdout
        and _workspace_is_safely_unresolvable(python.stdout),
        "a workspace herdr cannot resolve must reap nothing and say why: "
        f"{_describe(python)}",
    )

    foreign = harness.case(
        "reap-out-of-scope",
        {
            ".herdr-run.yaml": _REAP_CONFIG,
            ".herdr-run/runs/20260819T000000-kvm-1/meta.json": _reap_record(
                "w1:p1", "someone-elses"
            ),
        },
    )
    python, rust = harness.invoke(foreign, ("reap",))
    report.exact("reap/out-of-scope", python, rust, expected_rc=0)
    report.require(
        "reap/out-of-scope-shape",
        '"OUT_OF_SCOPE": 1' in python.stdout
        and '"STALE": 0' in python.stdout
        and '"reapable": []' in python.stdout,
        f"a pane recorded in another workspace must never be a candidate: {_describe(python)}",
    )

    run_dir = ".herdr-run/runs/20260819T000000-kvm-1"
    planted: tuple[tuple[str, dict[str, str | bytes]], ...] = (
        ("tab-is-a-string", {f"{run_dir}/meta.json": _reap_record("w1:p1", _REAP_WORKSPACE, tab="x")}),
        ("workspace-is-a-number", {f"{run_dir}/meta.json": _reap_record("w1:p1", 5)}),
        ("agent-is-a-number", {f"{run_dir}/meta.json": _reap_record("w1:p1", _REAP_WORKSPACE, agent=7)}),
        (
            "apostrophe-labels",
            {
                f"{run_dir}/meta.json": _reap_record(
                    "w1:p1",
                    "it's-elsewhere",
                    agent="o'neil",
                    tab={"id": "w1:t1", "label": "o'neil"},
                )
            },
        ),
        (
            "control-labels",
            {
                f"{run_dir}/meta.json": _reap_record(
                    "w1:p1", "bad\u001bspace", tab={"id": "w1:t1", "label": "tab\u202e"}
                )
            },
        ),
        (
            "late-exit-code-not-utf8",
            {
                f"{run_dir}/meta.json": _reap_record("w1:p1", "elsewhere", "null"),
                f"{run_dir}/exit_code": b"\xff\n",
            },
        ),
        (
            "late-exit-code-valid",
            {
                f"{run_dir}/meta.json": _reap_record("w1:p1", "elsewhere", "null"),
                f"{run_dir}/exit_code": b" 7\n",
            },
        ),
        (
            "late-exit-code-out-of-range",
            {
                f"{run_dir}/meta.json": _reap_record("w1:p1", "elsewhere", "null"),
                f"{run_dir}/exit_code": b"2147483648\n",
            },
        ),
        ("meta-not-json", {f"{run_dir}/meta.json": "{not json"}),
        ("meta-not-utf8", {f"{run_dir}/meta.json": b'{"pane_id": "\xff"}'}),
        ("meta-nan", {f"{run_dir}/meta.json": '{"pane_id": "w1:p1", "exit_code": NaN}'}),
        ("meta-array", {f"{run_dir}/meta.json": "[]"}),
    )
    for label, files in planted:
        case = harness.case(f"reap-planted-{label}", {".herdr-run.yaml": _REAP_CONFIG, **files})
        python, rust = harness.invoke(case, ("reap",))
        report.exact(f"reap/planted/{label}", python, rust, expected_rc=0)
        python, rust = harness.invoke(case, ("--json", "reap"))
        report.exact(f"reap/planted/{label}/json", python, rust, expected_rc=0)


#: `status` fixtures name a workspace no real session would carry, so a host that happens to be
#: running a live Herdr server cannot resolve it and the report stays the same everywhere.
_STATUS_CONFIG = f"""\
workspace: {_REAP_WORKSPACE}
allow: [git, gh]
prefixes: [with-proxy]
spool_dir: .herdr-run
"""

_STATUS_ANY_CONFIG = f"""\
workspace: {_REAP_WORKSPACE}
allow: ["*"]
prefixes: []
"""


def _documentation(harness: Harness, report: Report) -> None:
    """Both documentation commands print the same bytes in both editions.

    `quickstart` and `userguide` are each one shared source, including the installation section,
    and are compared byte for byte.

    The two documents are also compared to each other: `quickstart` is supposed to be a much
    shorter amended version, and two commands printing the same text would be one command with two
    names.
    """

    case = harness.case("documentation")
    quickstart_python, quickstart_rust = harness.invoke(case, ("quickstart",))
    report.exact("documentation/quickstart", quickstart_python, quickstart_rust, expected_rc=0)
    guide_python, guide_rust = harness.invoke(case, ("userguide",))
    report.exact("documentation/userguide", guide_python, guide_rust, expected_rc=0)
    report.require(
        "documentation/userguide-is-the-reference",
        "## Installation" in guide_python.stdout and len(guide_python.stdout) > 5000,
        f"the user guide lost its body or its installation section: {len(guide_python.stdout)} "
        "characters",
    )
    report.require(
        "documentation/quickstart-is-shorter",
        quickstart_python.stdout != guide_python.stdout
        and 2 * quickstart_python.stdout.count("\n") < guide_python.stdout.count("\n"),
        "quickstart is supposed to be a much shorter amended version of the guide: "
        f"{quickstart_python.stdout.count(chr(10))} lines against "
        f"{guide_python.stdout.count(chr(10))}",
    )

    # A configuration file too broken to load must not be able to withhold the documentation.
    broken = harness.case("documentation-broken-config", {".herdr-run.yaml": "allow: [\n"})
    python, rust = harness.invoke(broken, ("quickstart",))
    report.exact("documentation/quickstart-despite-broken-config", python, rust, expected_rc=0)
    python, rust = harness.invoke(broken, ("userguide",))
    report.require(
        "documentation/userguide-despite-broken-config",
        python == guide_python and rust == guide_rust,
        "a configuration file too broken to load withheld the user guide: "
        f"python={_describe(python)} rust={_describe(rust)}",
    )

    help_python, _ = harness.invoke(case, ("--help",))
    report.require(
        "documentation/help-distinguishes-them",
        "quickstart" in help_python.stdout
        and "userguide" in help_python.stdout
        and "Two documentation commands" in help_python.stdout,
        f"the top-level help does not say what each document is for: {_describe(help_python)}",
    )


def _status(harness: Harness, report: Report) -> None:
    """`status` must read the same and say the same, and must still be read-only in both."""

    case = harness.case("status", {".herdr-run.yaml": _STATUS_CONFIG})
    python, rust = harness.invoke(case, ("--agent", "fixture-agent", "status"))
    report.exact("status/text", python, rust, expected_rc=0)
    for fragment in (
        "    file          <ROOT>/.herdr-run.yaml\n",
        "    project root  <ROOT>\n",
        "    spool dir     .herdr-run\n",
        "    allow         git, gh\n",
        "    prefixes      with-proxy\n",
        "    agent         fixture-agent\n",
        f"    workspace     {_REAP_WORKSPACE}\n",
        "    tab label     fixture-agent",
        "\nNothing was changed: status only reads.\n",
    ):
        report.require(
            f"status/text-says/{fragment.split()[0]}",
            fragment in python.stdout,
            f"status omitted {fragment!r}: {_describe(python)}",
        )

    json_python, json_rust = harness.invoke(
        case, ("--json", "--agent", "fixture-agent", "status")
    )
    report.exact("status/json", json_python, json_rust, expected_rc=0)
    report.require(
        "status/json-shape",
        '"agent": "fixture-agent"' in json_python.stdout
        and '"allow_any_program": false' in json_python.stdout
        and '"config_file": "<ROOT>/.herdr-run.yaml"' in json_python.stdout
        and '"label": "fixture-agent"' in json_python.stdout
        and f'"workspace": "{_REAP_WORKSPACE}"' in json_python.stdout,
        f"status JSON changed shape: {_describe(json_python)}",
    )

    # Nothing status did may have created state on disk either.
    for edition, root in (("python", case.python_root), ("rust", case.rust_root)):
        report.require(
            f"status/creates-nothing/{edition}",
            not (root / ".herdr-run").exists(),
            "status created spool state in a directory it was only supposed to describe",
        )

    wildcard = harness.case("status-allow-any", {".herdr-run.yaml": _STATUS_ANY_CONFIG})
    python, rust = harness.invoke(wildcard, ("--agent", "fixture-agent", "status"))
    report.exact("status/allow-everything", python, rust, expected_rc=0)
    report.require(
        "status/allow-everything-shape",
        '    allow         any program ("*")\n' in python.stdout
        and "    prefixes      (none)\n" in python.stdout,
        f"the allow-everything mode was not reported as such: {_describe(python)}",
    )

    # Values that need quoting, and a timeout that is not an integer, read the same in both.
    quoted = harness.case(
        "status-quoted",
        {
            ".herdr-run.yaml": (
                f"workspace: {_REAP_WORKSPACE}\n"
                "allow: [git]\n"
                "timeout_seconds: 0.25\n"
                "ready_timeout_seconds: 1e-3\n"
                "cwd: /srv/it's\n"
                'prompt_tail: "\\u00bb "\n'
            )
        },
    )
    for label, args in (
        ("text", ("--agent", "o'neil", "status")),
        ("json", ("--json", "--agent", "o'neil", "status")),
    ):
        python, rust = harness.invoke(quoted, args)
        report.exact(f"status/quoted/{label}", python, rust, expected_rc=0)


def _init(harness: Harness, report: Report) -> None:
    """`init` must write the same bytes in both editions, and both must then read them the same.

    Comparing the generated `.herdr-run.yaml` byte for byte is what lets the user guide point at
    that file instead of restating it: the file is the reference, so a reference that differed
    between editions would be two references.
    """

    # `--config PATH` reads as "write PATH" here and would in fact write ./.herdr-run.yaml, so it
    # is refused. The refusal has to be total: nothing written, in either edition.
    refused = harness.case("init-config-refused")
    python, rust = harness.invoke(refused, ("--config", "elsewhere.yaml", "init"))
    report.exact("init/config-is-refused", python, rust, expected_rc=2)
    report.require(
        "init/config-refusal-writes-nothing",
        not (refused.python_root / ".herdr-run.yaml").exists()
        and not (refused.rust_root / ".herdr-run.yaml").exists(),
        "a refused '--config PATH init' still wrote a configuration file",
    )

    # `--agent NAME init` is refused for the same reason and must be just as total: `init` names no
    # tab, so the name has nowhere to go, and a refusal that still wrote the file would be worse
    # than accepting the flag.
    refused = harness.case("init-agent-refused")
    python, rust = harness.invoke(refused, ("--agent", "a", "init"))
    report.exact("init/agent-is-refused", python, rust, expected_rc=2)
    report.require(
        "init/agent-refusal-writes-nothing",
        not (refused.python_root / ".herdr-run.yaml").exists()
        and not (refused.rust_root / ".herdr-run.yaml").exists(),
        "a refused '--agent NAME init' still wrote a configuration file",
    )

    case = harness.case("init")
    python, rust = harness.invoke(case, ("init",))
    report.exact("init/first-write", python, rust, expected_rc=0)

    python_file = _read_normalized(case.python_root / ".herdr-run.yaml", case.python_root)
    rust_file = _read_normalized(case.rust_root / ".herdr-run.yaml", case.rust_root)
    report.exact("init/template-bytes", python_file, rust_file, expected_rc=0)
    report.exact(
        "init/template-mode",
        _modes(case.python_root, (".herdr-run.yaml",)),
        _modes(case.rust_root, (".herdr-run.yaml",)),
        expected_rc=0,
    )
    for marker in (
        "a human-only knob",
        "DO NOT LET AN AGENT EDIT THIS SECTION",
        "worktrees/slotNN/",
        "ALLOW-EVERYTHING MODE",
        'allow: ["*"]',
    ):
        report.require(
            f"init/template-says/{marker[:24]}",
            marker in python_file.stdout,
            f"the generated configuration never says {marker!r}",
        )

    # Every knob has to be there, or the guide cannot point at this file instead of listing them.
    missing = [key for key in _CONFIG_KEYS if f"\n{key}:" not in python_file.stdout]
    report.require(
        "init/template-covers-every-key",
        not missing,
        f"the generated configuration never sets {missing}",
    )

    resolved_python, resolved_rust = harness.invoke(
        case, ("--agent", "fixture-agent", "config")
    )
    report.exact("init/config-after-init", resolved_python, resolved_rust, expected_rc=0)

    python, rust = harness.invoke(case, ("init",))
    report.exact("init/refuses-to-clobber", python, rust, expected_rc=78)
    report.require(
        "init/refuses-to-clobber-names-force",
        "--force" in python.stderr and python.stdout == "",
        f"a refused init must name --force on stderr and print nothing: {_describe(python)}",
    )

    python, rust = harness.invoke(case, ("init", "--force"))
    report.exact("init/force-overwrites", python, rust, expected_rc=0)


def _deleted_cwd(harness: Harness, report: Report) -> None:
    """A deleted working directory reaches both tools, and both say the same about it."""

    case = harness.case("deleted-cwd", {".herdr-run.yaml": "allow: [git]\n"})
    for label, args, allowed in (
        ("version", ("--version",), frozenset({0})),
        ("config", ("config",), frozenset({78})),
        ("check", ("check", "git status"), frozenset({0, 78})),
        ("dry-run", ("--agent", "a", "run", "--dry-run", "git status"), frozenset({0, 78})),
        ("init", ("init",), frozenset({78})),
        ("quickstart", ("quickstart",), frozenset({0})),
    ):
        python, rust = harness.invoke(case, args, cwd="gone", mode="deleted-cwd")
        report.same(f"deleted-cwd/{label}", python, rust, allowed=allowed)


def _strip_engine_line(outcome: Outcome) -> tuple[Outcome, int]:
    stripped, count = _ENGINE_LINE.subn("", outcome.stderr)
    return Outcome(outcome.returncode, outcome.stdout, stripped), count


def _launchers(harness: Harness, report: Report) -> None:
    """Through the documented launcher, both engines differ only in the one ``engine=`` line."""

    if not LAUNCHER.exists():
        report.notes.append(f"launcher cases were not run: {LAUNCHER} is missing")
        return
    case = harness.case("launchers", {".herdr-run.yaml": "allow: [git]\nprefixes: []\n"})
    command = (str(LAUNCHER),)
    for label, args, mode, allowed in (
        ("version", ("--version",), "capture", frozenset({0})),
        ("help", ("--help",), "capture", frozenset({0})),
        ("surface", ("--dump-surface",), "capture", frozenset({0})),
        ("refusal", ("check", "curl\u202ex"), "capture", frozenset({77})),
        ("usage", ("--frobnicate",), "capture", frozenset({2})),
        ("non-utf8-argument", ("check", "git \udcff"), "capture", frozenset({2})),
        ("locale", ("check", "curl \u00e9"), "capture", frozenset({77})),
        ("deleted-cwd-version", ("--version",), "deleted-cwd", frozenset({0})),
        ("deleted-cwd-config", ("config",), "deleted-cwd", frozenset({78})),
        ("dev-full", ("--version",), "dev-full", frozenset({1})),
    ):
        environment = {"LC_ALL": "en_US", "LANG": "en_US"} if label == "locale" else None
        python, rust = harness.invoke_commands(
            case,
            command,
            command,
            args,
            cwd="gone" if mode == "deleted-cwd" else ".",
            extra_env=environment,
            rust_env={"DAGRUN_ENGINE": "rust"},
            mode=mode,
        )
        python_body, python_lines = _strip_engine_line(python)
        rust_body, rust_lines = _strip_engine_line(rust)
        report.same(f"launcher/{label}", python_body, rust_body, allowed=allowed)
        report.require(
            f"launcher/{label}/one-engine-line",
            python_lines == 1 and rust_lines == 1,
            "each engine must announce itself in exactly one line: "
            f"python={_describe(python)} rust={_describe(rust)}",
        )


def build_report(
    python_command: Sequence[str],
    rust_command: Sequence[str],
    *,
    launchers: bool = True,
) -> Report:
    """Run the complete differential and return its structured report without printing."""

    report = Report()
    with tempfile.TemporaryDirectory(prefix="herdr-run-cross-") as temporary:
        harness = Harness(Path(temporary), python_command, rust_command)
        _bootstrap(harness, report)
        _policy(harness, report)
        _cli_grammar(harness, report)
        _agents(harness, report)
        _config_success(harness, report)
        _config_malformed(harness, report)
        _os_errors(harness, report)
        _non_utf8(harness, report)
        _locales(harness, report)
        _output_failures(harness, report)
        _dry_run(harness, report)
        _spool(harness, report)
        _net_doctor(harness, report)
        _init(harness, report)
        _status(harness, report)
        _documentation(harness, report)
        _reap(harness, report)
        _deleted_cwd(harness, report)
        if launchers:
            _launchers(harness, report)
    report.notes.append(
        "Herdr protocol, readiness, session-cache, and run-record behaviour is compared through "
        "the golden corpora in rs/herdr-run/testdata, which both editions' unit tests load: "
        "production resolution intentionally ignores caller PATH and exposes no safe executable "
        "override for an external fake"
    )
    report.notes.append(
        "reap is paired on safety-equivalent verdicts (empty spool, unresolvable or unavailable "
        "workspace evidence, out of scope, malformed records); STALE and SHELL_ALIVE need a pane "
        "a live Herdr server still lists, and are covered by planted-population unit tests in "
        "each edition instead"
    )
    report.notes.append(
        "NUL cannot be represented in a POSIX argv entry; all other terminal-control classes are covered"
    )
    return report


def compare_herdr_run(
    python_command: Sequence[str],
    rust_command: Sequence[str],
) -> int:
    """Run, print, and return the status of the Python/Rust black-box comparison."""

    report = build_report(python_command, rust_command)
    for note in report.notes:
        print(f"NOTE [herdr-run] {note}")
    if report.failures:
        for failure in report.failures:
            print(f"DIVERGENCE [{failure}]")
        print(
            f"cross[herdr-run]: {len(report.failures)} divergence(s) "
            f"out of {report.checks} paired checks"
        )
        return 1
    print(f"cross[herdr-run]: OK - {report.checks} paired checks agree")
    return 0


def _decode_argv(encoded: str, option: str) -> list[str]:
    try:
        decoded: object = json.loads(encoded)
    except json.JSONDecodeError as error:
        raise ValueError(f"{option} must be valid JSON: {error}") from error
    if not isinstance(decoded, list):
        raise ValueError(f"{option} must decode to a JSON array of strings")
    values = cast(list[object], decoded)
    if not values or not all(isinstance(value, str) for value in values):
        raise ValueError(f"{option} must decode to a non-empty JSON array of strings")
    return [value for value in values if isinstance(value, str)]


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="black-box Python/Rust differential for herdr-run",
    )
    parser.add_argument(
        "--python-command-json",
        required=True,
        help='complete Python implementation argv as JSON, e.g. ["python3","-m","herdr_run"]',
    )
    parser.add_argument(
        "--rust-command-json",
        required=True,
        help='complete Rust implementation argv as JSON, e.g. ["rs/bin/herdr-run"]',
    )
    namespace = parser.parse_args(list(argv) if argv is not None else None)
    python_encoded = cast(str, namespace.python_command_json)
    rust_encoded = cast(str, namespace.rust_command_json)
    try:
        python_command = _decode_argv(python_encoded, "--python-command-json")
        rust_command = _decode_argv(rust_encoded, "--rust-command-json")
    except ValueError as error:
        parser.error(str(error))
    return compare_herdr_run(python_command, rust_command)


if __name__ == "__main__":
    raise SystemExit(main())
