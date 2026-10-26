"""Discover a project's agent tree from its coordinators and its working directories.

A project run by persistent coordinators spreads its sessions across harnesses and hosts: a
Claude Code coordinator starts Codex and Muse lanes through agentctl, a lane starts its own
sub-agents, Claude Code keeps Task subagents beside the session, and Codex records each spawned
child in the child's own rollout. No single log holds the tree. This module rebuilds it, without a
model, from what the logs and the agentctl registry do record:

* **native links** -- a Claude Code session's ``<session>/subagents/agent-*.jsonl`` files, a
  Codex rollout's ``thread_spawn.parent_thread_id``, an Orc session's ``parent_id``;
* **agentctl launches** -- an ``agentctl ... start NAME`` command inside a session's log, matched
  to the registry record of that name created within :data:`LAUNCH_WINDOW_S` after it, and from the
  record to the child's own log: by its recorded native session id when there is one, otherwise by
  the harness, working directory and a start within :data:`START_WINDOW_S` of the record.
  ``created_by`` / ``managed_by`` in a record, when present, are used the same way.

Every session is also swept by working directory: anything that ran under a ``--cwd-prefix`` and
that no root reaches is reported as an orphan, which is how a gap in the links shows up. Reads
only; calls no model; writes nothing but the report.
"""

from __future__ import annotations

import json
import re
import shlex
import sqlite3
from collections import deque
from collections.abc import Iterator, Mapping, Sequence
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path

from wrkviz.compressed_sources import (
    is_compressed,
    iter_logical_glob,
    read_first_line,
    read_source_bytes,
    resolve_source,
)

#: A launch command and the registry record it created are seconds apart; the window allows for
#: a slow harness start without letting a later agent that reused the name be mistaken for it.
LAUNCH_WINDOW_S = 900.0
#: A registry record and its harness's first log record are normally one or two seconds apart.
START_WINDOW_S = 300.0
#: Records read from the head of a log while looking for its working directory and start.
HEAD_RECORDS = 200

_LAUNCH = re.compile(
    rb"agentctl(?:[^\s\"\\]*)(?:\s+--?[A-Za-z][\w-]*(?:[ =][^\s\"\\-][^\s\"\\]*)?)*\s+start\s+"
    rb"([A-Za-z0-9][A-Za-z0-9._-]*)"
)
#: A harness started directly from a shell -- a one-shot reviewer or worker -- rather than through
#: agentctl. Its log starts within :data:`EXEC_WINDOW_S` of the command.
_EXEC = re.compile(rb"\b(?:codex\s+exec|claude\s+(?:-p|--print)|muse\s+(?:exec|run|-p))\b")
EXEC_WINDOW_S = 120.0
#: The same launch through a shell variable holding the agentctl path (``$A start NAME``); only
#: tried on a line that mentions agentctl, and only a registered NAME counts.
_VARIABLE_LAUNCH = re.compile(
    rb"\$\{?[A-Za-z_][A-Za-z0-9_]*\}?\s+start\s+([A-Za-z0-9][A-Za-z0-9._-]*)"
)
_PROVIDER_BY_HARNESS = {"claude": "claude", "codex": "codex", "muse": "muse"}


@dataclass
class Session:
    """One agent log: a coordinator, a lane, or a native subagent."""

    provider: str
    session_id: str
    host: str
    path: Path | None
    cwd: str | None
    started_at: float | None
    name: str | None = None
    parent: str | None = None
    link: str | None = None
    native_parent: str | None = None
    children: list[str] = field(default_factory=list)

    @property
    def key(self) -> str:
        return f"{self.provider}:{self.session_id}"


@dataclass(frozen=True)
class RegistryAgent:
    """One agentctl registry record, live or archived."""

    host: str
    name: str
    harness: str
    cwd: str | None
    created_at: float
    native_session: str | None
    created_by: str | None


