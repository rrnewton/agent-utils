"""``herdr-run`` command line.

The surface is a plain multiplexed CLI::

    herdr-run [GLOBAL OPTIONS] <subcommand> [OPTIONS]

Options that identify the invocation and its configuration are global and go before the
subcommand; options that only mean something to one subcommand go after it. Neither level
documents nor accepts the other's options, so ``--help`` at each level describes exactly the
options that level takes.
"""

from __future__ import annotations

import base64
import json
import math
import os
import shlex
import sys
from collections.abc import Sequence
from dataclasses import dataclass, replace

from herdr_run import __version__
from herdr_run.allowlist import Admission, admit
from herdr_run.audit import audit_path, record, warn_if_spool_is_tracked
from herdr_run.client import HerdrClient, _bounded_control_command
from herdr_run.config import MAX_TIMEOUT_SECONDS, Config, load_config
from herdr_run.errors import (
    EXIT_BUSY,
    ConfigError,
    HerdrRunError,
    HerdrUnavailable,
    Refused,
    RunTimeout,
)
from herdr_run.readiness import assess, infer_prompt_tail
from herdr_run.runner import RunResult, execute, read_output_bytes, write_meta
from herdr_run.session import resolve_target, tab_label_for
from herdr_run.sweep import sweep

__all__ = ["main", "help_text", "subcommand_help_text", "usage_line"]


#: Every subcommand, in the order ``herdr-run --help`` lists them, with its one-line summary.
#:
#: Listed by what a reader is most likely to want first rather than alphabetically: the action,
#: then the two questions asked about it, then setup, then the reports.
SUBCOMMANDS: tuple[tuple[str, str], ...] = (
    ("run", "run one allowlisted command in a pane and return its result"),
    ("check", "say whether a command would be admitted; touch no pane"),
    (
        "status",
        "report the configuration, policy, and session in effect; change nothing",
    ),
    ("init", "write an annotated .herdr-run.yaml in this directory"),
    ("config", "print the fully resolved configuration as JSON"),
    ("target", "resolve this agent's pane and print its ids and readiness"),
    ("reap", "report which command tabs are provably finished with"),
    ("net-doctor", "smoke-test one scenario: a caller whose own network is blocked"),
    ("quickstart", "print the one-screen introduction"),
    ("userguide", "print the complete reference"),
)

#: Packaged document each documentation subcommand prints.
_DOCUMENTS: dict[str, str] = {
    "quickstart": "QUICKSTART.md",
    "userguide": "USER_GUIDE.md",
}

_GLOBAL_OPTIONS = ("--help", "-h", "--version", "--json", "--config", "--agent")


class _UsageError(Exception):
    """A command line this surface cannot accept, carrying the message to print."""


class _EarlyExit(Exception):
    """A help or version request that has already printed everything it owes the caller."""


@dataclass(frozen=True)
class _Globals:
    """Options accepted only before the subcommand."""

    config: str | None = None
    agent: str | None = None
    json: bool = False


@dataclass(frozen=True)
class _RunOptions:
    """Options accepted only after ``run``."""

    command: str = ""
    cwd: str | None = None
    timeout: float | None = None
    wait_ready: float | None = None
    no_cache: bool = False
    dry_run: bool = False


@dataclass(frozen=True)
class _Invocation:
    """One fully parsed invocation."""

    globals: _Globals
    subcommand: str
    run: _RunOptions | None = None
    command: str | None = None
    force: bool = False
    no_cache: bool = False


# --- the option tables the two levels are built from --------------------------------------------


def _local_option_owners(name: str) -> tuple[str, ...]:
    """Return the subcommands that own ``name``, or an empty tuple if no subcommand does.

    Knowing the owner is what turns "unrecognized argument" into a usable message. A caller who
    writes ``herdr-run --cwd /tmp run ...`` has the right option and the wrong level, and being
    told which level it belongs to is the whole difference between a typo and a dead end.
    """
    if name in ("--cwd", "--timeout", "--wait-ready", "--dry-run"):
        return ("run",)
    if name == "--no-cache":
        return ("run", "target")
    if name == "--force":
        return ("init",)
    return ()


def _is_global_option(name: str) -> bool:
    return name in _GLOBAL_OPTIONS


def _find_subcommand(name: str) -> bool:
    return any(entry[0] == name for entry in SUBCOMMANDS)


def _subcommand_names() -> str:
    return ", ".join(sorted(name for name, _summary in SUBCOMMANDS))


def _join_quoted(values: Sequence[str]) -> str:
    return " or ".join(f"'{value}'" for value in values)


def _split_option(token: str) -> tuple[str, str | None]:
    """Split ``--name=value`` into its parts; a bare ``--name`` yields no inline value."""
    name, separator, value = token.partition("=")
    return (name, value if separator else None)


def _debug_repr(value: str) -> str:
    """Quote a string the way both editions quote an unparseable option value."""
    out = ['"']
    for character in value:
        code = ord(character)
        if character in ('"', "\\"):
            out.append("\\" + character)
        elif character == "\n":
            out.append("\\n")
        elif character == "\r":
            out.append("\\r")
        elif character == "\t":
            out.append("\\t")
        elif code < 0x20 or code == 0x7F:
            out.append(f"\\u{{{code:x}}}")
        else:
            out.append(character)
    out.append('"')
    return "".join(out)


