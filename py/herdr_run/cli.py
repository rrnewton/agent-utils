"""``herdr-run`` command line.

Default shape, matching how an agent actually uses it::

    herdr-run <agent> '<command>'          # run; stdout/stderr/exit code are the command's own
    herdr-run '<command>'                  # agent inferred from $DG_AGENT_NAME

Subcommands (``check``, ``doctor``, ``config``, ``target``) are opt-in and never shadow an agent
name, because the agent argument is only interpreted as a subcommand when it is the FIRST token and
no command string follows.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from collections.abc import Sequence

from herdr_run import __version__
from herdr_run.allowlist import Admission, admit
from herdr_run.audit import audit_path, record, warn_if_spool_is_tracked
from herdr_run.client import HerdrClient
from herdr_run.config import Config, load_config
from herdr_run.errors import HerdrRunError, Refused, RunTimeout
from herdr_run.readiness import assess, infer_prompt_tail
from herdr_run.runner import execute, write_meta
from herdr_run.session import resolve_target, tab_label_for

__all__ = ["main", "build_parser"]

_SUBCOMMANDS = ("check", "doctor", "config", "target", "userguide")


def _default_agent(environ: dict[str, str]) -> str:
    """Infer the invoking agent from the environment ORC actually sets."""
    for key in ("HERDR_RUN_AGENT", "DG_AGENT_NAME", "ORC_AGENT_NAME"):
        value = environ.get(key, "").strip()
        if value:
            return value
    return "unknown-agent"


def build_parser() -> argparse.ArgumentParser:
    """Construct the argument parser."""
    parser = argparse.ArgumentParser(
        prog="herdr-run",
        description=(
            "Run an allowlisted command in a Herdr pane OUTSIDE the agent sandbox. "
            "Intended for git/gh network operations that the in-jail proxy allowlist blocks."
        ),
        epilog="Full documentation: herdr-run userguide",
    )
    parser.add_argument("--version", action="version", version=f"herdr-run {__version__}")
    parser.add_argument(
        "--userguide",
        action="store_true",
        help="print the full embedded user guide (the complete reference)",
    )
    parser.add_argument(
        "positional",
        nargs="*",
        metavar="ARG",
        help="Either '<agent> <command>', '<command>', or one of: " + ", ".join(_SUBCOMMANDS),
    )
    parser.add_argument("--config", metavar="PATH", help="Explicit config file (default: nearest .herdr-run.yaml)")
    parser.add_argument("--agent", metavar="NAME", help="Override the invoking agent name")
    parser.add_argument("--cwd", metavar="PATH", help="Working directory for the command")
    parser.add_argument("--timeout", type=float, metavar="SECONDS", help="Override the command timeout")
    parser.add_argument(
        "--wait-ready",
        type=float,
        metavar="SECONDS",
        help="Wait up to SECONDS for the pane to become idle instead of refusing immediately",
    )
    parser.add_argument(
        "--no-cache", action="store_true", help="Ignore the session cache and re-resolve from labels"
    )
    parser.add_argument("--json", action="store_true", help="Emit one JSON object instead of raw output")
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Check the command against the allowlist and print the rendered line; execute nothing",
    )
    return parser


def _load(args: argparse.Namespace, environ: dict[str, str]) -> Config:
    config = load_config(explicit_path=args.config, start_dir=os.getcwd())
    return config


def _client(config: Config, environ: dict[str, str]) -> HerdrClient:
    return HerdrClient(broker=config.broker, environ=environ)


def _resolved_cwd(config: Config, args: argparse.Namespace) -> str:
    cwd = args.cwd or config.cwd or config.project_root
    if not os.path.isabs(cwd):
        cwd = os.path.abspath(os.path.join(config.project_root, cwd))
    return cwd


def _cmd_config(config: Config, agent: str) -> int:
    document = {
        "source": config.source_path or "(built-in defaults)",
        "project_root": config.project_root,
        "workspace": config.workspace,
        "tab_name": config.tab_name,
        "tab_label": tab_label_for(config, agent),
        "allow": list(config.allow),
        "prefixes": list(config.prefixes),
        "deny_global": {key: list(value) for key, value in config.deny_global.items()},
        "deny_subcommand": {key: list(value) for key, value in config.deny_subcommand.items()},
        "deny_anywhere": list(config.deny_anywhere),
        "spool_dir": config.spool_dir,
        "timeout_seconds": config.timeout_seconds,
        "ready_timeout_seconds": config.ready_timeout_seconds,
        "readiness": config.readiness,
        "prompt_tail": config.prompt_tail,
        "broker": config.broker,
    }
    json.dump(document, sys.stdout, indent=2, sort_keys=True)
    sys.stdout.write("\n")
    return 0


def _cmd_check(config: Config, command: str) -> int:
    """Answer only the policy question, touching no pane and no Herdr server."""
    try:
        admission = admit(command, config)
    except Refused as exc:
        print(f"REFUSED: {exc}", file=sys.stderr)
        return Refused.exit_code
    print(f"ALLOWED: program={admission.program} subcommand={admission.subcommand or '-'}")
    print(f"rendered: {admission.rendered}")
    return 0


def _cmd_target(config: Config, agent: str, environ: dict[str, str], *, use_cache: bool) -> int:
    client = _client(config, environ)
    target = resolve_target(client, config, agent, use_cache=use_cache)
    prompt_tail = infer_prompt_tail(config)
    readiness = assess(client, target.pane_id, config, prompt_tail=prompt_tail)
    document = {
        "workspace": {"label": target.workspace_label, "id": target.workspace_id},
        "tab": {"label": target.tab_label, "id": target.tab_id},
        "pane_id": target.pane_id,
        "created": list(target.created),
        "from_cache": target.from_cache,
        "ready": readiness.ready,
        "readiness": readiness.describe(),
        "prompt_tail": prompt_tail,
    }
    json.dump(document, sys.stdout, indent=2, sort_keys=True)
    sys.stdout.write("\n")
    return 0 if readiness.ready else 0


def _cmd_doctor(config: Config, agent: str, environ: dict[str, str]) -> int:
    """Bracket the premise in BOTH directions instead of asserting it.

    Positive: the same command through the pane must succeed. Negative: run in-process (inside the
    jail) it must fail. If BOTH succeed the pane is not actually buying anything; if both fail the
    egress path is broken. Reporting one side only would leave "the pane is outside the sandbox" as
    an inference rather than an observation.
    """
    import subprocess

    probe = f"git ls-remote {config.probe_remote} HEAD"
    prefixed = f"with-proxy {probe}" if "with-proxy" in config.prefixes else probe

    print(f"herdr-run doctor  (agent={agent}, config={config.source_path or 'defaults'})")
    print()

    inside = subprocess.run(
        ["bash", "-lc", prefixed], text=True, capture_output=True, check=False, timeout=120
    )
    inside_ok = inside.returncode == 0
    print(f"[in-jail ] {prefixed}")
    print(f"           rc={inside.returncode} {'SUCCEEDED' if inside_ok else 'failed'}")
    if not inside_ok:
        print(f"           {(inside.stderr or '').strip().splitlines()[-1] if inside.stderr.strip() else ''}")

    client = _client(config, environ)
    target = resolve_target(client, config, agent)
    admission = admit(prefixed, config)
    outside_rc = -1
    try:
        result = execute(
            client,
            config,
            target,
            admission,
            agent=agent,
            cwd=_resolved_cwd(config, argparse.Namespace(cwd=None)),
            ready_timeout=max(config.ready_timeout_seconds, 30.0),
            timeout=min(config.timeout_seconds, 180.0),
        )
        outside_rc = result.exit_code
        head = (result.stdout.strip().splitlines() or [""])[0]
        print(f"[via pane] {prefixed}   (pane {target.pane_id}, tab {target.tab_label})")
        print(f"           rc={outside_rc} {'SUCCEEDED' if outside_rc == 0 else 'failed'}  {head[:80]}")
    except HerdrRunError as exc:
        print(f"[via pane] FAILED: {exc}")

    print()
    if outside_rc == 0 and not inside_ok:
        print("VERDICT: working as intended — blocked in-jail, succeeds through the pane.")
        return 0
    if outside_rc == 0 and inside_ok:
        print(
            "VERDICT: the pane works, but so does the in-jail path. herdr-run is not buying you "
            "anything here; prefer running the command directly."
        )
        return 0
    print(
        "VERDICT: the pane path is NOT working. Most likely the Herdr server was started from "
        "inside a sandboxed process, so its panes inherit the confinement. Stop it "
        "('herdr server stop') and let herdr-run restart it via systemd-run, or start it from an "
        "unconfined shell."
    )
    return 1


def _run_command(
    config: Config, agent: str, command: str, args: argparse.Namespace, environ: dict[str, str]
) -> int:
    log = audit_path(config.project_root, config.spool_dir)
    warn_if_spool_is_tracked(config.project_root, config.spool_dir)

    try:
        admission: Admission = admit(command, config)
    except Refused as exc:
        record(log, agent=agent, command=command, verdict="REFUSED", detail=str(exc))
        print(f"herdr-run: REFUSED: {exc}", file=sys.stderr)
        return Refused.exit_code

    if args.dry_run:
        record(log, agent=agent, command=command, verdict="DRY-RUN", detail=admission.rendered)
        if args.json:
            json.dump(
                {"verdict": "allowed", "program": admission.program, "rendered": admission.rendered},
                sys.stdout,
                indent=2,
                sort_keys=True,
            )
            sys.stdout.write("\n")
        else:
            print(admission.rendered)
        return 0

    client = _client(config, environ)
    cwd = _resolved_cwd(config, args)
    ready_timeout = args.wait_ready if args.wait_ready is not None else config.ready_timeout_seconds
    timeout = args.timeout if args.timeout is not None else config.timeout_seconds

    try:
        target = resolve_target(client, config, agent, use_cache=not args.no_cache)
        result = execute(
            client,
            config,
            target,
            admission,
            agent=agent,
            cwd=cwd,
            ready_timeout=ready_timeout,
            timeout=timeout,
        )
    except RunTimeout as exc:
        # Emit whatever the command managed to print BEFORE reporting the timeout. Dropping it would
        # make a partially-successful run look like one that produced nothing, and a caller cannot
        # distinguish a false empty result from a real one.
        record(log, agent=agent, command=command, verdict="RUNTIMEOUT", detail=str(exc))
        sys.stdout.write(exc.partial_stdout)
        sys.stderr.write(exc.partial_stderr)
        print(f"herdr-run: {exc}", file=sys.stderr)
        return exc.exit_code
    except HerdrRunError as exc:
        record(log, agent=agent, command=command, verdict=type(exc).__name__.upper(), detail=str(exc))
        print(f"herdr-run: {exc}", file=sys.stderr)
        return exc.exit_code

    meta = write_meta(result, admission, config, agent)
    record(
        log,
        agent=agent,
        command=command,
        verdict="RAN",
        detail=f"exit {result.exit_code} in {result.duration_seconds:.1f}s",
        fields={
            "run_id": result.run_id,
            "pane_id": result.target.pane_id,
            "tab": result.target.tab_label,
            "exit_code": result.exit_code,
            "rendered": admission.rendered,
            "meta": meta,
        },
    )

    if args.json:
        json.dump(
            {
                "exit_code": result.exit_code,
                "stdout": result.stdout,
                "stderr": result.stderr,
                "run_id": result.run_id,
                "pane_id": result.target.pane_id,
                "tab": result.target.tab_label,
                "created": list(result.target.created),
                "duration_seconds": round(result.duration_seconds, 3),
                "spool_dir": result.spool.directory,
                "meta": meta,
            },
            sys.stdout,
            indent=2,
            sort_keys=True,
        )
        sys.stdout.write("\n")
    else:
        sys.stdout.write(result.stdout)
        sys.stderr.write(result.stderr)
    return result.exit_code


def _print_userguide() -> int:
    try:
        from importlib.resources import files

        text = (files("herdr_run") / "USER_GUIDE.md").read_text(encoding="utf-8")
    except (OSError, ModuleNotFoundError):
        print("herdr-run: user guide resource is not available in this installation", file=sys.stderr)
        return 1
    sys.stdout.write(text)
    return 0


def main(argv: Sequence[str] | None = None) -> int:
    """Entry point. Returns the wrapped command's exit code, or a distinct herdr-run code."""
    parser = build_parser()
    # parse_intermixed_args, not parse_args: with a greedy `nargs="*"` positional, plain parse_args
    # cannot handle an option BETWEEN positionals, so `herdr-run agent --cwd DIR 'cmd'` failed with
    # "unrecognized arguments" — and that is the natural way to write the call.
    args = parser.parse_intermixed_args(list(argv) if argv is not None else None)
    environ = dict(os.environ)

    positional: list[str] = list(args.positional)
    subcommand: str | None = None
    if positional and positional[0] in _SUBCOMMANDS:
        subcommand = positional.pop(0)

    if subcommand == "userguide" or args.userguide:
        return _print_userguide()

    try:
        config = _load(args, environ)
    except HerdrRunError as exc:
        print(f"herdr-run: {exc}", file=sys.stderr)
        return exc.exit_code

    # Agent resolution: --agent wins, then a leading positional when a command follows it, then the
    # environment. Requiring TWO positionals for the '<agent> <command>' form is what keeps a single
    # quoted command from being mistaken for an agent name.
    #
    # When --agent is given explicitly, NO positional may be consumed as an agent name: every
    # positional belongs to the command. Consuming positional[0] anyway used to silently DELETE a
    # leading `with-proxy` wrapper from `herdr-run --agent X with-proxy '<cmd>'`. The command still
    # ran -- just without its proxy -- so `gh` dialled GitHub direct and failed with "network is
    # unreachable", which reads like an egress outage rather than a mangled argv. Anything past a
    # single quoted command is now the loose-words shape, and is refused below like any other.
    agent = args.agent or ""
    command: str | None = None
    if len(positional) == 2 and not args.agent:
        agent = positional[0]
        command = positional[1]
    elif len(positional) == 1:
        command = positional[0]
    elif len(positional) > 1:
        # REFUSE rather than re-join. Joining loose words and re-splitting them silently DESTROYS
        # quoting: `herdr-run agent git commit -m "two words"` would arrive as four arguments
        # (`-m`, `two`, `words`) instead of two, and the caller would never see that it happened.
        # A loud refusal is the only safe reading of an ambiguous invocation.
        joined = " ".join(positional)
        print(
            "herdr-run: pass the command as ONE quoted argument, not as separate words.\n"
            f"  you wrote:  herdr-run {joined}\n"
            f"  instead:    herdr-run --agent {agent or '<agent>'} "
            f"'{' '.join(positional[1:] if not args.agent else positional)}'\n"
            "Re-joining loose words would silently change the quoting of your arguments.",
            file=sys.stderr,
        )
        return 2
    if not agent:
        agent = _default_agent(environ)

    try:
        if subcommand == "config":
            return _cmd_config(config, agent)
        if subcommand == "target":
            return _cmd_target(config, agent, environ, use_cache=not args.no_cache)
        if subcommand == "doctor":
            return _cmd_doctor(config, agent, environ)
        if subcommand == "check":
            if command is None:
                print("herdr-run check: needs a command to check", file=sys.stderr)
                return 2
            return _cmd_check(config, command)
        if command is None:
            # Bare invocation prints help and succeeds, matching every other tool in this repo (the
            # `make check-deps` contract requires each entrypoint to start cleanly with no args).
            parser.print_help()
            print("\nNothing to do: no command given. See 'herdr-run userguide'.")
            return 0
        return _run_command(config, agent, command, args, environ)
    except HerdrRunError as exc:
        print(f"herdr-run: {exc}", file=sys.stderr)
        return exc.exit_code
    except KeyboardInterrupt:  # pragma: no cover - interactive only
        print("herdr-run: interrupted", file=sys.stderr)
        return 130


if __name__ == "__main__":  # pragma: no cover
    raise SystemExit(main())