@dataclass(frozen=True)
class Sources:
    """Where to look, per host label."""

    claude_projects: Sequence[tuple[str, Path]] = ()
    codex_sessions: Sequence[tuple[str, Path]] = ()
    orc_roots: Sequence[tuple[str, Path]] = ()
    muse_sessions: Sequence[tuple[str, Path]] = ()
    registries: Sequence[tuple[str, Path]] = ()


def _iso(raw: object) -> float | None:
    if not isinstance(raw, str) or not raw:
        return None
    try:
        return datetime.fromisoformat(raw.replace("Z", "+00:00")).timestamp()
    except ValueError:
        return None


def _records(path: Path, limit: int) -> Iterator[Mapping[str, object]]:
    physical = resolve_source(path)
    if physical is None:
        return
    try:
        if is_compressed(physical):
            lines: Iterator[bytes] = iter(read_source_bytes(path).split(b"\n"))
        else:
            lines = iter(physical.open("rb"))
        for index, line in enumerate(lines):
            if index >= limit:
                return
            try:
                value = json.loads(line)
            except (json.JSONDecodeError, UnicodeDecodeError):
                continue
            if isinstance(value, dict):
                yield value
    except OSError:
        return


def _claude_head(path: Path) -> tuple[str | None, float | None]:
    cwd: str | None = None
    started: float | None = None
    with path.open("rb") as handle:
        for index, line in enumerate(handle):
            if index >= HEAD_RECORDS or (cwd and started):
                break
            try:
                record = json.loads(line)
            except (json.JSONDecodeError, UnicodeDecodeError):
                continue
            if not isinstance(record, dict):
                continue
            cwd = cwd or (record.get("cwd") if isinstance(record.get("cwd"), str) else None)
            started = started or _iso(record.get("timestamp"))
    return cwd, started


def scan_claude(host: str, projects: Path) -> list[Session]:
    """Top-level Claude Code sessions and their native subagents."""

    sessions: list[Session] = []
    for path in sorted(projects.glob("*/*.jsonl")):
        cwd, started = _claude_head(path)
        root = Session("claude", path.stem, host, path, cwd, started)
        sessions.append(root)
        for child in sorted((path.parent / path.stem / "subagents").glob("**/agent-*.jsonl")):
            agent_id = child.stem.removeprefix("agent-")
            _, child_started = _claude_head(child)
            sessions.append(
                Session(
                    "claude-subagent", f"{path.stem}/{agent_id}", host, child, cwd,
                    child_started, native_parent=root.key, link="claude-subagent",
                )
            )
    return sessions


def scan_codex(host: str, root: Path) -> list[Session]:
    """Codex rollouts, with each spawned child's parent thread."""

    sessions: list[Session] = []
    for path in iter_logical_glob(root, "rollout-*.jsonl"):
        try:
            meta_record = json.loads(read_first_line(path))
        except (OSError, json.JSONDecodeError, UnicodeDecodeError):
            continue
        meta = meta_record.get("payload") if isinstance(meta_record, dict) else None
        if not isinstance(meta, dict) or not isinstance(meta.get("id"), str):
            continue
        source = meta.get("source")
        spawn = source.get("subagent", {}).get("thread_spawn", {}) if isinstance(source, dict) else {}
        parent = (
            spawn.get("parent_thread_id") if isinstance(spawn, dict) else None
        ) or meta.get("parent_thread_id")
        sessions.append(
            Session(
                "codex", str(meta["id"]), host, path,
                meta.get("cwd") if isinstance(meta.get("cwd"), str) else None,
                _iso(meta.get("timestamp")),
                native_parent=f"codex:{parent}" if isinstance(parent, str) and parent else None,
                link="codex-spawn" if parent else None,
            )
        )
    return sessions


