"""Command-line interface for durable agent-team timeline archives."""

from __future__ import annotations

import argparse
import json
import os
import sys
from collections.abc import Sequence
from importlib.resources import files
from pathlib import Path

from agent_team_timeline import __version__
from agent_team_timeline.archive import as_array, as_object, read_json
from agent_team_timeline.claude import ClaudeParseError
from agent_team_timeline.codex import CodexParseError
from agent_team_timeline.github_enrich import PullMetadataReport, enrich_pull_request_metadata
from agent_team_timeline.identity import IdentityOverrides, parse_identity_overrides
from agent_team_timeline.naming import AgentNameError
from agent_team_timeline.orc import OrcParseError
from agent_team_timeline.pipeline import (
    IngestReport,
    SummarizeReport,
    build_archive,
    ingest_claude,
    ingest_codex,
    ingest_orc,
    load_archived_team,
    record_run,
    summarize_archive,
    utc_now,
)
from agent_team_timeline.server import serve
from agent_team_timeline.summarize import SummaryError
from agent_team_timeline.token_usage import TokenUsage
from agent_team_timeline.window import DateWindow, parse_date_window

PROG = "agent-team-timeline"
DEFAULT_MODEL = os.environ.get("AGENT_TEAM_TIMELINE_MODEL", "gpt-5.6-luna")
DEFAULT_REASONING_EFFORT = os.environ.get(
    "AGENT_TEAM_TIMELINE_REASONING_EFFORT", "xhigh"
)


def _load_userguide() -> str:
    return (files("agent_team_timeline") / "USER_GUIDE.md").read_text(encoding="utf-8")


def _quickstart() -> str:
    return f"""{PROG} v{__version__}

Build a version-controllable, multi-resolution website from an agent team's append-only logs.

1. Ingest, summarize only cache misses, and build the site:

   {PROG} refresh \\
     --root-session SESSION_UUID --team example-team \\
     --output ./timelines/example-team \\
     --timezone America/New_York --model {DEFAULT_MODEL}

2. Open it locally:

   cd ./timelines/example-team
   make serve
   # http://127.0.0.1:8765/

3. Rerun the same refresh later. New log records are normalized; content-addressed summaries are
   reused; only changed time windows and calendar rollups spend model tokens.

The stages are separate on purpose: `ingest`, `summarize`, and `build`. The `build` command never
calls a model, so formatting and UI changes can be regenerated for free.
"""


def _path(raw: str) -> Path:
    return Path(raw).expanduser()