# --- parsing ------------------------------------------------------------------------------------


def usage_line() -> str:
    """The one-line synopsis printed above every argument error."""
    return (
        "herdr-run [-h] [--version] [--config PATH] [--agent NAME] [--json] "
        "<subcommand> [OPTIONS]"
    )


def _option_value(
    raw: Sequence[str], index: int, name: str, inline: str | None
) -> tuple[str, int]:
    if inline is not None:
        return inline, index
    if index + 1 >= len(raw):
        raise _UsageError(f"argument {name}: expected one value")
    value = raw[index + 1]
    if value.startswith("-") and value != "-":
        raise _UsageError(f"argument {name}: expected one value")
    return value, index + 1


def _parse_seconds(option: str, value: str) -> float:
    if value != value.strip() or "_" in value:
        raise _UsageError(
            f"argument {option}: invalid numeric value: {_debug_repr(value)}"
        )
    try:
        parsed = float(value)
    except (OverflowError, ValueError):
        raise _UsageError(
            f"argument {option}: invalid numeric value: {_debug_repr(value)}"
        ) from None
    if not math.isfinite(parsed) or parsed < 0:
        raise _UsageError(f"argument {option}: must be a finite non-negative number")
    if parsed > MAX_TIMEOUT_SECONDS:
        raise _UsageError(
            f"argument {option}: must not exceed {MAX_TIMEOUT_SECONDS:.0f} seconds"
        )
    return parsed


def _local_argument_error(subcommand: str, token: str) -> str:
    """Describe an option that appeared after the subcommand but does not belong to it."""
    name, _inline = _split_option(token)
    if _is_global_option(name) and name not in ("--help", "-h"):
        return (
            f"argument {name}: this is a GLOBAL option; put it before the subcommand:\n"
            f"  herdr-run {name} ... {subcommand} ..."
        )
    owners = _local_option_owners(name)
    if owners and subcommand not in owners:
        return (
            f"argument {name}: this is a {_join_quoted(owners)} option, "
            f"not a '{subcommand}' option"
        )
    return f"{subcommand}: unrecognized arguments: {token}"


def _misplaced_global(name: str) -> str:
    """Describe an option that appeared before the subcommand but belongs to one of them."""
    owners = _local_option_owners(name)
    if not owners:
        return f"unrecognized arguments: {name}"
    return (
        f"argument {name}: this is a {_join_quoted(owners)} option; "
        f"put it AFTER the subcommand:\n  herdr-run {owners[0]} {name} ..."
    )


def _unknown_subcommand(token: str, following: Sequence[str]) -> str:
    """Describe a first positional that is not a subcommand, naming the removed form's replacement.

    ``herdr-run <agent> '<command>'`` and ``herdr-run '<command>'`` used to run a command with no
    subcommand at all. Removing that is the point of this surface, so the removal has to be said
    out loud, with the exact line to type instead -- an "unknown subcommand" alone would leave a
    caller guessing that the tool had been withdrawn.
    """
    message = f"unknown subcommand '{token}'"
    replacement: str | None = None
    if len(following) == 1 and not following[0].startswith("-"):
        replacement = f"herdr-run --agent {token} run '{following[0]}'"
    elif not following and " " in token:
        replacement = f"herdr-run run '{token}'"
    if replacement is not None:
        message += (
            "\nRunning a command without a subcommand is no longer accepted. "
            f"Use 'run':\n    {replacement}"
        )
    return message + f"\nSubcommands: {_subcommand_names()}"


def _one_argument_error(subcommand: str, positional: Sequence[str]) -> str:
    joined = " ".join(positional)
    return (
        f"{subcommand}: pass the command as ONE quoted argument, not as separate words.\n"
        f"  you wrote:  herdr-run {subcommand} {joined}\n"
        f"  instead:    herdr-run {subcommand} '{joined}'\n"
        "Re-joining loose words would silently change the quoting of your arguments."
    )


def _parse_bare(name: str, rest: Sequence[str]) -> list[str]:
    """Parse a subcommand that takes no options, returning its positional arguments."""
    positional: list[str] = []
    options = True
    for token in rest:
        if options and token == "--":
            options = False
            continue
        if options and token in ("--help", "-h"):
            _print_subcommand_help(name)
            raise _EarlyExit()
        if options and token.startswith("-"):
            raise _UsageError(_local_argument_error(name, token))
        positional.append(token)
    return positional


def _parse_run(rest: Sequence[str]) -> _RunOptions:
    options = _RunOptions()
    positional: list[str] = []
    parsing = True
    index = 0
    while index < len(rest):
        token = rest[index]
        if parsing and token == "--":
            parsing = False
            index += 1
            continue
        if not parsing or not token.startswith("-"):
            positional.append(token)
            index += 1
            continue
        if token in ("--help", "-h"):
            _print_subcommand_help("run")
            raise _EarlyExit()
        if token == "--no-cache":
            options = replace(options, no_cache=True)
        elif token == "--dry-run":
            options = replace(options, dry_run=True)
        else:
            name, inline = _split_option(token)
            if name == "--cwd":
                value, index = _option_value(rest, index, name, inline)
                options = replace(options, cwd=value)
            elif name == "--timeout":
                value, index = _option_value(rest, index, name, inline)
                options = replace(options, timeout=_parse_seconds(name, value))
            elif name == "--wait-ready":
                value, index = _option_value(rest, index, name, inline)
                options = replace(options, wait_ready=_parse_seconds(name, value))
            else:
                raise _UsageError(_local_argument_error("run", name))
        index += 1
    if not positional:
        raise _UsageError("run: needs a command to run")
    if len(positional) > 1:
        raise _UsageError(_one_argument_error("run", positional))
    return replace(options, command=positional[0])