def scan_orc(host: str, root: Path) -> list[Session]:
    """Orc sessions from the session index, with their recorded parents."""

    index = root / ".orc" / "index.db"
    if not index.is_file():
        return []
    connection = sqlite3.connect(f"file:{index}?mode=ro&immutable=1", uri=True)
    try:
        columns = {row[1] for row in connection.execute("PRAGMA table_info(sessions)")}
        cwd_column = "cwd" if "cwd" in columns else "NULL"
        rows = connection.execute(
            f"SELECT id, parent_id, name, created_at, {cwd_column} FROM sessions"
        ).fetchall()
    finally:
        connection.close()
    sessions = []
    for session_id, parent, name, created, cwd in rows:
        path = root / ".orc" / "sessions" / str(session_id) / "session.db"
        sessions.append(
            Session(
                "orc", str(session_id), host, path, cwd, _iso(created), name=name,
                native_parent=f"orc:{parent}" if parent else None,
                link="orc-parent" if parent else None,
            )
        )
    return sessions


def scan_muse(host: str, root: Path) -> list[Session]:
    """Muse sessions, by the working directory their route facts record."""

    sessions = []
    for path in sorted(root.glob("*/*/*/*/session.jsonl")):
        cwd: str | None = None
        started: float | None = None
        for record in _records(path, HEAD_RECORDS):
            payload = record.get("payload")
            if isinstance(payload, dict) and payload.get("kind") == "route_facts":
                facts = payload.get("record")
                if isinstance(facts, dict) and isinstance(facts.get("cwd"), str):
                    cwd = str(facts["cwd"]).rstrip("/")
                recorded = record.get("recorded_at")
                if isinstance(recorded, (int, float)):
                    started = float(recorded) / 1e6
                break
        sessions.append(Session("muse", path.parent.name, host, path, cwd, started))
    return sessions


def load_registry(host: str, registry: Path) -> list[RegistryAgent]:
    """Every agent record in an agentctl registry, live and archived."""

    agents = []
    for path in sorted([*registry.glob("*/agent.json"), *registry.glob("archive/**/agent.json")]):
        try:
            record = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError, UnicodeDecodeError):
            continue
        if not isinstance(record, dict) or not isinstance(record.get("name"), str):
            continue
        native = record.get("native_session")
        native_value = native.get("value") if isinstance(native, dict) else None
        session: str | None = native_value if isinstance(native_value, str) else None
        raw_arguments = record.get("arguments")
        arguments: list[object] = list(raw_arguments) if isinstance(raw_arguments, list) else []
        for flag, value in zip(arguments, arguments[1:]):
            if flag in ("--session-id", "--resume") and isinstance(value, str):
                session = session or value
        creator = record.get("created_by") or record.get("managed_by")
        agents.append(
            RegistryAgent(
                host, str(record["name"]), str(record.get("harness") or "unknown"),
                record.get("cwd") if isinstance(record.get("cwd"), str) else None,
                float(record.get("created_at") or 0.0),
                session,
                creator if isinstance(creator, str) else None,
            )
        )
    return agents


def load_queue(host: str, registry: Path) -> list[tuple[str, str, float]]:
    """``(owner NAME, text, queued_at)`` for every message ever queued to a registered agent."""

    messages = []
    for path in sorted(
        [*registry.glob("*/queue/*/*.json"), *registry.glob("archive/**/queue/*/*.json")]
    ):
        try:
            record = json.loads(path.read_text(encoding="utf-8"))
            owner = json.loads((path.parents[2] / "agent.json").read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError, UnicodeDecodeError):
            continue
        text, name = record.get("text"), owner.get("name")
        if isinstance(text, str) and isinstance(name, str) and text.strip():
            messages.append((name, text, float(record.get("queued_at") or 0.0)))
    return messages


#: How a coordinator addresses a lane in the first prompt of the lane's new log after a restart:
#: "→ NAME: ...", "SENDER -> NAME: ...", or a brief that opens "You are NAME, ...".
_ADDRESSED = re.compile(
    r"^\s*(?:→\s*|[A-Za-z0-9][\w.-]*\s*(?:->|→)\s*|you are\s+(?:lane\s+)?)`?([A-Za-z0-9][\w.-]*[A-Za-z0-9])`?",
    re.IGNORECASE,
)