def _add_archive(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--output", required=True, help="version-controllable archive directory")
    parser.add_argument("--team", required=True, help="stable team slug, for example example-team")


def _add_date_window(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--timezone",
        default="America/New_York",
        help="IANA display/calendar timezone (UTC instants remain canonical)",
    )
    start = parser.add_mutually_exclusive_group()
    start.add_argument(
        "--start-date",
        help="first local calendar date to include (YYYY-MM-DD, inclusive)",
    )
    start.add_argument(
        "--start-time",
        help="first instant to include (RFC3339 with offset or Z, inclusive)",
    )
    end = parser.add_mutually_exclusive_group()
    end.add_argument(
        "--end-date",
        help="local calendar boundary to stop at (YYYY-MM-DD, exclusive)",
    )
    end.add_argument(
        "--end-time",
        help="instant to stop at (RFC3339 with offset or Z, exclusive)",
    )


def _add_site_identity(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--project",
        action="append",
        default=[],
        metavar="LABEL=REPOSITORY_URL",
        help=(
            "project/repository identity; repeat for multi-repo work (the first is primary); "
            "Codex git metadata is used when omitted"
        ),
    )
    parser.add_argument(
        "--source-host",
        action="append",
        default=[],
        metavar="HOSTNAME",
        help="execution hostname captured by the source data; repeat for multi-host work",
    )


def _add_ingest(parser: argparse.ArgumentParser) -> None:
    _add_archive(parser)
    parser.add_argument(
        "--sessions-root", default="~/.codex/sessions", help="Codex rollout tree (default: %(default)s)"
    )
    parser.add_argument("--root-session", required=True, help="coordinator thread UUID")
    _add_date_window(parser)
    _add_site_identity(parser)


def _add_claude_ingest(parser: argparse.ArgumentParser) -> None:
    _add_archive(parser)
    parser.add_argument(
        "--session-file",
        required=True,
        help="Claude coordinator JSONL; nested subagents are discovered beside it",
    )
    _add_date_window(parser)
    _add_site_identity(parser)


def _add_orc_ingest(parser: argparse.ArgumentParser) -> None:
    _add_archive(parser)
    parser.add_argument(
        "--source-root",
        required=True,
        help="project root containing Orc .orc/ and task .tg/ directories",
    )
    parser.add_argument("--root-session", required=True, help="Orc coordinator session UUID")
    _add_date_window(parser)
    _add_site_identity(parser)


def _add_summary(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--backend",
        choices=("codex", "heuristic"),
        default="codex",
        help="summary backend (heuristic is deterministic/offline)",
    )
    parser.add_argument("--model", default=DEFAULT_MODEL, help="Codex model name")
    parser.add_argument(
        "--reasoning-effort",
        default=DEFAULT_REASONING_EFFORT,
        help=(
            "Codex reasoning effort, recorded in cache provenance "
            "(default: %(default)s)"
        ),
    )
    parser.add_argument(
        "--service-tier",
        default=None,
        help=(
            "Codex service tier; Fast uses 'priority', omission means 'default', "
            "and the effective value is recorded in cache and run provenance"
        ),
    )
    parser.add_argument("--summary-workers", type=int, default=3)
    parser.add_argument("--summary-batch-size", type=int, default=6)
    parser.add_argument(
        "--name-batch-size",
        type=int,
        default=12,
        help="agents per hindsight-naming model call",
    )
    parser.add_argument("--phase-minutes", type=int, default=30)
    parser.add_argument("--context-chars", type=int, default=16000)
    parser.add_argument("--transcript-chars", type=int, default=30000)
    parser.add_argument(
        "--codex-command", default="codex", help="Codex executable (primarily for testing/wrappers)"
    )
    summary_start = parser.add_mutually_exclusive_group()
    summary_start.add_argument(
        "--summary-start-date",
        help="first local date to summarize without truncating archived data",
    )
    summary_start.add_argument(
        "--summary-start-time",
        help="first RFC3339 instant to summarize, inclusive",
    )
    summary_end = parser.add_mutually_exclusive_group()
    summary_end.add_argument(
        "--summary-end-date",
        help="local date boundary where summarization stops, exclusive",
    )
    summary_end.add_argument(
        "--summary-end-time",
        help="RFC3339 instant where summarization stops, exclusive",
    )
    parser.add_argument(
        "--summary-timezone",
        default=None,
        help="IANA timezone for summary date bounds (defaults to the archived team timezone)",
    )
    parser.add_argument(
        "--rollup-kind",
        action="append",
        choices=("hourly", "daily", "weekly", "monthly", "quarterly"),
        default=[],
        help=(
            "calendar summary level to generate; repeat as needed "
            "(default: daily, weekly, monthly, quarterly)"
        ),
    )


def _add_github_options(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--github-token-env",
        default="",
        help="environment variable containing an optional GitHub token; the token is never stored",
    )
    parser.add_argument(
        "--github-timeout",
        type=float,
        default=15.0,
        help="per-request GitHub API timeout in seconds (default: %(default)s)",
    )


def _add_export_selection(parser: argparse.ArgumentParser) -> None:
    start = parser.add_mutually_exclusive_group()
    start.add_argument("--start-date", help="first local date to export")
    start.add_argument("--start-time", help="first RFC3339 instant to export")
    end = parser.add_mutually_exclusive_group()
    end.add_argument("--end-date", help="exclusive local date boundary")
    end.add_argument("--end-time", help="exclusive RFC3339 instant")
    parser.add_argument(
        "--timezone",
        default=None,
        help="IANA timezone for date bounds (defaults to the archived team timezone)",
    )
    parser.add_argument(
        "--rollup-kind",
        action="append",
        choices=("hourly", "daily", "weekly", "monthly", "quarterly"),
        default=[],
        help="calendar summary level to include; repeat as needed",
    )


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog=PROG,
        description="Turn coordinator/subagent transcripts into a zoomable, multi-level summary site.",
    )
    parser.add_argument("--version", action="version", version=f"%(prog)s {__version__}")
    parser.add_argument("--userguide", action="store_true", help="print the complete embedded guide")
    sub = parser.add_subparsers(dest="command")

    quick = sub.add_parser("quickstart", help="print a short end-to-end example")
    quick.set_defaults(handler="quickstart")

    ingest = sub.add_parser("ingest", help="normalize Codex logs; do not call a model")
    _add_ingest(ingest)
    ingest.set_defaults(handler="ingest")

    ingest_claude_parser = sub.add_parser(
        "ingest-claude", help="normalize Claude logs; do not call a model"
    )
    _add_claude_ingest(ingest_claude_parser)
    ingest_claude_parser.set_defaults(handler="ingest_claude")

    ingest_orc_parser = sub.add_parser(
        "ingest-orc", help="snapshot and normalize Orc SQLite logs; do not call a model"
    )
    _add_orc_ingest(ingest_orc_parser)
    ingest_orc_parser.set_defaults(handler="ingest_orc")

    summarize = sub.add_parser("summarize", help="fill structured summary cache misses")
    _add_archive(summarize)
    _add_summary(summarize)
    summarize.set_defaults(handler="summarize")

    build = sub.add_parser("build", help="regenerate Markdown/site from cached data; zero tokens")
    _add_archive(build)
    build.add_argument("--phase-minutes", type=int, default=30)
    build.set_defaults(handler="build")

    export = sub.add_parser(
        "export", help="build a zero-token website slice in a separate directory"
    )
    export.add_argument("--archive", required=True, help="durable source archive")
    export.add_argument("--output", required=True, help="website export directory")
    export.add_argument("--team", required=True, help="team slug to export")
    export.add_argument("--phase-minutes", type=int, default=30)
    _add_export_selection(export)
    export.set_defaults(handler="export")

    refresh = sub.add_parser("refresh", help="idempotent ingest + summarize + build")
    _add_ingest(refresh)
    _add_summary(refresh)
    refresh.add_argument(
        "--github-metadata",
        action="store_true",
        help="conditionally cache titles for evidenced GitHub pull links after the build",
    )
    _add_github_options(refresh)
    refresh.set_defaults(handler="refresh")

    github = sub.add_parser(
        "github-metadata",
        help="conditionally cache GitHub pull titles, then rebuild the site",
    )
    _add_archive(github)
    github.add_argument("--phase-minutes", type=int, default=30)
    _add_github_options(github)
    github.set_defaults(handler="github-metadata")

    refresh_claude = sub.add_parser(
        "refresh-claude", help="idempotent Claude ingest + summarize + build"
    )
    _add_claude_ingest(refresh_claude)
    _add_summary(refresh_claude)
    refresh_claude.add_argument(
        "--github-metadata",
        action="store_true",
        help="conditionally cache titles for evidenced GitHub pull links after the build",
    )
    _add_github_options(refresh_claude)
    refresh_claude.set_defaults(handler="refresh_claude")

    refresh_orc = sub.add_parser(
        "refresh-orc", help="idempotent Orc ingest + summarize + build"
    )
    _add_orc_ingest(refresh_orc)
    _add_summary(refresh_orc)
    refresh_orc.add_argument(
        "--github-metadata",
        action="store_true",
        help="conditionally cache titles for evidenced GitHub pull links after the build",
    )
    _add_github_options(refresh_orc)
    refresh_orc.set_defaults(handler="refresh_orc")

    serve_parser = sub.add_parser("serve", help="serve a built archive on localhost")
    serve_parser.add_argument("--output", required=True, help="built archive directory")
    serve_parser.add_argument("--port", type=int, default=8765, help="0 chooses an available port")
    serve_parser.add_argument("--open", action="store_true", dest="open_browser")
    serve_parser.set_defaults(handler="serve")

    inspect = sub.add_parser("inspect", help="print archive/run/source counts as JSON")
    inspect.add_argument("--output", required=True)
    inspect.set_defaults(handler="inspect")
    return parser