def _parse_flag_only(name: str, rest: Sequence[str], flag: str) -> bool:
    present = False
    for token in rest:
        if token in ("--help", "-h"):
            _print_subcommand_help(name)
            raise _EarlyExit()
        if token == flag:
            present = True
            continue
        raise _UsageError(_local_argument_error(name, token))
    return present


def _parse_subcommand(name: str, rest: Sequence[str], globals_: _Globals) -> _Invocation:
    """Parse one subcommand's own arguments."""
    if name == "run":
        return _Invocation(globals_, name, run=_parse_run(rest))
    if name == "check":
        positional = _parse_bare(name, rest)
        if not positional:
            raise _UsageError("check: needs a command to check")
        if len(positional) > 1:
            raise _UsageError(_one_argument_error("check", positional))
        return _Invocation(globals_, name, command=positional[0])
    if name == "init":
        return _Invocation(
            globals_, name, force=_parse_flag_only(name, rest, "--force")
        )
    if name == "target":
        return _Invocation(
            globals_, name, no_cache=_parse_flag_only(name, rest, "--no-cache")
        )
    positional = _parse_bare(name, rest)
    if positional:
        raise _UsageError(
            f"{name}: takes no positional arguments; got {len(positional)}"
        )
    return _Invocation(globals_, name)


def _parse(raw: Sequence[str]) -> _Invocation:
    if any(
        any(0xD800 <= ord(character) <= 0xDFFF for character in token) for token in raw
    ):
        raise _UsageError("arguments must be valid UTF-8")

    globals_ = _Globals()
    index = 0
    chosen: str | None = None
    while index < len(raw):
        token = raw[index]
        if not token.startswith("-"):
            if not _find_subcommand(token):
                raise _UsageError(_unknown_subcommand(token, raw[index + 1 :]))
            chosen = token
            index += 1
            break
        if token == "--":
            raise _UsageError(
                f"expected a subcommand before '--'. Subcommands: {_subcommand_names()}"
            )
        if token in ("--help", "-h"):
            _print_help()
            raise _EarlyExit()
        if token == "--version":
            print(f"herdr-run {__version__}")
            raise _EarlyExit()
        if token == "--json":
            globals_ = replace(globals_, json=True)
        else:
            name, inline = _split_option(token)
            if name == "--config":
                value, index = _option_value(raw, index, name, inline)
                globals_ = replace(globals_, config=value)
            elif name == "--agent":
                value, index = _option_value(raw, index, name, inline)
                globals_ = replace(globals_, agent=value)
            elif name in ("--json", "--help", "--version"):
                raise _UsageError(f"argument {name}: takes no value")
            else:
                raise _UsageError(_misplaced_global(name))
        index += 1

    if chosen is None:
        # No subcommand at all is not an error: it is somebody asking what this command is.
        _print_help()
        raise _EarlyExit()
    return _parse_subcommand(chosen, raw[index:], globals_)


# --- help ---------------------------------------------------------------------------------------


def help_text() -> str:
    """Build the top-level help so it can be asserted on as text, not only printed."""
    lines = [
        f"usage: {usage_line()}\n",
        "\nRun an allowlisted command in a terminal pane that is not subject to whatever\n"
        "constrains the caller, and get its real stdout, stderr, and exit code back.\n",
        "\nsubcommands:\n",
    ]
    for name, summary in SUBCOMMANDS:
        lines.append(f"  {name:<11} {summary}\n")
    lines.append(
        "\nglobal options (before the subcommand):\n"
        "  -h, --help            show this help message and exit\n"
        "  --version             show version and exit\n"
        "  --config PATH         explicit configuration file\n"
        "  --agent NAME          the agent this invocation speaks for; names its tab\n"
        "  --json                emit machine-readable output where a subcommand has it\n"
    )
    lines.append(
        "\nEach subcommand has its own options: run 'herdr-run <subcommand> --help'.\n"
        "Options are not shared between the two levels — '--cwd' is a 'run' option and\n"
        "goes after 'run'; '--agent' is global and goes before it.\n"
    )
    lines.append(
        "\nTwo documentation commands, and they are for different moments:\n"
        "  quickstart  one screen. What this is for, the four commands worth trying\n"
        "              first, and the five things to know before running anything real.\n"
        "  userguide   the complete reference: configuration, exit codes, readiness,\n"
        "              retention, the pane cap, and the trust model.\n"
    )
    return "".join(lines)