def _prompt_key(text: str) -> str:
    return " ".join(text.split())[:160]


def first_prompt(session: Session) -> str | None:
    """The first instruction a session received, as typed: what a restarted lane opens with."""

    if session.path is None or session.provider not in ("claude", "codex"):
        return None
    for record in _records(session.path, HEAD_RECORDS):
        if session.provider == "codex":
            payload = record.get("payload")
            if record.get("type") != "response_item" or not isinstance(payload, dict):
                continue
            if payload.get("type") != "message" or payload.get("role") != "user":
                continue
            content = payload.get("content")
            texts = [
                str(block.get("text")) for block in content if isinstance(block, dict)
                and isinstance(block.get("text"), str)
            ] if isinstance(content, list) else []
            text = "\n".join(texts).strip()
            if text and not text.startswith("<") and not text.startswith("# AGENTS.md"):
                return text
        else:
            if record.get("type") != "user" or record.get("isMeta"):
                continue
            message = record.get("message")
            content = message.get("content") if isinstance(message, dict) else None
            if isinstance(content, str) and content.strip() and not content.startswith("<"):
                return content
    return None


def _record_time(line: bytes) -> float | None:
    try:
        record = json.loads(line)
    except (json.JSONDecodeError, UnicodeDecodeError):
        return None
    if not isinstance(record, dict):
        return None
    when = _iso(record.get("timestamp"))
    if when is None and isinstance(record.get("recorded_at"), (int, float)):
        when = float(record["recorded_at"]) / 1e6
    return when


def _log_lines(path: Path) -> Iterator[bytes]:
    if path.suffix == ".db":
        return
    physical = resolve_source(path)
    if physical is None:
        return
    try:
        if is_compressed(physical):
            yield from read_source_bytes(path).split(b"\n")
        else:
            with physical.open("rb") as handle:
                yield from handle
    except OSError:
        return


def launches(path: Path) -> list[tuple[float, str]]:
    """``(when, NAME)`` for every ``agentctl ... start NAME`` in a log, from its records' times."""

    found: list[tuple[float, str]] = []
    for line in _log_lines(path):
        if b"agentctl" not in line or b"start" not in line:
            continue
        names = [
            match.group(1).decode()
            for pattern in (_LAUNCH, _VARIABLE_LAUNCH)
            for match in pattern.finditer(line)
        ]
        if not names:
            continue
        when = _record_time(line)
        if when is not None:
            found.extend((when, name) for name in dict.fromkeys(names))
    return found


def invocations(path: Path) -> list[float]:
    """When a log ran a harness CLI directly (``codex exec``, ``claude -p``, ...)."""

    found: list[float] = []
    for line in _log_lines(path):
        if b"exec" not in line and b"claude" not in line and b"muse" not in line:
            continue
        if not _EXEC.search(line):
            continue
        when = _record_time(line)
        if when is not None:
            found.append(when)
    return found


def _under(cwd: str | None, prefixes: Sequence[str]) -> bool:
    if not cwd:
        return False
    normalized = cwd.rstrip("/") + "/"
    return any(normalized.startswith(prefix.rstrip("/") + "/") for prefix in prefixes)