def _summary_call(ns: argparse.Namespace) -> SummarizeReport:
    raw_service_tier: object = ns.service_tier
    service_tier = (
        None if raw_service_tier is None else str(raw_service_tier)
    )
    archive = _path(str(ns.output))
    raw_summary_timezone: object = ns.summary_timezone
    has_summary_date_bound = (
        ns.summary_start_date is not None or ns.summary_end_date is not None
    )
    if raw_summary_timezone is None and has_summary_date_bound:
        summary_timezone = load_archived_team(
            archive, str(ns.team)
        ).display_timezone
    elif raw_summary_timezone is None:
        summary_timezone = "UTC"
    else:
        summary_timezone = str(raw_summary_timezone)
    summary_window = parse_date_window(
        (
            str(ns.summary_start_date)
            if ns.summary_start_date is not None
            else None
        ),
        str(ns.summary_end_date) if ns.summary_end_date is not None else None,
        summary_timezone,
        start_time=(
            str(ns.summary_start_time)
            if ns.summary_start_time is not None
            else None
        ),
        end_time=(
            str(ns.summary_end_time)
            if ns.summary_end_time is not None
            else None
        ),
    )
    raw_rollup_kinds: object = ns.rollup_kind
    if not isinstance(raw_rollup_kinds, list) or not all(
        isinstance(item, str) for item in raw_rollup_kinds
    ):
        raise ValueError("--rollup-kind values must be strings")
    rollup_kinds = (
        tuple(raw_rollup_kinds)
        if raw_rollup_kinds
        else ("daily", "weekly", "monthly", "quarterly")
    )
    return summarize_archive(
        archive,
        str(ns.team),
        str(ns.backend),
        str(ns.model),
        max_workers=int(ns.summary_workers),
        batch_size=int(ns.summary_batch_size),
        name_batch_size=int(ns.name_batch_size),
        phase_minutes=int(ns.phase_minutes),
        context_chars=int(ns.context_chars),
        transcript_chars=int(ns.transcript_chars),
        codex_command=(str(ns.codex_command),),
        reasoning_effort=str(ns.reasoning_effort),
        service_tier=service_tier,
        summary_window=summary_window,
        rollup_kinds=rollup_kinds,
    )