#: Each subcommand's own help: usage line, what it does, and the options it alone accepts.
_SUBCOMMAND_HELP: dict[str, str] = {
    "run": (
        "usage: herdr-run [GLOBAL OPTIONS] run [OPTIONS] '<command>'\n"
        "\nRun one allowlisted command in this agent's pane and return its stdout, stderr,\n"
        "and exit code. The command is ONE argument: quote it.\n"
        "\npositional arguments:\n"
        "  <command>             the whole command line, as a single quoted argument\n"
        "\noptions:\n"
        "  -h, --help            show this help message and exit\n"
        "  --cwd PATH            working directory for the command\n"
        "  --timeout SECONDS     how long to wait for the command to finish\n"
        "  --wait-ready SECONDS  how long to wait for the pane to go idle\n"
        "  --no-cache            ignore the session cache and re-resolve from labels\n"
        "  --dry-run             admit and render the command; execute nothing\n"
        "\nExample:\n"
        "  herdr-run --agent release-agent run 'git push origin HEAD'\n"
    ),
    "check": (
        "usage: herdr-run [GLOBAL OPTIONS] check '<command>'\n"
        "\nDecide whether a command would be admitted by the policy in effect. Touches no\n"
        "pane and executes nothing. Exits 0 when allowed and 77 when refused.\n"
        "\npositional arguments:\n"
        "  <command>             the whole command line, as a single quoted argument\n"
        "\noptions:\n"
        "  -h, --help            show this help message and exit\n"
    ),
    "init": (
        "usage: herdr-run [GLOBAL OPTIONS] init [OPTIONS]\n"
        "\nWrite an annotated .herdr-run.yaml into the current directory, the way 'git init'\n"
        "writes into the current directory. Every knob is present and set to the value in\n"
        "force today, so adopting the file changes nothing until you edit it. Refuses to\n"
        "overwrite an existing configuration.\n"
        "\noptions:\n"
        "  -h, --help            show this help message and exit\n"
        "  --force               overwrite an existing configuration file\n"
    ),
    "status": (
        "usage: herdr-run [GLOBAL OPTIONS] status\n"
        "\nReport what is in effect here: which configuration file was found and where its\n"
        "root is, what the policy admits, whether Herdr is reachable and running, and how\n"
        "many panes the workspace already holds. Strictly non-mutating — it starts no\n"
        "server and creates no workspace, tab, or pane.\n"
        "\noptions:\n"
        "  -h, --help            show this help message and exit\n"
    ),
    "config": (
        "usage: herdr-run [GLOBAL OPTIONS] config\n"
        "\nPrint the fully resolved configuration as JSON: every value in effect, the file\n"
        "it came from, and the tab label it renders for this agent.\n"
        "\noptions:\n"
        "  -h, --help            show this help message and exit\n"
    ),
    "target": (
        "usage: herdr-run [GLOBAL OPTIONS] target [OPTIONS]\n"
        "\nResolve this agent's pane, creating the workspace, tab, and pane if they are\n"
        "missing, and print their ids together with the readiness verdict.\n"
        "\noptions:\n"
        "  -h, --help            show this help message and exit\n"
        "  --no-cache            ignore the session cache and re-resolve from labels\n"
    ),
    "reap": (
        "usage: herdr-run [GLOBAL OPTIONS] reap\n"
        "\nReport which command tabs are provably finished with, and why every other one\n"
        "was declined. Closes nothing.\n"
        "\noptions:\n"
        "  -h, --help            show this help message and exit\n"
    ),
    "net-doctor": (
        "usage: herdr-run [GLOBAL OPTIONS] net-doctor\n"
        "\nSmoke-test one narrow scenario: a caller whose own network access is blocked,\n"
        "reaching the network through a pane that is not blocked. It runs one probe\n"
        "directly and again through the pane and compares the two. This is not a health\n"
        "check of the command as a whole.\n"
        "\noptions:\n"
        "  -h, --help            show this help message and exit\n"
    ),
    "quickstart": (
        "usage: herdr-run [GLOBAL OPTIONS] quickstart\n"
        "\nPrint the one-screen introduction: what this command is for, the four commands\n"
        "worth trying first, the shape of the command line, and the five things worth\n"
        "knowing before running anything real.\n"
        "\noptions:\n"
        "  -h, --help            show this help message and exit\n"
    ),
    "userguide": (
        "usage: herdr-run [GLOBAL OPTIONS] userguide\n"
        "\nPrint the complete reference: configuration, exit codes, readiness, retention,\n"
        "the pane cap, and the trust model.\n"
        "\noptions:\n"
        "  -h, --help            show this help message and exit\n"
    ),
}


def subcommand_help_text(name: str) -> str:
    """Build one subcommand's help so it can be asserted on as text, not only printed."""
    return _SUBCOMMAND_HELP.get(name, f"usage: herdr-run [GLOBAL OPTIONS] {name}\n")


def _print_help() -> None:
    sys.stdout.write(help_text())


def _print_subcommand_help(name: str) -> None:
    sys.stdout.write(subcommand_help_text(name))


# --- shared helpers -----------------------------------------------------------------------------


def _default_agent(environ: dict[str, str]) -> str:
    """Infer the invoking agent from the environment ORC actually sets."""
    for key in ("HERDR_RUN_AGENT", "DG_AGENT_NAME", "ORC_AGENT_NAME"):
        value = environ.get(key, "").strip()
        if value:
            return value
    return "unknown-agent"


def _client(config: Config, environ: dict[str, str]) -> HerdrClient:
    return HerdrClient(broker=config.broker, environ=environ)