@dataclass
class Discovery:
    """The tree, the orphans, and what could not be resolved."""

    sessions: dict[str, Session]
    roots: list[str]
    orphans: list[str]
    unresolved_launches: list[dict[str, object]]
    #: ``agentctl ... start NAME`` text whose NAME no registry ever held -- briefs, help text.
    ignored_mentions: int = 0

    def descendants(self, key: str) -> list[str]:
        seen: list[str] = []
        queue = deque(self.sessions[key].children)
        while queue:
            child = queue.popleft()
            if child in seen:
                continue
            seen.append(child)
            queue.extend(self.sessions[child].children)
        return seen

    def to_json_obj(self) -> dict[str, object]:
        def row(key: str) -> dict[str, object]:
            session = self.sessions[key]
            return {
                "key": key, "provider": session.provider, "session_id": session.session_id,
                "host": session.host, "name": session.name, "cwd": session.cwd,
                "started_at": session.started_at, "parent": session.parent,
                "link": session.link, "path": str(session.path) if session.path else None,
            }

        return {
            "schema_version": 1,
            "roots": [
                {
                    **row(root),
                    "direct_children": len(self.sessions[root].children),
                    "descendants": len(self.descendants(root)),
                    "by_link": _count_links(self, self.descendants(root)),
                }
                for root in self.roots
            ],
            "tree": [row(key) for root in self.roots for key in self.descendants(root)],
            "orphans": [row(key) for key in self.orphans],
            "unresolved_launches": self.unresolved_launches,
            "ignored_mentions": self.ignored_mentions,
        }


def _count_links(discovery: Discovery, keys: Sequence[str]) -> dict[str, int]:
    counts: dict[str, int] = {}
    for key in keys:
        link = discovery.sessions[key].link or "unknown"
        counts[link] = counts.get(link, 0) + 1
    return dict(sorted(counts.items()))


