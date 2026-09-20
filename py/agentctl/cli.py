#!/usr/bin/env python3
"""Command-line control of persistent coding-agent sessions."""
from __future__ import annotations

import argparse
import json
import math
import sys
from collections.abc import Sequence
from importlib.resources import files
from pathlib import Path

if __package__ in (None, ""):
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from agentctl import __version__
from agentctl.client import HerdrClient
from agentctl.errors import AgentPending, AgentPossiblySubmitted, HerdrRunError
from agentctl.legacy_cli import _ascii_float, _bounded_uint
from agentctl.sessions import Sessions


def guide(document: str) -> int:
    """Print documentation embedded in the installed package, offline."""
    sys.stdout.write(files("agentctl").joinpath(document).read_text(encoding="utf-8"))
    return 0


def _common(parser: argparse.ArgumentParser, *, inherited: bool = False) -> None:
    default: object = argparse.SUPPRESS if inherited else ".agentctl"
    parser.add_argument("--registry", "--state", default=default, metavar="DIR",
        help="private session registry (default: .agentctl); use an existing registry to retain its sessions")
    parser.add_argument("--herdr-bin", default=argparse.SUPPRESS if inherited else "herdr", metavar="PATH",
        help="Herdr executable for interactive control (default: installed herdr)")


def _message(parser: argparse.ArgumentParser) -> None:
    group = parser.add_mutually_exclusive_group()
    group.add_argument("text", nargs="?", metavar="TEXT", help="literal instruction text; quote spaces and shell characters")
    group.add_argument("--file", metavar="PATH", help="read the instruction from a UTF-8 file")


def _delivery(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--ready-timeout", type=_ascii_float, default=900.0, metavar="SECONDS",
        help="interactive only: maximum readiness wait; 0 leaves busy work queued (default: 900 seconds)")
    parser.add_argument("--working-timeout", type=_ascii_float, default=30.0, metavar="SECONDS",
        help="interactive only: wait for evidence that input was accepted (default: 30 seconds)")
    parser.add_argument("--max-attempts", type=_bounded_uint, default=3, metavar="COUNT",
        help="interactive only: delivery attempt limit; uncertain input is never replayed automatically (default: 3)")


