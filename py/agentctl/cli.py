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

from agentctl import __version__, registry_layout, sender_header
from agentctl.client import HerdrClient
from agentctl.errors import AgentPending, AgentPossiblySubmitted, HerdrRunError
from agentctl.legacy_cli import _ascii_float, _bounded_uint
from agentctl.profiles import (
    AGENTCLOUD_HARNESS,
    configuration_root, load_configuration, load_profiles, validate_raw_harness_arguments,
)
from agentctl.sessions import Sessions
from agentctl.skill_install import install_skill
from agentctl.stop_recovery import StopRecoveryParser, stop_refusal_message
from agentctl.subagents import environment_entries


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
    parser.add_argument("--deliver-while-working", action="store_true",
        help="interactive only: also type into a Claude Code pane that is working, native or behind a root slot "
             "relay, and let Claude Code queue the prompt behind its turn (default: off, which waits for idle or "
             "done). The composer must be empty first and the submission verified; a prompt Claude Code never "
             "accepts is taken back out and stays pending. Codex ordinary follow-ups use Enter while working "
             "by default; slash commands and unsupported harnesses still wait")


def _environment_entry(value: str) -> str:
    try:
        return environment_entries((value,))[0]
    except HerdrRunError as exc:
        raise argparse.ArgumentTypeError(str(exc)) from exc