def _emit_json(document: object) -> None:
    json.dump(document, sys.stdout, indent=2, sort_keys=True, ensure_ascii=False)
    sys.stdout.write("\n")


def _resolved_cwd(config: Config, override: str | None) -> str:
    """Where the command runs: --cwd, else the project's configured cwd, else THE CALLER'S CWD.

    Defaulting to ``project_root`` was a silent-mistargeting bug. Config discovery walks UP to the
    nearest ``.herdr-run.yaml``, so a config file at the top of a multi-repo tree makes every nested
    worktree resolve ``project_root`` to that top directory -- and the command then ran there
    instead of in the slot the caller was standing in. It did not look like an error; it looked like
    the wrong repository answering. "Where the policy lives" and "where the command runs" are
    different questions, and only the first is anchored to the config file.
    """
    cwd = override if override is not None else config.cwd
    if cwd is None:
        try:
            return os.getcwd()
        except OSError as exc:
            raise ConfigError(f"cannot determine current directory: {exc}") from exc
    if not os.path.isabs(cwd):
        cwd = os.path.abspath(os.path.join(config.project_root, cwd))
    return cwd


def _print_document(subcommand: str) -> int:
    resource = _DOCUMENTS[subcommand]
    try:
        from importlib.resources import files

        text = (files("herdr_run") / resource).read_text(encoding="utf-8")
    except (OSError, ModuleNotFoundError):
        print(
            f"herdr-run: the {subcommand} document is not available in this installation",
            file=sys.stderr,
        )
        return 1
    sys.stdout.write(text)
    return 0


# --- subcommands --------------------------------------------------------------------------------


def _cmd_init(directory: str, *, force: bool, json_output: bool) -> int:
    from herdr_run.init import write_config_template

    path = write_config_template(directory, force=force)
    if json_output:
        _emit_json({"created": True, "path": path})
    else:
        print(f"wrote {path}")
        print(
            "Every knob is in that file, set to the value in force today. The allowlist near the\n"
            "top is a HUMAN-ONLY knob: an agent that can widen its own allowlist does not have one."
        )
    return 0


def _cmd_status(
    config: Config, agent: str, environ: dict[str, str], *, json_output: bool
) -> int:
    """Report what is in effect here, and change nothing while doing it."""
    from herdr_run.status import (
        inspect_session,
        status_document,
        status_text,
        unreachable,
    )

    tab_label = tab_label_for(config, agent)
    # A resolution failure is a fact about the session worth REPORTING, not an error to exit on:
    # "herdr is not installed" is one of the most useful things status can tell somebody.
    try:
        client = _client(config, environ)
        # Resolve the executables eagerly, the way the Rust edition's constructor does, so an
        # absent Herdr is reported as "not reachable" rather than as "no server is running".
        client.preflight()
    except HerdrUnavailable as exc:
        session = unreachable(str(exc))
    else:
        session = inspect_session(client, config, tab_label)
    if json_output:
        _emit_json(status_document(config, agent, tab_label, session))
    else:
        sys.stdout.write(status_text(config, agent, tab_label, session))
    return 0


def _cmd_config(config: Config, agent: str) -> int:
    document = {
        "source": config.source_path or "(built-in defaults)",
        "project_root": config.project_root,
        "workspace": config.workspace,
        "tab_name": config.tab_name,
        "tab_label": tab_label_for(config, agent),
        "cwd": config.cwd,
        "allow": list(config.allow),
        "prefixes": list(config.prefixes),
        "deny_global": {key: list(value) for key, value in config.deny_global.items()},
        "deny_subcommand": {
            key: list(value) for key, value in config.deny_subcommand.items()
        },
        "deny_anywhere": list(config.deny_anywhere),
        "allow_subcommand": {
            key: list(value) for key, value in config.allow_subcommand.items()
        },
        "value_options": {
            key: list(value) for key, value in config.value_options.items()
        },
        "spool_dir": config.spool_dir,
        "timeout_seconds": config.timeout_seconds,
        "retention_days": config.retention_days,
        "max_panes": config.max_panes,
        "ready_timeout_seconds": config.ready_timeout_seconds,
        "readiness": config.readiness,
        "prompt_tail": config.prompt_tail,
        "shells": list(config.shells),
        "probe_remote": config.probe_remote,
        "broker": config.broker,
    }
    _emit_json(document)
    return 0


def _cmd_check(config: Config, command: str, *, json_output: bool) -> int:
    """Answer only the policy question, touching no pane and no Herdr server."""
    try:
        admission = admit(command, config)
    except Refused as exc:
        if json_output:
            _emit_json({"reason": str(exc), "verdict": "refused"})
        else:
            print(f"REFUSED: {exc}", file=sys.stderr)
        return Refused.exit_code
    if json_output:
        _emit_json(
            {
                "program": admission.program,
                "rendered": admission.rendered,
                "subcommand": admission.subcommand,
                "verdict": "allowed",
            }
        )
    else:
        print(
            f"ALLOWED: program={admission.program} subcommand={admission.subcommand or '-'}"
        )
        print(f"rendered: {admission.rendered}")
    return 0


def _cmd_target(
    config: Config, agent: str, environ: dict[str, str], *, use_cache: bool
) -> int:
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
    _emit_json(document)
    return 0 if readiness.ready else EXIT_BUSY