def parser() -> argparse.ArgumentParser:
    """Build real subcommands with operation-specific options and examples."""
    root = argparse.ArgumentParser(prog="agentctl", allow_abbrev=False,
        description="Start persistent coding agents, delegate follow-up work, and keep their terminals accessible.",
        epilog=("Start here: agentctl quickstart\n"
            "Examples:\n"
            "  agentctl start reviewer --cwd . --brief 'Review the current changes'\n"
            "  agentctl adopt reviewer --pane w1:p2 --workspace project --cwd /work/project --harness codex"),
        formatter_class=argparse.RawDescriptionHelpFormatter)
    _common(root)
    root.add_argument("--version", action="version", version=f"agentctl {__version__}")
    root.add_argument("--userguide", action="store_true", help="print the complete installed guide and exit")
    commands = root.add_subparsers(dest="command", metavar="COMMAND")

    def command(name: str, purpose: str, example: str, *, named: bool = False) -> argparse.ArgumentParser:
        child = commands.add_parser(name, help=purpose, description=purpose, allow_abbrev=False,
            epilog=f"Example: {example}", formatter_class=argparse.RawDescriptionHelpFormatter)
        _common(child, inherited=True)
        if named:
            child.add_argument("name", metavar="NAME", help="registered session name (lowercase letters, digits, hyphens)")
        return child

    start = command("start", "Start a named interactive agent or a resumable headless worker.",
        "agentctl start reviewer --harness codex --cwd . --brief 'Review this change'", named=True)
    start.add_argument("--cwd", default=".", metavar="DIR", help="worker working directory (default: current directory)")
    start.add_argument("--mode", choices=("interactive", "headless"), default="interactive",
        help="interactive keeps a native TUI; headless runs resumable structured turns (default: interactive)")
    start.add_argument("--backend", choices=("herdr", "tmux"), default="herdr",
        help="terminal host; interactive requires Herdr (default: herdr)")
    start.add_argument("--harness", default="codex", metavar="KIND",
        help="native harness: codex/claude interactive, codex/agy headless (default: codex)")
    start.add_argument("--model", metavar="MODEL", help="native model name; omission preserves the harness default")
    start.add_argument("--resume", metavar="SESSION", help="resume an explicit native conversation (interactive only)")
    start.add_argument("--harness-arg", action="append", default=[], metavar="ARG",
        help="literal interactive harness argument; repeat and use = for flags")
    start.add_argument("--workspace-id", metavar="ID", help="interactive only: exact Herdr workspace; default: current workspace or a shared subagents workspace")
    first = start.add_mutually_exclusive_group()
    first.add_argument("--brief", metavar="TEXT", help="initial task, submitted after launch")
    first.add_argument("--file", metavar="PATH", help="UTF-8 file containing the initial task")
    start.add_argument("--startup-timeout", type=_ascii_float, default=30.0, metavar="SECONDS",
        help="interactive startup deadline, greater than 0 and at most 300 (default: 30)")
    _delivery(start)

    adopt = command("adopt", "Register an existing Herdr agent without taking ownership of its runtime.",
        "agentctl adopt reviewer --pane w1:p2 --workspace project --cwd /work/project --harness codex",
        named=True)
    adopt.add_argument("--pane", required=True, metavar="ID",
        help="exact live Herdr pane containing the agent (required)")
    adopt.add_argument("--workspace", required=True, metavar="LABEL",
        help="expected live Herdr workspace label; a mismatch is refused (required)")
    adopt.add_argument("--cwd", required=True, metavar="DIR",
        help="expected live agent working directory; compared canonically (required)")
    adopt.add_argument("--harness", required=True, metavar="KIND",
        help="expected live harness kind, for example codex or claude (required)")
    adopt.add_argument("--session", metavar="ID",
        help="optional stable native conversation ID already reported by this exact pane")

    command("list", "List every registered session, including unavailable and failed launches.", "agentctl list")
    command("capabilities", "Show the adapters and services available in this installation.", "agentctl capabilities")
    command("status", "Inspect saved identity, runtime state, and supported operations.", "agentctl status reviewer", named=True)
    send = command("send", "Submit follow-up work; uncertain delivery is retained for inspection.",
        "agentctl send reviewer 'Please check the cancellation path too'", named=True)
    _message(send)
    _delivery(send)
    send.add_argument("--message-id", metavar="ID", help="caller-selected interactive request ID; duplicate IDs are rejected")
    send.add_argument("--model", metavar="MODEL", help="model override for this headless turn only")
    drain = command("drain", "Deliver safely pending interactive requests; never replay uncertain submissions.",
        "agentctl drain reviewer --ready-timeout 0", named=True)
    _delivery(drain)
    read = command("read", "Read a terminal snapshot or a headless transcript/answer.",
        "agentctl read reviewer --lines 100", named=True)
    read.add_argument("--lines", type=_bounded_uint, default=500, metavar="COUNT", help="maximum tail/snapshot lines (default: 500)")
    read.add_argument("--output", choices=("tail", "all", "last", "since_turn"), default="tail",
        help="output boundary; last and since_turn require headless transcripts (default: tail)")
    read.add_argument("--since-turn", type=_bounded_uint, metavar="NUMBER", help="first included turn; requires --output since_turn")
    wait = command("wait", "Wait for readiness; readiness does not mean the goal is complete.",
        "agentctl wait reviewer --timeout 60", named=True)
    wait.add_argument("--timeout", type=_ascii_float, default=900.0, metavar="SECONDS", help="readiness deadline (default: 900 seconds)")
    for name, purpose in (
        ("stop", "Stop an owned runtime, or unregister an adopted runtime without closing it, and archive state."),
        ("attach", "Focus the session's terminal for direct inspection and interaction."),
        ("pause", "Pause automated input while allowing an active turn to finish."),
        ("resume", "Allow automated input after human interaction; clear any unfinished composer draft first."),
        ("reset", "Clear an idle headless worker's conversation context."),
        ("repair", "Recreate the terminal presentation of a live headless runner."),
    ):
        command(name, purpose, f"agentctl {name} reviewer", named=True)
    goal = command("goal", "Inspect a native interactive goal when supported, or submit a goal instruction.",
        "agentctl goal reviewer 'Finish reviewing cancellation behavior'", named=True)
    _message(goal)
    _delivery(goal)
    goal.add_argument("--goal-command-json", metavar="JSON", help="native goal RPC command as a nonempty JSON array of arguments")
    bind = command("bind-session", "Bind an explicitly reported interactive conversation ID for goal inspection.",
        "agentctl bind-session reviewer SESSION_ID", named=True)
    bind.add_argument("session", metavar="SESSION", help="native conversation ID reported by the harness")
    bind.add_argument("--goal-command-json", metavar="JSON", help="native goal RPC command as a nonempty JSON array of arguments")
    migrate = command("migrate", "Move an idle headless worker while preserving its conversation and queue.",
        "agentctl migrate worker --backend herdr", named=True)
    migrate.add_argument("--backend", choices=("herdr", "tmux"), required=True, help="destination terminal host")
    migrate.add_argument("--mode", choices=("interactive", "headless"), help="destination execution mode; default: retain current mode")
    command("quickstart", "Print a short working setup, available offline.", "agentctl quickstart")
    command("userguide", "Print the complete installed operator guide.", "agentctl userguide")
    command("chat", "Connect Google Chat to one coordinator; use agentctl chat --help.", "agentctl chat --help")
    command("mcp", "Serve the same session operations over MCP stdio.", "agentctl mcp")
    return root


