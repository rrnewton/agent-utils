"""Command line for run-agent-prompt: one prompt, first harness that answers."""

from __future__ import annotations

import argparse
import json
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
from collections.abc import Sequence
from importlib.resources import files
from pathlib import Path

from run_agent_prompt import __version__
from run_agent_prompt.attempts import (
    EFFORT_LEVELS,
    AttemptResult,
    attempt_claude,
    attempt_codex,
    attempt_muse,
    child_env,
)
from run_agent_prompt.config import Config, Profile, load_config, load_profiles
from run_agent_prompt.process import (
    BackendProcesses,
    TerminationSignal,
    defer_signals_during_cleanup,
    termination_signals_as_exceptions,
)

ALL_HARNESSES = ("muse", "claude", "codex")
ALL_FAILED_EXIT = 69


def _load_userguide() -> str:
    """Return the complete embedded operator guide."""

    return (files("run_agent_prompt") / "USER_GUIDE.md").read_text(encoding="utf-8")


def _load_quickstart() -> str:
    """Return the embedded one-screen introduction."""

    return (files("run_agent_prompt") / "QUICKSTART.md").read_text(encoding="utf-8")


def _command(value: str | None, default: str) -> list[str]:
    parsed = shlex.split(value) if value else [default]
    return parsed or [default]