def _cmd_reap(config: Config, environ: dict[str, str]) -> int:
    """Report which command tabs are PROVABLY finished with. Closes nothing.

    Report-only is the point, not a limitation. The expensive mistake is closing a tab whose agent
    is merely thinking, and a reaper that is wrong once in that direction is switched off for good;
    so the first version of this has to be checkable against a known-good population before anyone
    lets it act. Every declined pane carries its reason, and every verdict is counted including the
    zeros, because "reaped 0 because nothing was stale" and "reaped 0 because the detector is inert"
    are otherwise the same output.

    ``candidate_source`` states the bound on what could POSSIBLY have been considered. The candidate
    set is the panes named by surviving run records, and herdr-run prunes a run record
    ``retention_days`` after the run finished -- so the oldest leaked tabs, which are exactly the
    ones the pane cap exists to bound, drop out of this report while still counting against
    ``max_panes``. Printing the window is the difference between a report an operator can reason
    about and a count that quietly means something narrower than it says.
    """
    client = _client(config, environ)
    plan = sweep(client, config)
    document = {
        "workspace": config.workspace,
        "candidate_source": {
            "spool_dir": config.spool_dir,
            "retention_days": config.retention_days,
            "note": (
                "candidates are the panes named by surviving run records; run records are pruned "
                "retention_days after the run finished, so a tab whose agent last ran longer ago "
                "than that is not considered here and must be closed by hand"
            ),
        },
        "counts": plan.counts(),
        "reapable": [
            {
                "pane_id": decision.pane_id,
                "tab_id": decision.tab_id,
                "tab_label": decision.tab_label,
                "reason": decision.reason,
            }
            for decision in plan.reapable
        ],
        "declined": [
            {
                "pane_id": decision.pane_id,
                "tab_id": decision.tab_id,
                "tab_label": decision.tab_label,
                "verdict": decision.verdict,
                "reason": decision.reason,
            }
            for decision in plan.declined
        ],
    }
    _emit_json(document)
    return 0


#: The scope disclaimer ``net-doctor`` prints before it does anything.
#:
#: It goes FIRST, not in the verdict, because a diagnostic that only says what it covered once it
#: has already failed reads as an excuse. ``net-doctor`` answers exactly one question -- does
#: routing a network command through a pane get past a block on the caller's own network -- and a
#: reader whose interest in this command is something else should be able to stop at line one.
NET_DOCTOR_SCOPE = (
    "This is a smoke test for ONE scenario, not a health check of this command as a whole:\n"
    "a caller whose own network access is blocked, running a network command through a pane\n"
    "that is not blocked. It runs the same probe twice — directly, then through the pane —\n"
    "and compares. If you route commands through a pane for some other reason, the verdict\n"
    "below says nothing about that reason.\n"
)


def _net_doctor_error_fields(direct_exit_code: int | None = None) -> dict[str, object]:
    fields: dict[str, object] = {"net_doctor": True}
    if direct_exit_code is not None:
        fields["direct_exit_code"] = direct_exit_code
    return fields


def _cmd_net_doctor(config: Config, agent: str, environ: dict[str, str]) -> int:
    """Bracket ONE scenario in both directions instead of asserting it.

    Positive: the probe through the pane must succeed. Negative: run directly, under whatever
    constrains this process, it must fail. If BOTH succeed the pane is not buying anything for this
    scenario; if both fail the pane path is broken. Reporting one side only would leave "the pane is
    outside the caller's confinement" as an inference rather than an observation.
    """
    import subprocess

    probe = shlex.join(("git", "ls-remote", config.probe_remote, "HEAD"))
    prefixed = (
        shlex.join(("with-proxy", *shlex.split(probe)))
        if "with-proxy" in config.prefixes
        else probe
    )
    print(
        f"herdr-run net-doctor  (agent={agent}, config={config.source_path or 'defaults'})"
    )
    print()
    print(NET_DOCTOR_SCOPE)

    log = audit_path(config.project_root, config.spool_dir)
    warn_if_spool_is_tracked(config.project_root, config.spool_dir)
    try:
        admission = admit(prefixed, config)
    except Refused as exc:
        _record_audit(
            log, agent, prefixed, "REFUSED", str(exc), _net_doctor_error_fields()
        )
        raise
    _record_audit(
        log,
        agent,
        prefixed,
        "ADMITTED",
        "net-doctor probe accepted; target resolution and launch have not yet completed",
        {"net_doctor": True, "rendered": admission.rendered},
    )

    try:
        direct = _bounded_control_command(["/bin/bash", "-lc", prefixed], timeout=120)
    except (OSError, subprocess.SubprocessError) as exc:
        error = HerdrUnavailable(f"cannot run the direct probe: {exc}")
        _record_audit(
            log,
            agent,
            prefixed,
            "HERDRUNAVAILABLE",
            str(error),
            _net_doctor_error_fields(),
        )
        raise error from exc
    direct_ok = direct.returncode == 0
    print(f"[direct  ] {prefixed}")
    print(f"           rc={direct.returncode} {'SUCCEEDED' if direct_ok else 'failed'}")
    if not direct_ok:
        stderr_lines = (direct.stderr or "").splitlines()
        print(f"           {stderr_lines[-1] if stderr_lines else ''}")

    pane_rc = -1
    try:
        client = _client(config, environ)
        target = resolve_target(client, config, agent)
        result = execute(
            client,
            config,
            target,
            admission,
            agent=agent,
            cwd=_resolved_cwd(config, None),
            ready_timeout=max(config.ready_timeout_seconds, 30.0),
            timeout=min(config.timeout_seconds, 180.0),
        )
        pane_rc = result.exit_code
        meta, meta_error = _write_meta_best_effort(result, admission, config, agent)
        fields = _net_doctor_error_fields(direct.returncode)
        fields.update(
            {
                "exit_code": pane_rc,
                "run_id": result.run_id,
                "pane_id": result.target.pane_id,
                "meta": meta,
            }
        )
        if meta_error is not None:
            fields["meta_error"] = meta_error
        _record_audit(
            log,
            agent,
            prefixed,
            "RAN",
            f"net-doctor pane probe exited {pane_rc}",
            fields,
        )
        head = (result.stdout.strip().splitlines() or [""])[0]
        print(f"[via pane] {prefixed}   (pane {target.pane_id}, tab {target.tab_label})")
        print(
            f"           rc={pane_rc} {'SUCCEEDED' if pane_rc == 0 else 'failed'}  {head[:80]}"
        )
    except HerdrRunError as exc:
        _record_audit(
            log,
            agent,
            prefixed,
            type(exc).__name__.upper(),
            str(exc),
            _net_doctor_error_fields(direct.returncode),
        )
        print(f"[via pane] FAILED: {exc}")

    print()
    if pane_rc == 0 and not direct_ok:
        print(
            "VERDICT: this scenario works — the probe is blocked directly and succeeds "
            "through the pane."
        )
        return 0
    if pane_rc == 0:
        print(
            "VERDICT: the pane works, but so does running the probe directly. For THIS scenario "
            "the pane is buying you nothing; prefer running the command directly. Other reasons "
            "to route a command through a pane are untouched by that."
        )
        return 0
    print(
        "VERDICT: the pane path is NOT working. Most likely the Herdr server was started from "
        "inside a confined process, so its panes inherit the confinement. Stop it "
        "('herdr server stop') and let herdr-run restart it via systemd-run, or start it from an "
        "unconfined shell."
    )
    return 1