def _identity_overrides(
    ns: argparse.Namespace, command_args: Sequence[str]
) -> IdentityOverrides:
    raw_projects: object = getattr(ns, "project", [])
    raw_hosts: object = getattr(ns, "source_host", [])
    if not isinstance(raw_projects, list) or not all(
        isinstance(item, str) for item in raw_projects
    ):
        raise ValueError("--project values must be strings")
    if not isinstance(raw_hosts, list) or not all(
        isinstance(item, str) for item in raw_hosts
    ):
        raise ValueError("--source-host values must be strings")
    projects, hosts = parse_identity_overrides(raw_projects, raw_hosts)
    timezone_explicit = any(
        item == "--timezone" or item.startswith("--timezone=")
        for item in command_args
    )
    return IdentityOverrides(
        projects,
        hosts,
        "explicit" if timezone_explicit else "default",
    )


def _print_ingest(report: IngestReport) -> None:
    print(
        f"ingest: {report.agents} agents, {report.events} messages/events, "
        f"{report.tool_calls} outer tools, {report.edges} edges from {report.sources} source files "
        f"({report.source_bytes / (1024 * 1024):.1f} MiB); {report.files_changed} archive files changed"
    )


def _print_summaries(report: SummarizeReport) -> None:
    tier = report.service_tier or "unspecified"
    print(
        f"summarize ({report.backend} / {report.model} / tier={tier}): "
        f"{report.phases} phases + {report.agent_names} hindsight agent names + "
        f"{report.rollups} calendar periods × 2 summary audiences; "
        f"cache {report.cache_hits} hit / {report.cache_misses} miss in "
        f"{report.backend_batches} backend batch(es); {report.project_overviews} project overview + "
        f"{report.glossary_definitions} glossary definitions; "
        f"catalog {report.catalog_artifacts} immutable artifacts"
    )
    print(
        _usage_line(
            "tokens newly spent",
            report.newly_spent_usage,
            report.newly_spent_unknown_receipts,
        )
    )
    print(
        _usage_line(
            "tokens behind returned cached artifacts",
            report.artifact_generation_usage,
            report.artifact_generation_unknown_receipts,
        )
    )
    if report.unknown_legacy_artifacts:
        print(
            "token accounting: "
            f"{report.unknown_legacy_artifacts} legacy artifact(s) have no usage receipt"
        )


def _usage_line(label: str, usage: TokenUsage, unknown_receipts: int) -> str:
    return (
        f"{label}: input={usage.input_tokens}, "
        f"cached_input={usage.cached_input_tokens}, "
        f"cache_write_input={usage.cache_write_input_tokens}, "
        f"output={usage.output_tokens}, "
        f"reasoning_output={usage.reasoning_output_tokens}, "
        f"total={usage.total_tokens}; unknown_receipts={unknown_receipts}"
    )