def build_parser() -> argparse.ArgumentParser:
    """Build the run-agent-prompt argument parser."""

    parser = argparse.ArgumentParser(
        prog="run-agent-prompt",
        description=(
            "Run one judgement-only prompt through the first agent harness "
            "that answers (muse, then claude, then codex) and print only "
            "its final answer. Attempts are tool-free or read-confined, individually timed "
            "out, and logged to stderr without prompt or answer text."
        ),
        epilog=(
            "Examples:\n"
            "  run-agent-prompt quickstart\n"
            "  run-agent-prompt --userguide\n"
            "  run-agent-prompt --prompt-file prompt.txt\n"
            "  cat prompt.txt | run-agent-prompt --timeout 120\n"
            "  run-agent-prompt --only claude --claude-model MODEL --prompt-file p.txt\n\n"
            "Exit codes: 0 answered, 2 usage/config error, 69 every harness failed, "
            "129 SIGHUP, 130 SIGINT, 143 SIGTERM."
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument(
        "--version",
        action="version",
        version=f"%(prog)s {__version__}",
        help="show the run-agent-prompt version and exit",
    )
    parser.add_argument(
        "--userguide",
        action="store_true",
        help="print the complete embedded operator guide and exit",
    )
    parser.add_argument(
        "--prompt-file",
        type=Path,
        metavar="PATH",
        help="read the prompt from PATH instead of stdin (it never appears in argv)",
    )
    parser.add_argument(
        "--config",
        type=Path,
        metavar="PATH",
        help="config TOML (default: ~/.config/agent-utils/run-agent-prompt.toml)",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=300.0,
        metavar="SECONDS",
        help="per-attempt timeout in seconds (default: 300)",
    )
    parser.add_argument(
        "--total-timeout",
        type=float,
        default=900.0,
        metavar="SECONDS",
        help="whole-chain deadline in seconds across attempts (default: 900)",
    )
    parser.add_argument(
        "--cwd",
        type=Path,
        metavar="DIR",
        help="run attempts in DIR instead of a fresh temporary workspace",
    )
    parser.add_argument(
        "--log-file",
        type=Path,
        metavar="PATH",
        help="also append one JSONL record per attempt to PATH",
    )
    parser.add_argument(
        "--profile-file",
        type=Path,
        metavar="PATH",
        help="ordered recovery profiles TOML (schema agentctl-recovery-profiles/v1); "
        "its [[profile]] order is the attempt order, replacing the built-in order",
    )
    parser.add_argument(
        "--profile",
        action="append",
        metavar="NAME",
        help="with --profile-file, try only the named profile (repeatable); "
        "an unknown or disabled name is a usage error",
    )
    parser.add_argument(
        "--only",
        choices=ALL_HARNESSES,
        metavar="HARNESS",
        help="try only this harness (muse, claude, or codex)",
    )
    parser.add_argument(
        "--effort",
        metavar="LEVEL",
        help="shared reasoning effort (default: config value, else low)",
    )
    for harness in ALL_HARNESSES:
        parser.add_argument(
            f"--{harness}-model",
            metavar="MODEL",
            help=f"model for the {harness} attempt (default: config value, else harness default)",
        )
        parser.add_argument(
            f"--{harness}-effort",
            metavar="LEVEL",
            help=f"effort for the {harness} attempt, overriding --effort",
        )
        parser.add_argument(
            f"--{harness}-command",
            metavar="COMMAND",
            help=f"{harness} executable or command line (primarily for testing/wrappers)",
        )
    return parser


def _attempt_line(result: AttemptResult, effort: str) -> str:
    exit_text = "-" if result.exit_code is None else str(result.exit_code)
    return (
        f"run-agent-prompt: harness={result.harness} status={result.status} "
        f"exit={exit_text} duration_s={result.duration_s:.3f} "
        f"model={result.model or 'unknown'} effort={effort}"
    )


def _log_attempt(
    result: AttemptResult,
    effort: str,
    log_file: Path | None,
    profile: str | None = None,
) -> None:
    line = _attempt_line(result, effort)
    if profile is not None:
        line = line.replace("run-agent-prompt: ", f"run-agent-prompt: profile={profile} ", 1)
    print(line, file=sys.stderr, flush=True)
    if log_file is not None:
        record = {
            "event": "attempt",
            "harness": result.harness,
            "status": result.status,
            "exit": result.exit_code,
            "duration_s": round(result.duration_s, 3),
            "model": result.model,
            "effort": effort,
            "profile": profile,
        }
        log_file.parent.mkdir(parents=True, exist_ok=True)
        with log_file.open("a", encoding="utf-8") as handle:
            handle.write(json.dumps(record, separators=(",", ":")) + "\n")


def _resolve(
    harness: str,
    args: argparse.Namespace,
    config: Config,
) -> tuple[str | None, str]:
    """Model and effort for one harness: flags, then config, then defaults."""

    model = getattr(args, f"{harness}_model") or config.models.get(harness)
    effort = (
        getattr(args, f"{harness}_effort")
        or args.effort
        or config.efforts.get(harness)
        or config.effort
        or "low"
    )
    return model, effort


def main(argv: Sequence[str] | None = None) -> int:
    """Run the fallback chain; print only the winning answer."""

    raw_argv = list(sys.argv[1:] if argv is None else argv)
    if raw_argv == ["quickstart"]:
        sys.stdout.write(_load_quickstart())
        return 0
    if raw_argv == ["userguide"]:
        sys.stdout.write(_load_userguide())
        return 0
    parser = build_parser()
    args = parser.parse_args(raw_argv)
    if args.userguide:
        sys.stdout.write(_load_userguide())
        return 0
    if args.timeout <= 0 or args.total_timeout <= 0:
        parser.error("--timeout and --total-timeout must be positive")
    try:
        config = load_config(args.config)
    except ValueError as error:
        parser.error(str(error))
    profiles: list[Profile] | None = None
    if args.profile_file is not None:
        try:
            profiles = load_profiles(args.profile_file)
        except ValueError as error:
            parser.error(str(error))
        if args.profile:
            known = {profile.name: profile for profile in profiles}
            selected: list[Profile] = []
            for name in args.profile:
                profile = known.get(name)
                if profile is None or profile.disabled:
                    parser.error(f"profile {name} is unknown or disabled")
                selected.append(profile)
            profiles = selected
        else:
            profiles = [profile for profile in profiles if not profile.disabled]
            if not profiles:
                parser.error("profile file enables no profile")
    if args.prompt_file is not None:
        try:
            prompt = args.prompt_file.read_text(encoding="utf-8")
        except OSError as error:
            parser.error(f"could not read prompt file: {error}")
    else:
        prompt = sys.stdin.read()
    if not prompt.strip():
        parser.error("prompt is empty")
    if args.cwd is not None and not args.cwd.is_dir():
        parser.error(f"--cwd is not a directory: {args.cwd}")

    if profiles is not None:
        plans: list[tuple[str, Profile | None]] = [(p.harness, p) for p in profiles]
    elif args.only:
        plans = [(args.only, None)]
    else:
        plans = [(harness, None) for harness in ALL_HARNESSES]
    env = child_env()
    deadline = time.monotonic() + args.total_timeout
    processes = BackendProcesses()
    temporary: tempfile.TemporaryDirectory[str] | None = None
    try:
        with termination_signals_as_exceptions():
            blocked_signals = {
                signum
                for signum in (
                    signal.SIGINT,
                    getattr(signal, "SIGTERM", None),
                    getattr(signal, "SIGHUP", None),
                )
                if isinstance(signum, int)
            }
            previous_mask: set[signal.Signals | int] | None = None
            if threading.current_thread() is threading.main_thread():
                previous_mask = signal.pthread_sigmask(
                    signal.SIG_BLOCK, blocked_signals
                )
            try:
                temporary = tempfile.TemporaryDirectory(prefix="run-agent-prompt-")
            finally:
                if previous_mask is not None:
                    signal.pthread_sigmask(signal.SIG_SETMASK, previous_mask)
            scratch = Path(temporary.name)
            for harness, profile in plans:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    result = AttemptResult(harness, "total-timeout", None, 0.0)
                    _log_attempt(result, _resolve(harness, args, config)[1], args.log_file, profile.name if profile else None)
                    continue
                model, effort = _resolve(harness, args, config)
                if profile is not None:
                    if getattr(args, f"{harness}_model") is None and profile.model:
                        model = profile.model
                    if (
                        getattr(args, f"{harness}_effort") is None
                        and args.effort is None
                        and profile.effort
                    ):
                        effort = profile.effort
                attempt_timeout = (
                    profile.timeout_seconds
                    if profile is not None and profile.timeout_seconds
                    else args.timeout
                )
                command = (
                    [profile.command]
                    if profile is not None and profile.command
                    else _command(getattr(args, f"{harness}_command"), harness)
                )
                if effort not in EFFORT_LEVELS[harness]:
                    result = AttemptResult(
                        harness, "invalid-effort", None, 0.0, model=model
                    )
                    _log_attempt(result, effort, args.log_file, profile.name if profile else None)
                    continue
                if shutil.which(command[0], path=env.get("PATH")) is None and not Path(
                    command[0]
                ).is_file():
                    result = AttemptResult(
                        harness, "spawn-error", None, 0.0, model=model
                    )
                    _log_attempt(result, effort, args.log_file, profile.name if profile else None)
                    continue
                timeout = min(attempt_timeout, remaining)
                attempt_scratch = scratch / harness
                workspace = args.cwd or attempt_scratch / "workspace"
                attempt_scratch.mkdir(parents=True, exist_ok=True)
                workspace.mkdir(parents=True, exist_ok=True)
                if harness == "codex" and args.cwd is None:
                    try:
                        subprocess.run(
                            ["git", "init", "--quiet"],
                            cwd=workspace,
                            env=env,
                            stdout=subprocess.DEVNULL,
                            stderr=subprocess.DEVNULL,
                            check=False,
                            timeout=min(10.0, timeout),
                        )
                    except (OSError, subprocess.TimeoutExpired):
                        pass
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        result = AttemptResult(harness, "total-timeout", None, 0.0)
                        _log_attempt(
                            result, effort, args.log_file, profile.name if profile else None
                        )
                        continue
                    timeout = min(attempt_timeout, remaining)
                if harness == "muse":
                    result = attempt_muse(
                        processes,
                        command,
                        prompt=prompt,
                        prompt_path=attempt_scratch / "prompt.txt",
                        workspace=workspace,
                        model=model,
                        effort=effort,
                        timeout_seconds=timeout,
                        env=env,
                    )
                elif harness == "claude":
                    result = attempt_claude(
                        processes,
                        command,
                        prompt=prompt,
                        workspace=workspace,
                        model=model,
                        effort=effort,
                        timeout_seconds=timeout,
                        env=env,
                    )
                else:
                    result = attempt_codex(
                        processes,
                        command,
                        prompt=prompt,
                        workspace=workspace,
                        last_message_path=attempt_scratch / "codex-last-message.txt",
                        model=model,
                        effort=effort,
                        timeout_seconds=timeout,
                        env=env,
                    )
                _log_attempt(result, effort, args.log_file, profile.name if profile else None)
                if result.status == "answered" and result.answer is not None:
                    sys.stdout.write(result.answer)
                    if not result.answer.endswith("\n"):
                        sys.stdout.write("\n")
                    return 0
    except KeyboardInterrupt:
        return 130
    except TerminationSignal as signal_error:
        return 128 + signal_error.signum
    finally:
        with defer_signals_during_cleanup():
            try:
                processes.terminate_all()
            finally:
                if temporary is not None:
                    temporary.cleanup()
    print("run-agent-prompt: every harness failed", file=sys.stderr, flush=True)
    return ALL_FAILED_EXIT


__all__ = ["ALL_FAILED_EXIT", "build_parser", "main"]