def _run_command(
    config: Config,
    agent: str,
    options: _RunOptions,
    environ: dict[str, str],
    *,
    json_output: bool,
) -> int:
    command = options.command
    log = audit_path(config.project_root, config.spool_dir)
    warn_if_spool_is_tracked(config.project_root, config.spool_dir)

    try:
        admission: Admission = admit(command, config)
    except Refused as exc:
        _record_audit(log, agent, command, "REFUSED", str(exc))
        print(f"herdr-run: REFUSED: {exc}", file=sys.stderr)
        return Refused.exit_code

    if options.dry_run:
        _record_audit(log, agent, command, "DRY-RUN", admission.rendered)
        if json_output:
            _emit_json(
                {
                    "verdict": "allowed",
                    "program": admission.program,
                    "rendered": admission.rendered,
                }
            )
        else:
            print(admission.rendered)
        return 0

    _record_audit(
        log,
        agent,
        command,
        "ADMITTED",
        "policy accepted command; target resolution and launch have not yet completed",
        {"rendered": admission.rendered},
    )
    ready_timeout = (
        options.wait_ready
        if options.wait_ready is not None
        else config.ready_timeout_seconds
    )
    timeout = options.timeout if options.timeout is not None else config.timeout_seconds

    try:
        client = _client(config, environ)
        cwd = _resolved_cwd(config, options.cwd)
        target = resolve_target(client, config, agent, use_cache=not options.no_cache)
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
        _record_audit(log, agent, command, "RUNTIMEOUT", str(exc))
        _emit_raw_output(
            exc.partial_stdout.encode("utf-8"), exc.partial_stderr.encode("utf-8")
        )
        print(f"herdr-run: {exc}", file=sys.stderr)
        return exc.exit_code
    except HerdrRunError as exc:
        _record_audit(log, agent, command, type(exc).__name__.upper(), str(exc))
        print(f"herdr-run: {exc}", file=sys.stderr)
        return exc.exit_code

    meta, meta_error = _write_meta_best_effort(result, admission, config, agent)
    stdout_bytes, stderr_bytes = read_output_bytes(result)
    audit_fields: dict[str, object] = {
        "run_id": result.run_id,
        "pane_id": result.target.pane_id,
        "tab": result.target.tab_label,
        "exit_code": result.exit_code,
        "rendered": admission.rendered,
        "meta": meta,
    }
    if meta_error is not None:
        audit_fields["meta_error"] = meta_error
    _record_audit(
        log,
        agent,
        command,
        "RAN",
        f"exit {result.exit_code} in {result.duration_seconds:.1f}s",
        audit_fields,
    )

    if json_output:
        _emit_json(
            {
                "exit_code": result.exit_code,
                "stdout": result.stdout,
                "stderr": result.stderr,
                "stdout_base64": base64.b64encode(stdout_bytes).decode("ascii"),
                "stderr_base64": base64.b64encode(stderr_bytes).decode("ascii"),
                "run_id": result.run_id,
                "pane_id": result.target.pane_id,
                "tab": result.target.tab_label,
                "created": list(result.target.created),
                "duration_seconds": round(result.duration_seconds, 3),
                "spool_dir": result.spool.directory,
                "meta": meta,
            }
        )
    else:
        _emit_raw_output(stdout_bytes, stderr_bytes)
    return result.exit_code


