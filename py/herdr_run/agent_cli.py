"""Command line for durable interactive-agent messaging through Herdr."""

from __future__ import annotations

import argparse
import json
import os
import sys
from collections.abc import Sequence

from herdr_run.agent import Target, drain, read, send, status
from herdr_run.client import HerdrClient
from herdr_run.errors import HerdrRunError


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="herdr-agent",
        description="Queue, submit, inspect, and read interactive agents hosted in Herdr panes.",
        allow_abbrev=False,
    )
    parser.add_argument("command", choices=("send", "drain", "status", "read", "userguide"))
    parser.add_argument("text", nargs="?")
    parser.add_argument("--file", help="read a message from this artifact")
    parser.add_argument("--pane")
    parser.add_argument("--session-agent")
    parser.add_argument("--session")
    parser.add_argument("--agent", dest="expected_agent")
    parser.add_argument("--workspace", dest="expected_workspace")
    parser.add_argument("--cwd", dest="expected_cwd")
    parser.add_argument("--queue", default=".herdr-agent")
    parser.add_argument("--ready-timeout", type=float, default=900.0)
    parser.add_argument("--working-timeout", type=float, default=30.0)
    parser.add_argument("--max-attempts", type=int, default=3)
    parser.add_argument("--lines", type=int, default=500)
    parser.add_argument("--herdr-bin", default="herdr")
    return parser


def _target(args: argparse.Namespace) -> Target:
    return Target(
        pane_id=args.pane,
        session_agent=args.session_agent,
        session_value=args.session,
        expected_agent=args.expected_agent,
        expected_workspace=args.expected_workspace,
        expected_cwd=args.expected_cwd,
    )


def _message(args: argparse.Namespace) -> str:
    if args.file is not None:
        if args.text is not None:
            raise ValueError("pass message text or --file, not both")
        try:
            with open(args.file, encoding="utf-8") as handle:
                return handle.read()
        except (OSError, UnicodeError) as exc:
            raise ValueError(f"cannot read message artifact {args.file}: {exc}") from exc
    if args.text is None:
        raise ValueError("send needs message text or --file")
    return str(args.text)


def _guide() -> int:
    from importlib.resources import files

    try:
        text = (files("herdr_run") / "AGENT_USER_GUIDE.md").read_text(encoding="utf-8")
    except (OSError, ModuleNotFoundError) as exc:
        print(f"herdr-agent: user guide unavailable: {exc}", file=sys.stderr)
        return 1
    sys.stdout.write(text)
    return 0


def main(argv: Sequence[str] | None = None) -> int:
    parser = _parser()
    args = parser.parse_intermixed_args(list(argv) if argv is not None else None)
    if args.command == "userguide":
        return _guide()
    if args.ready_timeout < 0 or args.working_timeout <= 0 or args.max_attempts <= 0 or args.lines <= 0:
        parser.error("timeouts, max-attempts, and lines must be positive (ready-timeout may be zero)")
    client = HerdrClient(herdr_bin=str(args.herdr_bin))
    target = _target(args)
    queue = os.path.abspath(str(args.queue))
    try:
        if args.command == "send":
            result = send(
                client, target, queue, _message(args), ready_timeout=args.ready_timeout,
                working_timeout=args.working_timeout, max_attempts=args.max_attempts,
            )
            json.dump(result.__dict__, sys.stdout, indent=2, sort_keys=True)
            sys.stdout.write("\n")
            return 0
        if args.command == "drain":
            result = drain(
                client, target, queue, ready_timeout=args.ready_timeout,
                working_timeout=args.working_timeout, max_attempts=args.max_attempts,
            )
            json.dump(result.__dict__, sys.stdout, indent=2, sort_keys=True)
            sys.stdout.write("\n")
            return 0 if not result.quarantined and result.blocked is None else 75
        if args.command == "status":
            json.dump(status(client, target, queue), sys.stdout, indent=2, sort_keys=True)
            sys.stdout.write("\n")
            return 0
        sys.stdout.write(read(client, target, lines=args.lines))
        return 0
    except (HerdrRunError, ValueError) as exc:
        print(f"herdr-agent: {exc}", file=sys.stderr)
        return exc.exit_code if isinstance(exc, HerdrRunError) else 2


if __name__ == "__main__":
    raise SystemExit(main())