def _text(args: argparse.Namespace, *, optional: bool = False) -> str | None:
    path = getattr(args, "file", None)
    value = Path(path).read_text(encoding="utf-8") if path is not None else getattr(args, "text", None)
    if value is None and not optional:
        raise ValueError("supply instruction TEXT or --file PATH")
    if value is not None and (not isinstance(value, str) or not value.strip()):
        raise ValueError("instruction must not be empty")
    return value


def _goal_command(args: argparse.Namespace) -> list[str] | None:
    if args.goal_command_json is None:
        return None
    value: object = json.loads(args.goal_command_json)
    if not isinstance(value, list) or not value or any(not isinstance(item, str) or not item or "\0" in item for item in value):
        raise ValueError("goal command must be a nonempty JSON array of argument strings")
    return [str(item) for item in value]


def main(argv: Sequence[str] | None = None) -> int:
    """Run the unified interface with explicit outcome exit statuses."""
    arguments = list(sys.argv[1:] if argv is None else argv)
    command_index = 0
    while command_index < len(arguments):
        argument = arguments[command_index]
        if argument in ("--registry", "--state", "--herdr-bin"):
            command_index += 2
        elif any(argument.startswith(option + "=") for option in ("--registry", "--state", "--herdr-bin")):
            command_index += 1
        else:
            break
    if command_index < len(arguments) and arguments[command_index] == "chat":
        settings = argparse.ArgumentParser(prog="agentctl", add_help=False, allow_abbrev=False)
        _common(settings)
        prefix = settings.parse_args(arguments[:command_index])
        if prefix.herdr_bin != "herdr":
            settings.error("--herdr-bin applies to session commands; Chat uses its configured target adapter")
        from agentctl.chat import run_cli as chat_main
        return chat_main(arguments[command_index + 1:], default_state=Path(prefix.registry) / ".chat")
    root = parser()
    args = root.parse_args(arguments)
    if args.userguide or args.command == "userguide":
        return guide("USER_GUIDE.md")
    if args.command == "quickstart":
        return guide("QUICKSTART.md")
    if args.command is None:
        root.print_help()
        return 0
    if args.command == "capabilities":
        print(json.dumps({"interactive": {"backends": ["herdr"], "harnesses": ["codex", "claude"]},
            "headless": {"backends": ["herdr", "tmux"], "harnesses": ["codex", "agy"]},
            "services": ["chat", "mcp"], "registry": args.registry}, indent=2, sort_keys=True))
        return 0
    if args.command == "mcp":
        from agentctl.mcp import _serve
        return _serve(args.registry, args.herdr_bin)
    try:
        for key in ("ready_timeout", "working_timeout", "startup_timeout", "timeout"):
            value = getattr(args, key, None)
            if value is not None and (not math.isfinite(value) or value < 0 or value > 31_536_000
                or (key in ("working_timeout", "startup_timeout") and value == 0)):
                raise ValueError(f"{key.replace('_', '-')} must be finite and within its documented range")
        for key in ("max_attempts", "lines"):
            if getattr(args, key, 1) <= 0:
                raise ValueError(f"{key.replace('_', '-')} must be positive")
        sessions = Sessions(HerdrClient(herdr_bin=args.herdr_bin), args.registry)
        options: dict[str, object] = {key: getattr(args, key) for key in
            ("ready_timeout", "working_timeout", "max_attempts") if hasattr(args, key)}
        result: object
        name = getattr(args, "name", "")
        if args.command == "start":
            brief = Path(args.file).read_text(encoding="utf-8") if args.file else args.brief
            result = sessions.start_session(name, cwd=args.cwd, mode=args.mode, backend=args.backend,
                harness=args.harness, model=args.model, brief=brief, resume=args.resume,
                harness_args=args.harness_arg, workspace_id=args.workspace_id,
                startup_timeout=args.startup_timeout, ready_timeout=args.ready_timeout,
                working_timeout=args.working_timeout, max_attempts=args.max_attempts)
        elif args.command == "adopt":
            result = sessions.adopt(name, pane_id=args.pane,
                expected_workspace=args.workspace, expected_cwd=args.cwd,
                harness=args.harness, session=args.session)
        elif args.command == "list":
            result = sessions.list()
        elif args.command == "status":
            result = sessions.status(name)
        elif args.command == "send":
            result = sessions.send_session(name, _text(args) or "", message_id=args.message_id, model=args.model, **options)
        elif args.command == "read":
            sys.stdout.write(sessions.read_session(name, lines=args.lines, output=args.output, since_turn=args.since_turn))
            return 0
        elif args.command == "wait":
            result = sessions.wait(name, timeout=args.timeout)
        elif args.command == "drain":
            from dataclasses import asdict
            outcome = sessions.drain(name, **options)
            print(json.dumps(asdict(outcome), indent=2, sort_keys=True))
            return 75 if outcome.blocked else (76 if outcome.quarantined else 0)
        elif args.command == "goal":
            result = sessions.goal(name, _text(args, optional=True), goal_command=_goal_command(args), **options)
        elif args.command == "bind-session":
            result = sessions.bind_session(name, args.session, goal_command=_goal_command(args))
        elif args.command in ("pause", "resume"):
            result = sessions.pause(name, paused=args.command == "pause")
        elif args.command == "attach":
            result = sessions.attach(name)
        elif args.command == "stop":
            result = sessions.stop(name)
        elif args.command in ("reset", "repair"):
            result = sessions.runtime_operation(name, args.command)
        elif args.command == "migrate":
            result = sessions.runtime_operation(name, "migrate", backend=args.backend, mode=args.mode)
        else:
            raise ValueError(f"unsupported command: {args.command}")
        print(json.dumps(result, indent=2, sort_keys=True))
        return 0
    except (AgentPending, AgentPossiblySubmitted) as exc:
        print(json.dumps({"outcome": exc.outcome, "message_id": exc.message_id,
            "artifact": exc.artifact, "error": str(exc), "safe_to_retry": isinstance(exc, AgentPending)}, sort_keys=True))
        return exc.exit_code
    except (ValueError, TypeError, OSError, HerdrRunError) as exc:
        print(f"agentctl: {exc}", file=sys.stderr)
        return exc.exit_code if isinstance(exc, HerdrRunError) else 2


if __name__ == "__main__":
    raise SystemExit(main())