def _github_token(ns: argparse.Namespace) -> str | None:
    requested = str(ns.github_token_env).strip()
    if requested:
        value = os.environ.get(requested)
        if not value:
            raise ValueError(
                f"GitHub token environment variable {requested!r} is unset or empty"
            )
        return value
    return os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN") or None


def _print_github_metadata(report: PullMetadataReport) -> None:
    print(
        f"github metadata: {report.references} evidenced references / "
        f"{report.distinct_pulls} distinct pulls; {report.fetched} fetched, "
        f"{report.not_modified} unchanged, {len(report.failures)} failed; "
        f"cache {report.cache_path}"
    )


def _export_selection(
    ns: argparse.Namespace, archive: Path, team_slug: str
) -> tuple[DateWindow | None, tuple[str, ...]]:
    raw_timezone: object = ns.timezone
    timezone_name = (
        load_archived_team(archive, team_slug).display_timezone
        if raw_timezone is None
        else str(raw_timezone)
    )
    window = parse_date_window(
        str(ns.start_date) if ns.start_date is not None else None,
        str(ns.end_date) if ns.end_date is not None else None,
        timezone_name,
        start_time=str(ns.start_time) if ns.start_time is not None else None,
        end_time=str(ns.end_time) if ns.end_time is not None else None,
    )
    raw_kinds: object = ns.rollup_kind
    if not isinstance(raw_kinds, list) or not all(
        isinstance(item, str) for item in raw_kinds
    ):
        raise ValueError("--rollup-kind values must be strings")
    kinds = (
        tuple(raw_kinds)
        if raw_kinds
        else ("daily", "weekly", "monthly", "quarterly")
    )
    return window, kinds


def _inspect(archive: Path) -> int:
    timeline_path = archive / "data" / "timeline.json"
    manifest_path = archive / "manifest.json"
    if not timeline_path.is_file():
        raise ValueError(f"no built timeline at {timeline_path}")
    timeline = as_object(read_json(timeline_path), str(timeline_path))
    manifest = as_object(read_json(manifest_path), str(manifest_path)) if manifest_path.is_file() else {}
    result = {
        "archive": str(archive.resolve()),
        "agents": len(as_array(timeline.get("agents"), "timeline.agents")),
        "phases": len(as_array(timeline.get("phases"), "timeline.phases")),
        "edges": len(as_array(timeline.get("edges"), "timeline.edges")),
        "events": len(as_array(timeline.get("events"), "timeline.events")),
        "rollups": len(as_array(timeline.get("rollups"), "timeline.rollups")),
        "summary_files": len(
            as_array(timeline.get("summary_files"), "timeline.summary_files")
        ),
        "manifest": manifest,
    }
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