def parser() -> argparse.ArgumentParser:
    """Build real subcommands with operation-specific options and examples."""
    root = StopRecoveryParser(prog="agentctl", allow_abbrev=False,
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
        root.share_stop_context(child)
        _common(child, inherited=True)
        if named:
            child.add_argument("name", metavar="NAME", help="registered session name (lowercase letters, digits, hyphens)")
        return child

    start = command("start", "Start a named interactive agent or a resumable headless worker.",
        "agentctl start reviewer --harness codex --cwd . --brief 'Review this change'", named=True)
    start.add_argument("--cwd", default=".", metavar="DIR", help="worker working directory (default: current directory)")
    start.add_argument("--profile", metavar="NAME",
        help="owner-configured profile from CWD/.agentctl/profiles.json, or, when CWD has none, from the "
             "profiles.json beside a registry named .agentctl (so a worktree outside the project "
             "uses the project's profiles); accepts --resume for local interactive Claude/Codex/Muse profiles; conflicts "
             "with other explicit harness launch settings")
    start.add_argument("--mode", choices=("interactive", "headless"), default=None,
        help="interactive keeps a native TUI; headless runs resumable structured turns (default: interactive)")
    start.add_argument("--backend", choices=("herdr", "tmux"), default="herdr",
        help="terminal host; interactive requires Herdr (default: herdr)")
    start.add_argument("--harness", default=None, metavar="KIND",
        help="native harness: codex/claude/muse interactive, codex/agy/muse headless (default: codex)")
    start.add_argument("--model", metavar="MODEL", help="native model name; omission preserves the harness default")
    start.add_argument("--reasoning-effort", metavar="EFFORT",
        help="structured harness effort; use the profile or this option, never a duplicate raw argument")
    start.add_argument("--resume", metavar="SESSION",
        help="resume an explicit native conversation for local interactive Claude/Codex/Muse, optionally using --profile")
    start.add_argument("--harness-arg", action="append", default=[], metavar="ARG",
        help="literal interactive harness argument; repeat and use = for flags")
    start.add_argument("--env", action="append", default=[], type=_environment_entry,
        metavar="KEY=VALUE",
        help="interactive Herdr only: set a literal variable in the created tab; repeat; values are not shown by status. "
             "A PATH set here is reordered by the pane shell's startup files; give a team tool directory with the "
             "top-level launch_env key of profiles.json instead, which every interactive start and revive types into "
             "the pane after those files have run")
    start.add_argument("--workspace-id", metavar="ID",
        help="interactive only: exact Herdr workspace; default: configured project workspace, else HERDR_WORKSPACE_ID, else shared subagents; an explicit ID must match configured policy")
    first = start.add_mutually_exclusive_group()
    first.add_argument("--brief", metavar="TEXT", help="initial task, submitted after launch")
    first.add_argument("--file", metavar="PATH", help="UTF-8 file containing the initial task")
    start.add_argument("--slot", metavar="SLOT",
        help="interactive only: box the agent to this wrkslots slot (plain-worktree or image-backed); "
             "the pane shell is replaced by the line `wrkslots shell-command SLOT` prints (per-slot "
             "slice limits and, except with cgroup isolation, a confined file-system view) before "
             "the harness starts, and the agent's cwd is the slot directory. Needs wrkslots on PATH "
             "or AGENTCTL_WRKSLOTS_BIN")
    start.add_argument("--slot-isolation", choices=("userns", "cgroup", "root"), default=None,
        help="with --slot: userns = limits plus the file-system view in a user namespace; root = the "
             "same view built through sudo -n, for harness launchers that need a setuid step (claude "
             "and codex only: the boxed harness runs behind sudo's terminal relay, so agentctl launches "
             "it itself, reads its state from Herdr's screen rules, and types prompts with screen "
             "verification; goals are unavailable); cgroup = limits only (default: the project's "
             "configuration.sandbox.isolation, else userns)")
    start.add_argument("--slot-project", metavar="DIR",
        help="with --slot: wrkslots project root (default: --cwd, searched upward for .wrkslots.yml)")
    start.add_argument("--startup-timeout", type=_ascii_float, default=30.0, metavar="SECONDS",
        help="interactive startup deadline, greater than 0 and at most 300 (default: 30)")
    _delivery(start)

    adopt = command("adopt", "Register an existing Herdr agent without taking ownership of its runtime; Muse requires a pinned foreground process.",
        "agentctl adopt reviewer --pane w1:p2 --workspace project --cwd /work/project --harness codex",
        named=True)
    adopt.add_argument("--pane", required=True, metavar="ID",
        help="exact live Herdr pane containing the agent (required)")
    adopt.add_argument("--workspace", required=True, metavar="LABEL",
        help="expected live Herdr workspace label; a mismatch is refused (required)")
    adopt.add_argument("--cwd", required=True, metavar="DIR",
        help="expected live agent working directory; compared canonically (required)")
    adopt.add_argument("--harness", required=True, metavar="KIND",
        help="expected live harness kind, for example codex, claude, or muse (required)")
    adopt.add_argument("--session", metavar="ID",
        help="optional stable native conversation ID already reported by this exact pane")

    command("move", "Move a running owned native Herdr agent into an existing workspace required by project configuration; adopted, custom-pane, and multi-pane tabs are refused.",
        "agentctl move reviewer", named=True)

    rename = command("rename", "Rename a running native or adopted agent: registry entry, Herdr agent name, and tab label "
        "together. Use it when an agent's warm context is still useful but its purpose changed; prefer a new agent "
        "otherwise. An interrupted rename refuses both names until the same command is rerun.",
        "agentctl rename reviewer release-reviewer", named=True)
    rename.add_argument("new_name", metavar="NEW", help="new registered name (lowercase letters, digits, hyphens)")
    anchor = command("anchor", "Pin the terminal and foreground harness process of an agent whose record pins no "
        "harness process, after checking that its pane runs the intended agent. Input to an agent with no pinned harness "
        "process or observed native session is refused. For a Muse pane or root slot relay, whose process is "
        "pinned at launch, only record that process's launch spec.",
        "agentctl anchor reviewer", named=True)
    anchor.add_argument("--replace", action="store_true",
        help="replace anchors that no longer match (the pane may hold another program; inspect it first)")
    anchor.add_argument("--session-id", metavar="ID",
        help="native conversation id to record in the launch spec when Herdr reports none; a Claude agent "
        "anchoring itself can pass \"$CLAUDE_CODE_SESSION_ID\". Refused if it contradicts Herdr's report. "
        "Default: Herdr's report, then the conversation the harness command line selects")
    doctor = command("doctor", "Compare every registry record with live Herdr state and report each mismatch. "
        "Read-only unless --repair-labels. Exit 0 when clean, 1 with findings.",
        "agentctl doctor")
    doctor.add_argument("--repair-labels", action="store_true",
        help="restore the tab label and Herdr agent name of owned agents whose every other anchor still matches")
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
    command("prompts", "Inspect a blocking question; reveal collapsed forms without selecting or submitting.",
        "agentctl prompts reviewer", named=True)
    answer = command("answer", "Answer a recognized TUI question and verify it was dismissed; never retry uncertain answers.",
        "agentctl answer reviewer --option 2", named=True)
    selection = answer.add_mutually_exclusive_group(required=True)
    selection.add_argument("--option", type=_bounded_uint, metavar="N", help="one-based option number printed by prompts")
    selection.add_argument("--text", metavar="TEXT", help="literal single-line answer for a supported empty or identical-draft free-text form")
    answer.add_argument("--replace-draft", action="store_true",
        help="with --text, clear an inspected unsent draft in a single Codex free-text form; default preserves drafts")
    read = command("read", "Read a terminal snapshot or a headless transcript/answer.",
        "agentctl read reviewer --lines 100", named=True)
    read.add_argument("--lines", type=_bounded_uint, default=500, metavar="COUNT", help="maximum tail/snapshot lines (default: 500)")
    read.add_argument("--output", choices=("tail", "all", "last", "since_turn"), default="tail",
        help="output boundary; last and since_turn require headless transcripts (default: tail)")
    read.add_argument("--since-turn", type=_bounded_uint, metavar="NUMBER", help="first included turn; requires --output since_turn")
    wait = command("wait", "Wait for readiness; readiness does not mean the goal is complete.",
        "agentctl wait reviewer --timeout 60", named=True)
    wait.add_argument("--timeout", type=_ascii_float, default=900.0, metavar="SECONDS", help="readiness deadline (default: 900 seconds)")
    revive = command("revive", "Resume a running record whose interactive harness is positively dead, in a fresh tab, by "
        "replaying its launch spec (launch_spec in status: what ran, written at start and read from the pinned process at "
        "adopt and anchor) with one explicit resume. A record without a spec is rebuilt from its launch policy and profile; "
        "an adopted record needs a spec, and its old pane is left open. "
        "Preserves the old queue and quarantine in its archive; never replays queued prompts.",
        "agentctl revive reviewer\n  agentctl revive --all --dry-run")
    revive.add_argument("name", nargs="?", metavar="NAME", help="registered session name; required unless --all is used")
    revive.add_argument("--all", action="store_true", help="consider every registered name; continue past blocked records")
    revive.add_argument("--dry-run", action="store_true", help="print recovery plans without changing registry files, locks, tabs, or processes")
    revive.add_argument("--expected-token", metavar="TOKEN", help="require this old record generation; applies only with NAME")
    revive.add_argument("--startup-timeout", type=_ascii_float, default=30.0, metavar="SECONDS",
        help="replacement harness startup deadline, greater than 0 and at most 300 seconds (default: 30)")

    promote = command("promote", "Mark a launch_failed record running when its harness is in fact live: Herdr reports "
        "the launched harness in the recorded pane, tab, workspace and cwd, the Herdr agent name still points there, and "
        "the harness is the pane's only foreground process. Pins that process and terminal; never touches the tab, pane, "
        "or process. Use it when a readiness wait timed out on a harness that runs fine.",
        "agentctl promote reviewer", named=True)
    promote.add_argument("--expected-token", metavar="TOKEN", help="require this record generation")

    stop = command("stop", "Stop an owned runtime, or unregister an adopted runtime without closing it, and archive state.",
        "agentctl stop reviewer\nagentctl stop reviewer --retire-dead-adoption --expected-token TOKEN --expected-record-sha256 SHA256", named=True)
    stop.add_argument("--expected-token", metavar="TOKEN",
        help="require this exact current registry generation; mandatory for dead managed or adopted recovery")
    adoption_recovery = stop.add_mutually_exclusive_group()
    adoption_recovery.add_argument("--recover-legacy-adoption", action="store_true",
        help="recover an identity-less dead adopted row without changing its pane; requires --expected-token and --expected-record-sha256")
    adoption_recovery.add_argument("--retire-dead-adoption", action="store_true",
        help="archive a running adopted record whose pinned harness is dead, without querying Herdr; requires --expected-token and --expected-record-sha256")
    stop.add_argument("--expected-record-sha256", metavar="SHA256",
        help="exact lowercase 64-hex SHA-256 of raw agent.json bytes; requires --recover-legacy-adoption or --retire-dead-adoption")
    attach_self = command("attach-self",
        "Show the agentcloud session this process runs in through its viewer in a Herdr tab and "
        "register it (Rust edition only; this edition refuses).",
        "agentctl attach-self coordinator --cwd .", named=True)
    attach_self.add_argument("--cwd", default=".", metavar="DIR",
        help="directory recorded for the session (default: current directory)")
    attach_self.add_argument("--workspace-id", metavar="ID", help="existing Herdr workspace ID")
    attach_self.add_argument("--startup-timeout", type=_ascii_float, default=30.0, metavar="SECONDS",
        help="viewer startup deadline, greater than 0 and at most 300 seconds (default: 30)")
    for rust_only, purpose, example in (
        ("restore", "Bring the coordinator team back after the host restarts (Rust edition only; this edition refuses).",
         "agentctl --registry ~/project/.agentctl restore --dry-run"),
        ("install-units", "Write the user units that restore the team at boot (Rust edition only; this edition refuses).",
         "agentctl install-units --dest ~/.config/systemd/user --dry-run"),
        ("watch", "Check and repair Herdr, configured units and timers, and slot images (Rust edition only; this "
         "edition refuses).", "agentctl watch --once --check-only"),
        ("watch-agents", "Watch every registered agent and tell the coordinator when one finished its turn with a "
         "verdict, is at a prompt, idle without a verdict, stalled, or dead (Rust edition only; this edition refuses).",
         "agentctl watch-agents --coordinator coordinator --once --check-only"),
        ("hold", "Record that an agent lands after another, for watch-agents to announce (Rust edition only; this "
         "edition refuses).", "agentctl hold cl-ingest --after cl-dedupe"),
        ("coordinator", "Show, set, or clear this registry's designated coordinator (Rust edition only; this "
         "edition refuses).", "agentctl coordinator show"),
        ("manage", "Change which agent controls a registered agent (Rust edition only; this edition refuses).",
         "agentctl manage worker-1 --by lieutenant"),
        ("tree", "Print the control tree of coordinator, managers, and workers (Rust edition only; this "
         "edition refuses).", "agentctl tree"),
        ("progress", "Declare what counts as progress for an agent, for watch-agents to judge (Rust edition only; "
         "this edition refuses).", "agentctl progress lane-a --file status.md --hours 6"),
    ):
        command(rust_only, purpose, example)
    for name, purpose in (
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
    migrate_layout = command("migrate-layout",
        "Move a registry's agents from the old layout (REGISTRY/NAME/, REGISTRY/.NAME.lock) to layout 2 "
        "(REGISTRY/agents/NAME/, REGISTRY/locks/NAME.lock), so the registry's fixed files are not buried among "
        "per-agent entries. Reports what it would do unless --apply.",
        "agentctl --registry ~/project/.agentctl migrate-layout --apply")
    migrate_layout.add_argument("--apply", action="store_true",
        help="move the agents and delete the old lock files (default: only report what would happen)")
    migrate_layout.add_argument("--wait", type=_ascii_float, default=10.0, metavar="SECONDS",
        help="seconds to wait for a busy agent or registry lock before leaving that agent where it is (default: 10)")
    migrate_layout.epilog = (migrate_layout.epilog or "") + (
        "\n\nEach agent moves by one atomic rename while agentctl holds its old and new lifecycle locks and the\n"
        "registry's identity lock; a rerun after a crash continues. An agent another process uses, whose record names\n"
        "paths inside its directory (a turn runner's home), or whose lock stays busy is listed under left; a name with a\n"
        "directory in both layouts is listed under conflicts and never touched. Running it again changes nothing.\n"
        "Exit 0, or 1 when something was left or conflicts. Commands that change an agent run the same migration\n"
        "first without waiting for any lock (AGENTCTL_LAYOUT_AUTO_MIGRATE=0 turns that off). Builds older than layout 2 cannot see\n"
        "moved agents and refuse the registry: upgrade every agentctl and agent-teams that uses it, and the wrkslots\n"
        "liveness probe copies, before migrating. The old layout stays readable until 2026-11-26.")
    command("quickstart", "Print a short working setup, available offline.", "agentctl quickstart")
    command("userguide", "Print the complete installed operator guide.", "agentctl userguide")
    profiles = command("profiles", "List safe metadata for ignored private launch profiles.",
        "agentctl profiles --cwd /work/project")
    profiles.add_argument("--cwd", default=".", metavar="DIR",
        help="project directory containing .agentctl/profiles.json (default: current directory); "
             "when it has none, the profiles.json beside a registry named .agentctl is listed")
    skill = commands.add_parser("skill", help="Install the bundled agentctl harness skill.",
        description="Install the bundled agentctl harness skill.", allow_abbrev=False)
    root.share_stop_context(skill)
    _common(skill, inherited=True)
    skill_commands = skill.add_subparsers(dest="skill_command", required=True, metavar="COMMAND")
    install = skill_commands.add_parser("install", help="Install for Codex, Claude, and Muse.", allow_abbrev=False)
    root.share_stop_context(install)
    _common(install, inherited=True)
    install.add_argument("--harness", action="append", choices=("codex", "claude", "muse"), default=[],
        help="target harness; repeat; omission installs all supported harnesses")
    install.add_argument("--force", action="store_true",
        help="replace divergent regular SKILL.md content; symlinks are always refused")
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


#: Commands only the Rust edition implements; this edition refuses them with exit status 2.
_RUST_ONLY = ("restore", "install-units", "watch", "watch-agents", "hold", "coordinator", "manage", "tree",
              "progress", "services")


#: Commands that change agents' state, so idle layout-1 agents move to layout 2 first (not before a
#: dry run). Everything else, reads and the chat services included, leaves the layout alone.
_WRITES_AGENTS = frozenset(("start", "adopt", "move", "rename", "anchor", "promote", "stop", "revive",
                            "send", "drain", "answer", "goal", "bind-session", "pause", "resume",
                            "reset", "repair", "migrate"))


def _auto_migrate(registry: Path) -> None:
    """New writes use layout 2, and idle layout-1 agents move first (#593 registry-agents-subdir)."""
    notice = registry_layout.auto_migrate(registry)
    if notice:
        print(notice, file=sys.stderr)


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
    if (command_index < len(arguments) and arguments[command_index] in _RUST_ONLY
            and not {"-h", "--help"} & set(arguments[command_index + 1:])):
        print(f"agentctl: {arguments[command_index]} is implemented only by the Rust edition of agentctl; "
              "run it with the Rust agentctl (for example DAGRUN_ENGINE=rust)", file=sys.stderr)
        return 2
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
        print(json.dumps({"interactive": {"backends": ["herdr"], "harnesses": ["codex", "claude", "muse"]},
            "headless": {"backends": ["herdr", "tmux"], "harnesses": ["codex", "agy", "muse"]},
            "services": ["chat", "mcp"], "registry": args.registry}, indent=2, sort_keys=True))
        return 0
    if args.command == "mcp":
        from agentctl.mcp import _serve
        return _serve(args.registry, args.herdr_bin)
    try:
        if args.command == "profiles":
            config_root = configuration_root(args.cwd, args.registry, absent_ok=True)
            path, profiles, workspace = load_configuration(config_root, absent_ok=True)
            print(json.dumps({"path": str(path), "workspace": workspace,
                "profiles": [item.public() for item in profiles.values()]},
                indent=2, sort_keys=True))
            return 0
        if args.command == "skill":
            if args.herdr_bin != "herdr" or args.registry != ".agentctl":
                raise ValueError("--registry and --herdr-bin do not apply to skill install")
            print(json.dumps(install_skill(args.harness, force=args.force), indent=2, sort_keys=True))
            return 0
        for key in ("ready_timeout", "working_timeout", "startup_timeout", "timeout"):
            value = getattr(args, key, None)
            if value is not None and (not math.isfinite(value) or value < 0 or value > 31_536_000
                or (key in ("working_timeout", "startup_timeout") and value == 0)):
                raise ValueError(f"{key.replace('_', '-')} must be finite and within its documented range")
        for key in ("max_attempts", "lines"):
            if getattr(args, key, 1) <= 0:
                raise ValueError(f"{key.replace('_', '-')} must be positive")
        if args.command == "migrate-layout":
            report = registry_layout.migrate(Path(args.registry), apply=args.apply, wait=args.wait)
            print(json.dumps(report, indent=2, sort_keys=True))
            return 1 if report["left"] or report["conflicts"] else 0
        if args.command in _WRITES_AGENTS and not getattr(args, "dry_run", False):
            _auto_migrate(Path(args.registry))
        sessions = Sessions(HerdrClient(herdr_bin=args.herdr_bin), args.registry)
        options: dict[str, object] = {key: getattr(args, key) for key in
            ("ready_timeout", "working_timeout", "max_attempts") if hasattr(args, key)}
        if getattr(args, "deliver_while_working", False):
            # Only when asked: a headless worker's request schema has no such option.
            options["deliver_while_working"] = True
        result: object
        name = getattr(args, "name", "")
        if args.command == "attach-self":
            raise ValueError(
                "attach-self presents an agentcloud session, which only the Rust edition of "
                "agentctl implements; run it with the Rust agentctl"
            )
        if args.command == "start":
            profile = None
            if args.profile is not None:
                overlaps = []
                for option, value in (
                    ("--mode", args.mode), ("--harness", args.harness), ("--model", args.model),
                    ("--reasoning-effort", args.reasoning_effort),
                ):
                    if value is not None:
                        overlaps.append(option)
                if args.harness_arg:
                    overlaps.append("--harness-arg")
                if args.env:
                    overlaps.append("--env")
                if overlaps:
                    raise ValueError(f"--profile conflicts with explicit launch settings: {', '.join(overlaps)}")
                _path, available = load_profiles(configuration_root(args.cwd, args.registry))
                try:
                    profile = available[args.profile]
                except KeyError as exc:
                    raise ValueError(f"unknown profile {args.profile!r}; run agentctl profiles --cwd {args.cwd}") from exc
            if args.slot is None and (args.slot_isolation is not None or args.slot_project is not None):
                raise ValueError("--slot-isolation and --slot-project need --slot")
            harness = profile.harness if profile else (args.harness or "codex")
            if harness == AGENTCLOUD_HARNESS:
                selected = f"profile {profile.name!r}" if profile else "--harness agentcloud"
                raise ValueError(
                    f"{selected} uses the agentcloud adapter, which only the Rust edition of "
                    "agentctl implements; start it with the Rust agentctl, or choose a Codex, "
                    f"Claude, or Muse profile from agentctl profiles --cwd {args.cwd}"
                )
            mode = profile.mode if profile else (args.mode or "interactive")
            model = profile.model if profile else args.model
            reasoning_effort = profile.reasoning_effort if profile else args.reasoning_effort
            if profile is None:
                validate_raw_harness_arguments(
                    harness, args.harness_arg, label="launch",
                    structured_model=args.model is not None,
                    structured_effort=args.reasoning_effort is not None,
                    structured_resume=args.resume is not None,
                )
            harness_args = list(profile.argv) if profile else args.harness_arg
            environment = list(profile.environment) if profile else args.env
            brief = Path(args.file).read_text(encoding="utf-8") if args.file else args.brief
            result = sessions.start_session(name, cwd=args.cwd, mode=mode, backend=args.backend,
                harness=harness, model=model, brief=brief, resume=args.resume,
                reasoning_effort=reasoning_effort, harness_args=harness_args,
                environment=environment,
                workspace_id=args.workspace_id,
                startup_timeout=args.startup_timeout, ready_timeout=args.ready_timeout,
                working_timeout=args.working_timeout, max_attempts=args.max_attempts,
                slot=args.slot, slot_isolation=args.slot_isolation,
                slot_project=args.slot_project, profile=args.profile,
                deliver_while_working=args.deliver_while_working)
        elif args.command == "adopt":
            result = sessions.adopt(name, pane_id=args.pane,
                expected_workspace=args.workspace, expected_cwd=args.cwd,
                harness=args.harness, session=args.session)
        elif args.command == "move":
            result = sessions.move_to_project_workspace(name)
        elif args.command == "list":
            result = sessions.list()
            print(json.dumps(result, indent=2, sort_keys=True))
            return 1 if any(row.get("record_error") is True for row in result) else 0
        elif args.command == "status":
            result = sessions.status(name)
        elif args.command == "send":
            # Every path delivers the prompt opened with its sender header (#609
            # sender-header-mandatory); nothing suppresses it.
            text = sender_header.stamp(_text(args) or "",
                                       sender_header.resolve_current(sessions.caller_agent()),
                                       sender_header.ROUTE)
            result = sessions.send_session(name, text, message_id=args.message_id, model=args.model, **options)
        elif args.command == "prompts":
            result = sessions.prompts(name)
        elif args.command == "answer":
            result = sessions.prompts(name, option=args.option, text=args.text, answering=True, replace_draft=args.replace_draft)
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
        elif args.command == "rename":
            result = sessions.rename(name, args.new_name)
        elif args.command == "revive":
            if args.startup_timeout > 300:
                raise ValueError("startup-timeout must be greater than 0 and at most 300 seconds")
            if bool(name) == args.all:
                raise ValueError("revive requires either NAME or --all")
            if args.all and args.expected_token is not None:
                raise ValueError("--expected-token requires NAME, not --all")
            if args.all:
                result = sessions.revive_all(dry_run=args.dry_run, startup_timeout=args.startup_timeout)
                print(json.dumps(result, indent=2, sort_keys=True))
                return 75 if result["blocked"] and not args.dry_run else 0
            result = sessions.revive(name, dry_run=args.dry_run, expected_token=args.expected_token,
                                     startup_timeout=args.startup_timeout)
        elif args.command == "promote":
            result = sessions.promote(name, expected_token=args.expected_token)
        elif args.command == "anchor":
            result = sessions.anchor(name, replace=args.replace, session=args.session_id)
        elif args.command == "doctor":
            report = sessions.doctor(repair_labels=args.repair_labels)
            print(json.dumps(report, indent=2, sort_keys=True))
            return 0 if report["clean"] else 1
        elif args.command == "bind-session":
            result = sessions.bind_session(name, args.session, goal_command=_goal_command(args))
        elif args.command in ("pause", "resume"):
            result = sessions.pause(name, paused=args.command == "pause")
        elif args.command == "attach":
            result = sessions.attach(name)
        elif args.command == "stop":
            recovery_selector = ("--retire-dead-adoption" if args.retire_dead_adoption
                                 else "--recover-legacy-adoption" if args.recover_legacy_adoption
                                 else None)
            if recovery_selector is not None and (
                args.expected_token is None or args.expected_record_sha256 is None
            ):
                raise ValueError(
                    f"{recovery_selector} requires --expected-token and "
                    "--expected-record-sha256"
                )
            if (recovery_selector is None
                    and args.expected_record_sha256 is not None):
                raise ValueError(
                    "--expected-record-sha256 requires --recover-legacy-adoption "
                    "or --retire-dead-adoption"
                )
            result = sessions.stop(
                name, expected_token=args.expected_token,
                recover_legacy_adoption=args.recover_legacy_adoption,
                retire_dead_adoption=args.retire_dead_adoption,
                expected_record_sha256=args.expected_record_sha256,
            )
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
        if args.command == "stop":
            print(stop_refusal_message(
                exc, prefix="agentctl", registry=args.registry, herdr_bin=args.herdr_bin,
                expected_token=args.expected_token,
                expected_record_sha256=args.expected_record_sha256,
            ), file=sys.stderr)
        return exc.exit_code
    except (ValueError, TypeError, OSError, HerdrRunError) as exc:
        if args.command == "stop":
            print(stop_refusal_message(
                exc, prefix="agentctl", registry=args.registry, herdr_bin=args.herdr_bin,
                expected_token=args.expected_token,
                expected_record_sha256=args.expected_record_sha256,
            ), file=sys.stderr)
        else:
            print(f"agentctl: {exc}", file=sys.stderr)
        return exc.exit_code if isinstance(exc, HerdrRunError) else 2


if __name__ == "__main__":
    raise SystemExit(main())