def discover(
    sources: Sources,
    roots: Sequence[str],
    cwd_prefixes: Sequence[str],
    since: float | None = None,
) -> Discovery:
    """Rebuild the tree under *roots* (``provider:session-id``) and sweep *cwd_prefixes*."""

    sessions: dict[str, Session] = {}
    for host, path in sources.claude_projects:
        for session in scan_claude(host, path):
            sessions.setdefault(session.key, session)
    for host, path in sources.codex_sessions:
        for session in scan_codex(host, path):
            sessions.setdefault(session.key, session)
    for host, path in sources.orc_roots:
        for session in scan_orc(host, path):
            sessions.setdefault(session.key, session)
    for host, path in sources.muse_sessions:
        for session in scan_muse(host, path):
            sessions.setdefault(session.key, session)
    registry = [agent for host, path in sources.registries for agent in load_registry(host, path)]
    missing = [root for root in roots if root not in sessions]
    if missing:
        raise ValueError("root session(s) not found in any source: " + ", ".join(missing))

    def attach(child: str, parent: str, link: str) -> None:
        session = sessions[child]
        if session.parent is not None or child == parent:
            return
        session.parent = parent
        session.link = link
        sessions[parent].children.append(child)

    for key, session in sessions.items():
        if session.native_parent and session.native_parent in sessions:
            attach(key, session.native_parent, session.link or "native")

    by_provider_host: dict[tuple[str, str], list[Session]] = {}
    for session in sessions.values():
        by_provider_host.setdefault((session.provider, session.host), []).append(session)
    claimed: set[str] = set()

    def resolve(agent: RegistryAgent) -> str | None:
        provider = _PROVIDER_BY_HARNESS.get(agent.harness)
        if provider is None:
            return None
        if agent.native_session and f"{provider}:{agent.native_session}" in sessions:
            return f"{provider}:{agent.native_session}"
        best: Session | None = None
        for candidate in by_provider_host.get((provider, agent.host), ()):
            if candidate.key in claimed or candidate.started_at is None:
                continue
            if agent.cwd and candidate.cwd and candidate.cwd.rstrip("/") != agent.cwd.rstrip("/"):
                continue
            delay = candidate.started_at - agent.created_at
            if -5.0 <= delay <= START_WINDOW_S and (
                best is None or candidate.started_at < (best.started_at or 0.0)
            ):
                best = candidate
        return best.key if best is not None else None

    resolved: dict[RegistryAgent, str] = {}
    for agent in sorted(registry, key=lambda item: item.created_at):
        match = resolve(agent)
        if match is not None:
            resolved[agent] = match
            claimed.add(match)
            sessions[match].name = sessions[match].name or agent.name
    by_name: dict[tuple[str, str], list[RegistryAgent]] = {}
    for agent in registry:
        by_name.setdefault((agent.host, agent.name), []).append(agent)

    unresolved: list[dict[str, object]] = []
    mentions = 0
    scanned: set[str] = set()
    queued: dict[tuple[str, str], list[tuple[str, float]]] = {}
    for host, path in sources.registries:
        for name, text, at in load_queue(host, path):
            queued.setdefault((host, _prompt_key(text)), []).append((name, at))
    sessions_by_agent: dict[tuple[str, str], list[str]] = {}
    for agent, key in resolved.items():
        sessions_by_agent.setdefault((agent.host, agent.name), []).append(key)

    def scan_launches(parent_key: str) -> None:
        nonlocal mentions
        parent = sessions[parent_key]
        if parent.path is None or parent.provider not in ("claude", "codex", "muse", "claude-subagent"):
            return
        for when in invocations(parent.path):
            exec_calls.append((when, parent_key, parent.host))
        for when, name in launches(parent.path):
            named = by_name.get((parent.host, name), [])
            if not named:
                # A name no registry on this host ever held: prose or a usage example that
                # happens to read like a launch, not a launch.
                mentions += 1
                continue
            # The record is normally created by the launch itself; one re-created later
            # (an adopt or a restart) still names the session the launch started.
            candidates = [
                agent for agent in named
                if 0.0 <= agent.created_at - when <= LAUNCH_WINDOW_S
                or (
                    agent in resolved
                    and 0.0
                    <= (sessions[resolved[agent]].started_at or -1e18) - when
                    <= LAUNCH_WINDOW_S
                )
            ]
            launched = min(candidates, key=lambda item: item.created_at, default=None)
            if launched is None or launched not in resolved:
                unresolved.append(
                    {"parent": parent_key, "name": name, "at": when,
                     "reason": "no record created by this launch" if launched is None
                     else "no matching log"}
                )
                continue
            attach(resolved[launched], parent_key, "agentctl-launch")

    def reached_from_roots() -> set[str]:
        found: set[str] = set(roots)
        frontier = deque(roots)
        while frontier:
            for child in sessions[frontier.popleft()].children:
                if child not in found:
                    found.add(child)
                    frontier.append(child)
        return found

    prompts: dict[str, str | None] = {}
    exec_calls: list[tuple[float, str, str]] = []
    changed = True
    while changed:
        changed = False
        for key in sorted(reached_from_roots() - scanned):
            scanned.add(key)
            before = len(sessions[key].children)
            scan_launches(key)
            changed = changed or len(sessions[key].children) != before
        # A lane restarted in its pane opens a new log whose first prompt is a message that was
        # queued to that agent: link the new log to the agent's earlier session.
        for key, session in sessions.items():
            if session.parent is not None or key in roots or not _under(session.cwd, cwd_prefixes):
                continue
            if key not in prompts:
                prompts[key] = first_prompt(session)
            prompt = prompts[key]
            if prompt is None:
                continue
            owners = {
                name for name, at in queued.get((session.host, _prompt_key(prompt)), ())
                if session.started_at is None or at <= session.started_at + 60.0
            }
            addressed = _ADDRESSED.match(prompt)
            if not owners and addressed and (session.host, addressed.group(1)) in by_name:
                owners = {addressed.group(1)}
            if len(owners) != 1:
                continue
            owner = owners.pop()
            earlier = [
                candidate for candidate in sessions_by_agent.get((session.host, owner), ())
                if candidate != key
                and (sessions[candidate].started_at or 0.0) <= (session.started_at or 0.0)
            ]
            if earlier:
                original = max(earlier, key=lambda item: sessions[item].started_at or 0.0)
                attach(key, original, "agentctl-restart")
                session.name = session.name or owner
                sessions_by_agent.setdefault((session.host, owner), []).append(key)
                changed = True
        # A harness run straight from a reached session's shell: the unlinked log on the same host
        # that starts soonest after the command, if no other reached session ran one closer.
        for key, session in sessions.items():
            if (
                session.parent is not None or key in roots or session.started_at is None
                or session.provider == "claude-subagent" or not _under(session.cwd, cwd_prefixes)
            ):
                continue
            prior = [
                (session.started_at - when, parent_key)
                for when, parent_key, host in exec_calls
                if host == session.host and 0.0 <= session.started_at - when <= EXEC_WINDOW_S
                and parent_key != key
            ]
            if not prior:
                continue
            prior.sort()
            if len(prior) > 1 and prior[1][0] - prior[0][0] < 1.0 and prior[1][1] != prior[0][1]:
                continue  # two sessions ran a harness at the same moment: ambiguous
            attach(key, prior[0][1], "shell-exec")
            changed = True
    # A registry record naming its creator links even when the launch command left no trace.
    by_agent_name = {(agent.host, agent.name): key for agent, key in resolved.items()}
    for agent, key in resolved.items():
        if agent.created_by and (agent.host, agent.created_by) in by_agent_name:
            attach(key, by_agent_name[(agent.host, agent.created_by)], "agentctl-created-by")

    reached: set[str] = set(roots)
    for root in roots:
        reached.update(Discovery(sessions, [root], [], []).descendants(root))
    orphans = sorted(
        (
            key for key, session in sessions.items()
            if key not in reached
            and session.provider != "claude-subagent"
            and _under(session.cwd, cwd_prefixes)
            and (since is None or (session.started_at or 0.0) >= since or _recent(session, since))
        ),
        key=lambda key: (sessions[key].started_at or 0.0, key),
    )
    return Discovery(sessions, list(roots), orphans, unresolved, mentions)