def main(argv: Sequence[str] | None = None) -> int:
    """Run the command-line interface and return its process exit status."""

    args = list(argv) if argv is not None else sys.argv[1:]
    parser = _parser()
    ns = parser.parse_args(args)
    if bool(ns.userguide):
        print(_load_userguide(), end="")
        return 0
    handler = getattr(ns, "handler", None)
    if handler is None:
        parser.print_help()
        return 0
    if handler == "quickstart":
        print(_quickstart())
        return 0
    if handler == "serve":
        serve(_path(str(ns.output)), "127.0.0.1", int(ns.port), bool(ns.open_browser))
        return 0
    if handler == "inspect":
        try:
            return _inspect(_path(str(ns.output)))
        except (OSError, ValueError) as error:
            print(f"{PROG}: {error}", file=sys.stderr)
            return 2
    if handler == "export":
        source_archive = _path(str(ns.archive))
        target = _path(str(ns.output))
        team_slug = str(ns.team)
        started = utc_now()
        command = [PROG, *args]
        try:
            window, rollup_kinds = _export_selection(
                ns, source_archive, team_slug
            )
            export_report = build_archive(
                source_archive,
                team_slug,
                phase_minutes=int(ns.phase_minutes),
                display_window=window,
                rollup_kinds=rollup_kinds,
                output=target,
            )
            run_path = record_run(
                target,
                command,
                started,
                "completed",
                team_slug,
                None,
                None,
                export_report,
            )
            print(
                f"export: {export_report['agents']} tracks, "
                f"{export_report['phases']} phases, "
                f"{export_report['rollups']} rollups; "
                f"{export_report['files_changed']} files changed"
            )
            print(f"run metadata: {run_path}")
            print(f"open: cd {target} && make serve")
            return 0
        except (OSError, ValueError) as error:
            try:
                run_path = record_run(
                    target,
                    command,
                    started,
                    "failed",
                    team_slug,
                    None,
                    None,
                    None,
                    error=str(error),
                )
                print(f"run metadata: {run_path}", file=sys.stderr)
            except (OSError, ValueError):
                pass
            print(f"{PROG}: {error}", file=sys.stderr)
            return 2

    archive = _path(str(ns.output))
    team_slug = str(ns.team)
    started = utc_now()
    command = [PROG, *args]
    ingest_report: IngestReport | None = None
    summary_report: SummarizeReport | None = None
    build_report: dict[str, int] | None = None
    try:
        ingest_handlers = (
            "ingest",
            "refresh",
            "ingest_claude",
            "refresh_claude",
            "ingest_orc",
            "refresh_orc",
        )
        if handler in ingest_handlers:
            identity_overrides = _identity_overrides(ns, args)
            date_window = parse_date_window(
                str(ns.start_date) if ns.start_date is not None else None,
                str(ns.end_date) if ns.end_date is not None else None,
                str(ns.timezone),
                start_time=(
                    str(ns.start_time) if ns.start_time is not None else None
                ),
                end_time=str(ns.end_time) if ns.end_time is not None else None,
            )
            if handler in ("ingest_claude", "refresh_claude"):
                _, ingest_report = ingest_claude(
                    archive,
                    _path(str(ns.session_file)),
                    team_slug,
                    str(ns.timezone),
                    date_window,
                    identity_overrides,
                )
            elif handler in ("ingest_orc", "refresh_orc"):
                _, ingest_report = ingest_orc(
                    archive,
                    _path(str(ns.source_root)),
                    str(ns.root_session),
                    team_slug,
                    str(ns.timezone),
                    date_window,
                    identity_overrides,
                )
            else:
                _, ingest_report = ingest_codex(
                    archive,
                    _path(str(ns.sessions_root)),
                    str(ns.root_session),
                    team_slug,
                    str(ns.timezone),
                    date_window,
                    identity_overrides,
                )
            _print_ingest(ingest_report)
        refresh_handlers = ("refresh", "refresh_claude", "refresh_orc")
        if handler == "summarize" or handler in refresh_handlers:
            summary_report = _summary_call(ns)
            _print_summaries(summary_report)
        if handler == "build" or handler in refresh_handlers:
            build_report = build_archive(
                archive, team_slug, phase_minutes=int(ns.phase_minutes)
            )
            print(
                f"build: {build_report['agents']} tracks, {build_report['phases']} phases, "
                f"{build_report['edges']} edges, {build_report['summary_files']} Markdown files; "
                f"{build_report['files_changed']} presentation files changed"
            )
        wants_github = handler == "github-metadata" or (
            handler in refresh_handlers and bool(ns.github_metadata)
        )
        if wants_github:
            github_report = enrich_pull_request_metadata(
                archive,
                team_slug,
                token=_github_token(ns),
                timeout_seconds=float(ns.github_timeout),
            )
            _print_github_metadata(github_report)
            build_report = build_archive(
                archive, team_slug, phase_minutes=int(ns.phase_minutes)
            )
            print(
                f"rebuild: {build_report['files_changed']} presentation files changed "
                "after GitHub metadata"
            )
            if github_report.failures:
                raise ValueError(
                    "GitHub metadata refresh had failures (successful records were retained): "
                    + "; ".join(github_report.failures)
                )
        run_path = record_run(
            archive,
            command,
            started,
            "completed",
            team_slug,
            ingest_report,
            summary_report,
            build_report,
        )
        print(f"run metadata: {run_path}")
        if build_report is not None:
            print(f"open: cd {archive} && make serve")
        return 0
    except (
        AgentNameError,
        ClaudeParseError,
        CodexParseError,
        OrcParseError,
        SummaryError,
        OSError,
        ValueError,
    ) as error:
        try:
            run_path = record_run(
                archive,
                command,
                started,
                "failed",
                team_slug,
                ingest_report,
                summary_report,
                build_report,
                error=str(error),
            )
            print(f"run metadata: {run_path}", file=sys.stderr)
        except (OSError, ValueError):
            pass
        print(f"{PROG}: {error}", file=sys.stderr)
        return 2


__all__ = ["main"]