def _record_audit(
    path: str,
    agent: str,
    command: str,
    verdict: str,
    detail: str,
    fields: dict[str, object] | None = None,
) -> None:
    """Attempt one audit append and make degradation visible without changing the result."""
    if not record(
        path,
        agent=agent,
        command=command,
        verdict=verdict,
        detail=detail,
        fields=fields,
    ):
        print(
            f"herdr-run: WARNING: could not append audit record to {path}",
            file=sys.stderr,
        )


def _write_meta_best_effort(
    result: RunResult, admission: Admission, config: Config, agent: str
) -> tuple[str | None, str | None]:
    """Write supplemental metadata without replacing an already-completed command result."""
    try:
        return write_meta(result, admission, config, agent), None
    except (OSError, UnicodeError) as exc:
        error = f"cannot write run metadata: {exc}"
        print(f"herdr-run: WARNING: {error}", file=sys.stderr)
        return None, error


def _emit_raw_output(stdout_bytes: bytes, stderr_bytes: bytes) -> None:
    """Write each captured byte stream exactly once, without newline or text translation."""
    stdout_buffer = getattr(sys.stdout, "buffer", None)
    stderr_buffer = getattr(sys.stderr, "buffer", None)
    try:
        if stdout_buffer is not None:
            stdout_buffer.write(stdout_bytes)
            stdout_buffer.flush()
        else:  # pragma: no cover - only nonstandard embedded text streams lack .buffer
            sys.stdout.write(stdout_bytes.decode("utf-8", errors="replace"))
    except (OSError, UnicodeError) as exc:
        raise HerdrRunError(f"cannot write stdout: {exc}") from exc
    try:
        if stderr_buffer is not None:
            stderr_buffer.write(stderr_bytes)
            stderr_buffer.flush()
        else:  # pragma: no cover - only nonstandard embedded text streams lack .buffer
            sys.stderr.write(stderr_bytes.decode("utf-8", errors="replace"))
    except (OSError, UnicodeError) as exc:
        raise HerdrRunError(f"cannot write stderr: {exc}") from exc


# --- dispatch -----------------------------------------------------------------------------------


def _dispatch(invocation: _Invocation, environ: dict[str, str]) -> int:
    try:
        start_dir = os.getcwd()
    except OSError as exc:
        raise ConfigError(f"cannot determine current directory: {exc}") from exc

    if invocation.subcommand == "init":
        # Deliberately before load_config: the reason to reach for `init` is often that discovery
        # found nothing, or found something broken, and refusing to write a fresh template because
        # the old one will not parse would be exactly the wrong moment to be strict.
        return _cmd_init(
            start_dir, force=invocation.force, json_output=invocation.globals.json
        )

    config = load_config(explicit_path=invocation.globals.config, start_dir=start_dir)
    agent = invocation.globals.agent or ""
    if not agent:
        agent = _default_agent(environ)
    json_output = invocation.globals.json

    if invocation.subcommand == "run":
        assert invocation.run is not None
        return _run_command(
            config, agent, invocation.run, environ, json_output=json_output
        )
    if invocation.subcommand == "check":
        assert invocation.command is not None
        return _cmd_check(config, invocation.command, json_output=json_output)
    if invocation.subcommand == "status":
        return _cmd_status(config, agent, environ, json_output=json_output)
    if invocation.subcommand == "config":
        return _cmd_config(config, agent)
    if invocation.subcommand == "target":
        return _cmd_target(
            config, agent, environ, use_cache=not invocation.no_cache
        )
    if invocation.subcommand == "reap":
        return _cmd_reap(config, environ)
    if invocation.subcommand == "net-doctor":
        return _cmd_net_doctor(config, agent, environ)
    raise AssertionError(f"unhandled subcommand {invocation.subcommand!r}")


def main(argv: Sequence[str] | None = None) -> int:
    """Entry point. Return the wrapped status or the documented code for a wrapper failure."""
    raw = list(argv) if argv is not None else sys.argv[1:]
    try:
        invocation = _parse(raw)
    except _EarlyExit:
        return 0
    except _UsageError as exc:
        print(f"usage: {usage_line()}", file=sys.stderr)
        print(f"herdr-run: error: {exc}", file=sys.stderr)
        return 2

    # Printed before any configuration is read: documentation is exactly what somebody with a
    # broken configuration file needs, and refusing to show it would be perverse.
    if invocation.subcommand in _DOCUMENTS:
        return _print_document(invocation.subcommand)

    try:
        return _dispatch(invocation, dict(os.environ))
    except HerdrRunError as exc:
        _safe_diagnostic(f"herdr-run: {exc}")
        return exc.exit_code
    except KeyboardInterrupt:  # pragma: no cover - interactive only
        _safe_diagnostic("herdr-run: interrupted")
        return 130


def _safe_diagnostic(message: str) -> None:
    """Best-effort stderr diagnostic for paths already handling a broken output stream."""
    try:
        print(message, file=sys.stderr)
    except (OSError, UnicodeError):
        pass


if __name__ == "__main__":  # pragma: no cover
    raise SystemExit(main())