def _recent(session: Session, since: float) -> bool:
    try:
        return session.path is not None and session.path.stat().st_mtime >= since
    except OSError:
        return False


def format_markdown(discovery: Discovery) -> str:
    """A short human report: per-root counts, then orphans."""

    lines = ["| root | name | direct | descendants | by link |", "|---|---|---|---|---|"]
    for root in discovery.roots:
        descendants = discovery.descendants(root)
        links = ", ".join(f"{k} {v}" for k, v in _count_links(discovery, descendants).items())
        session = discovery.sessions[root]
        lines.append(
            f"| {root} | {session.name or ''} | {len(session.children)} | {len(descendants)} | {links} |"
        )
    lines += ["", f"orphans (under a --cwd-prefix, reached by no root): {len(discovery.orphans)}"]
    for key in discovery.orphans[:200]:
        session = discovery.sessions[key]
        started = (
            datetime.fromtimestamp(session.started_at, timezone.utc).strftime("%Y-%m-%d %H:%M")
            if session.started_at else "?"
        )
        lines.append(f"- {started} {key} {session.name or ''} cwd={session.cwd}")
    lines += [
        "",
        f"launches of a registered name with no resolvable child: "
        f"{len(discovery.unresolved_launches)}; launch-like text naming no registered agent "
        f"(ignored): {discovery.ignored_mentions}",
    ]
    return "\n".join(lines) + "\n"


def parse_labelled(values: Sequence[str], flag: str) -> list[tuple[str, Path]]:
    """``LABEL=DIR`` pairs, as given to the repeatable source flags."""

    pairs = []
    for value in values:
        label, separator, directory = value.partition("=")
        if not separator or not label or not directory:
            raise ValueError(f"{flag} expects LABEL=DIR, got {shlex.quote(value)}")
        pairs.append((label, Path(directory).expanduser()))
    return pairs


__all__ = [
    "Discovery",
    "RegistryAgent",
    "Session",
    "Sources",
    "discover",
    "format_markdown",
    "launches",
    "load_registry",
    "parse_labelled",
]
