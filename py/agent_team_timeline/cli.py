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
from agent_team_timeline.codex import CodexParseError
from agent_team_timeline.naming import AgentNameError
from agent_team_timeline.pipeline import (
    IngestReport,
    SummarizeReport,
    build_archive,
    ingest_codex,
    record_run,
    summarize_archive,
    utc_now,
)
from agent_team_timeline.server import serve
from agent_team_timeline.summarize import SummaryError

PROG = "agent-team-timeline"
DEFAULT_MODEL = os.environ.get("AGENT_TEAM_TIMELINE_MODEL", "gpt-5.6-sol")


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


def _add_ingest(parser: argparse.ArgumentParser) -> None:
    _add_archive(parser)
    parser.add_argument(
        "--sessions-root", default="~/.codex/sessions", help="Codex rollout tree (default: %(default)s)"
    )
    parser.add_argument("--root-session", required=True, help="coordinator thread UUID")
    parser.add_argument(
        "--timezone",
        default="America/New_York",
        help="IANA display/calendar timezone (UTC instants remain canonical)",
    )


def _add_summary(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--backend",
        choices=("codex", "heuristic"),
        default="codex",
        help="summary backend (heuristic is deterministic/offline)",
    )
    parser.add_argument("--model", default=DEFAULT_MODEL, help="Codex model name")
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

    summarize = sub.add_parser("summarize", help="fill structured summary cache misses")
    _add_archive(summarize)
    _add_summary(summarize)
    summarize.set_defaults(handler="summarize")

    build = sub.add_parser("build", help="regenerate Markdown/site from cached data; zero tokens")
    _add_archive(build)
    build.add_argument("--phase-minutes", type=int, default=30)
    build.set_defaults(handler="build")

    refresh = sub.add_parser("refresh", help="idempotent ingest + summarize + build")
    _add_ingest(refresh)
    _add_summary(refresh)
    refresh.set_defaults(handler="refresh")

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
    return summarize_archive(
        _path(str(ns.output)),
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
    )


def _print_ingest(report: IngestReport) -> None:
    print(
        f"ingest: {report.agents} agents, {report.events} messages/events, "
        f"{report.tool_calls} outer tools, {report.edges} edges from {report.sources} source files "
        f"({report.source_bytes / (1024 * 1024):.1f} MiB); {report.files_changed} archive files changed"
    )


def _print_summaries(report: SummarizeReport) -> None:
    print(
        f"summarize: {report.phases} phases + {report.agent_names} hindsight agent names + "
        f"{report.rollups} calendar rollups; "
        f"cache {report.cache_hits} hit / {report.cache_misses} miss in "
        f"{report.backend_batches} backend batch(es); {report.glossary_terms} glossary terms"
    )


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

    archive = _path(str(ns.output))
    team_slug = str(ns.team)
    started = utc_now()
    command = [PROG, *args]
    ingest_report: IngestReport | None = None
    summary_report: SummarizeReport | None = None
    build_report: dict[str, int] | None = None
    try:
        if handler in ("ingest", "refresh"):
            _, ingest_report = ingest_codex(
                archive,
                _path(str(ns.sessions_root)),
                str(ns.root_session),
                team_slug,
                str(ns.timezone),
            )
            _print_ingest(ingest_report)
        if handler in ("summarize", "refresh"):
            summary_report = _summary_call(ns)
            _print_summaries(summary_report)
        if handler in ("build", "refresh"):
            build_report = build_archive(
                archive, team_slug, phase_minutes=int(ns.phase_minutes)
            )
            print(
                f"build: {build_report['agents']} tracks, {build_report['phases']} phases, "
                f"{build_report['edges']} edges, {build_report['summary_files']} Markdown files; "
                f"{build_report['files_changed']} presentation files changed"
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
    except (AgentNameError, CodexParseError, SummaryError, OSError, ValueError) as error:
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
