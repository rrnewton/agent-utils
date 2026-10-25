"""Parent-child responsiveness of an agentctl team, measured from its own records.

A coordinator that delegates to persistent subagents keeps them busy only if it learns
promptly that a child stopped and then acts. This module rebuilds that loop from the logs
the team already writes, without calling a model:

* the agentctl registry: every agent record (live and archived) and every queue message,
  with the times agentctl queued it and confirmed it typed into the target;
* the coordinator's Claude Code transcript: its turns, the prompts that started them
  (queued messages, chat-bridge prompts, scheduled-task fires) and the tool calls it made;
* each child's own log: Claude Code transcripts found by the agent's native session id,
  and Muse session logs matched by the working directory each log records when it starts.

From those it derives, for every child turn end that the parent had to answer, when the
parent was told (a message the child sent, a watch-agents notice, a scheduled sweep), when
it first looked at the child and when it first acted on it; and, for every idle stretch of
every child, what the child was waiting for. ``USER_GUIDE.md`` defines each figure.
"""

from __future__ import annotations

import bisect
from collections import Counter
from collections.abc import Iterable, Iterator, Mapping, Sequence
from dataclasses import dataclass, field
from datetime import datetime, timezone
from fnmatch import fnmatchcase
import json
from pathlib import Path
import re


# Prefixes of queue message IDs that agentctl writes for its own senders.
WATCH_ID_PREFIX = "agent-watch-"
CHAT_ID_PREFIX = "chat-"

# Claude Code system records.
_SCHEDULED_FIRE = "scheduled_task_fire"
_TURN_DURATION = "turn_duration"
_TASK_NOTIFICATION = "<task-notification>"

# Verbs in a coordinator tool call that count as acting on a child, as opposed to looking.
ACTING_VERBS = ("stop", "send", "answer", "start")

# A queued prompt the child picks up within this many seconds of going idle was already
# waiting for it: the parent had answered before the child stopped.
PRE_QUEUED_SECONDS = 180.0
# A child's own message counts as notice of a turn end if queued at most this long before it.
CHILD_MESSAGE_LEAD_SECONDS = 900.0
# Records read from the head of a Muse session log while looking for its working directory.
MUSE_HEADER_RECORDS = 200
# A Muse session log belongs to a child registered at most this long before the log started.
MUSE_START_WINDOW = 600.0
# How long after a turn end to look for the parent's response when the child never resumed.
OPEN_HORIZON_SECONDS = 6 * 3600.0

_ASK = re.compile(r"\?\s*$|\bmay i\b|\bshall i\b|\bshould i\b|\bdo you want\b")
_AWAIT = re.compile(
    r"waiting (?:for|on) (?:your|the coordinator|their|an? )|all[- ]clear|\bholding\b|on hold|"
    r"hold acknowledged|your (?:decision|answer|go|call)|landing (?:turn|queue)|"
    r"coordinator (?:now )?(?:has|knows)|until you|when you (?:say|reply)"
)


class ResponsivenessError(ValueError):
    """A source is missing or unreadable in a way that makes the report meaningless."""


@dataclass(frozen=True)
class Sources:
    """Where one team's records live."""

    registry: Path
    coordinator_transcript: Path
    coordinator: str = "coordinator"
    claude_projects: Path | None = None
    muse_sessions: Path | None = None
    sweep_marker: str | None = None
    exclude: tuple[str, ...] = ()


@dataclass
class AgentRecord:
    """One child as the registry describes it."""

    name: str
    harness: str
    created: float
    cwd: str | None
    slot: str | None
    native_session: str | None
    log: Path | None = None
    stopped: float | None = None


@dataclass(frozen=True)
class QueueMessage:
    """One message from any agentctl queue."""

    owner: str
    message_id: str
    text: str
    queued: float | None
    confirmed: float | None


@dataclass(frozen=True)
class CoordinatorInput:
    """A prompt that reached the coordinator: it started a turn or was absorbed into one."""

    at: float
    kind: str
    agent: str | None
    mid_turn: bool


@dataclass(frozen=True)
class Action:
    """A coordinator tool call (or queued message) addressed to one child."""

    at: float
    verb: str
    agent: str


@dataclass
class ChildTurn:
    """One turn of a child: what started it, when it ended, and its last reply."""

    start: float
    trigger: str  # first, parent, self
    end: float | None = None
    reply: str = ""


@dataclass
class Episode:
    """A child turn end that the parent had to answer."""

    agent: str
    harness: str
    turn_end: float
    verdict: str
    next_trigger: str
    resumed: float | None
    path: str
    first_notice: str | None
    parent_busy: bool
    notified: float | None
    observed: float | None
    acted: float | None
    act_verb: str | None
    reply: str

    def latency(self, at: float | None) -> float | None:
        """Seconds from the turn end to ``at``."""

        return None if at is None else at - self.turn_end


@dataclass(frozen=True)
class Gap:
    """One idle stretch of one child, from a turn end to its next turn or its stop."""

    agent: str
    harness: str
    start: float
    end: float
    waiting_for: str  # parent, self, stop
    verdict: str


@dataclass
class Report:
    """Everything the command prints."""

    coordinator: str
    window_start: float
    window_end: float
    coordinator_turns: list[tuple[float, float]]
    inputs: list[CoordinatorInput]
    agents: list[AgentRecord]
    episodes: list[Episode]
    gaps: list[Gap]
    messages: list[QueueMessage] = field(default_factory=list)


# ------------------------------------------------------------------------------- reading


def _iso_seconds(raw: object) -> float | None:
    if not isinstance(raw, str):
        return None
    try:
        return datetime.fromisoformat(raw.replace("Z", "+00:00")).timestamp()
    except ValueError:
        return None


def _number(raw: object) -> float | None:
    if isinstance(raw, bool) or not isinstance(raw, (int, float)):
        return None
    return float(raw)


def _string(raw: object) -> str | None:
    return raw if isinstance(raw, str) else None


def _json_lines(path: Path) -> Iterator[Mapping[str, object]]:
    with path.open(encoding="utf-8", errors="replace") as handle:
        for line in handle:
            try:
                record = json.loads(line)
            except ValueError:
                continue
            if isinstance(record, dict):
                yield record


def _read_json(path: Path) -> Mapping[str, object] | None:
    try:
        record = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    return record if isinstance(record, dict) else None


def _record_dirs(registry: Path) -> Iterator[Path]:
    for parent in (registry, registry / "archive"):
        if parent.is_dir():
            for child in sorted(parent.iterdir()):
                if (child / "agent.json").is_file():
                    yield child


def load_agents(registry: Path, coordinator: str) -> list[AgentRecord]:
    """Every registered child, live or archived, oldest first."""

    agents: list[AgentRecord] = []
    for directory in _record_dirs(registry):
        record = _read_json(directory / "agent.json")
        if record is None:
            continue
        name = _string(record.get("name"))
        if not name or name == coordinator:
            continue
        native = record.get("native_session")
        session = _string(native.get("value")) if isinstance(native, dict) else None
        agents.append(AgentRecord(
            name=name,
            harness=_string(record.get("harness")) or "unknown",
            created=_number(record.get("created_at")) or 0.0,
            cwd=_string(record.get("cwd")),
            slot=_string(record.get("slot")),
            native_session=session,
        ))
    agents.sort(key=lambda agent: agent.created)
    return agents


def load_messages(registry: Path) -> list[QueueMessage]:
    """Every queue message in every state, attributed to the queue's owner."""

    messages: list[QueueMessage] = []
    for directory in _record_dirs(registry):
        owner_record = _read_json(directory / "agent.json") or {}
        owner = _string(owner_record.get("name")) or directory.name
        for path in sorted((directory / "queue").glob("*/*.json")):
            record = _read_json(path)
            if record is None:
                continue
            messages.append(QueueMessage(
                owner=owner,
                message_id=_string(record.get("id")) or path.stem,
                text=_string(record.get("text")) or "",
                queued=_number(record.get("queued_at")),
                confirmed=_number(record.get("confirmed_at")),
            ))
    return messages


def message_author(text: str, names: Iterable[str]) -> str | None:
    """The child a coordinator message came from, by the ``NAME:`` convention.

    agentctl records no sender, so a child names itself: the message starts with its
    registered name followed by a colon or a space, optionally after ``LANDED (``.
    """

    head = text.lstrip()
    if head.startswith("LANDED ("):
        head = head[len("LANDED ("):]
    best: str | None = None
    for name in names:
        if head.startswith(name) and head[len(name):len(name) + 1] in (":", " ", ")", ","):
            if best is None or len(name) > len(best):
                best = name
    return best


def _prompt_text(record: Mapping[str, object]) -> str | None:
    message = record.get("message")
    if not isinstance(message, dict):
        return None
    content = message.get("content")
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        for block in content:
            if isinstance(block, dict) and block.get("type") == "text":
                return _string(block.get("text")) or ""
    return None


def classify_input(text: str, names: Iterable[str], sweep_marker: str | None) -> tuple[str, str | None]:
    """The kind of a coordinator prompt and the first child it concerns, when one is named."""

    head = text[:300]
    if sweep_marker and sweep_marker in head:
        return "sweep", None
    if head.startswith(_TASK_NOTIFICATION):
        return "task-notification", None
    if head.startswith("[agentctl watch-agents]"):
        listed = watch_agents(text)
        return "watch", listed[0] if listed else None
    author = message_author(text, names)
    if author is not None:
        return "child-message", author
    if "chat bridge" in head:
        return "chat", None
    return "other", None


def watch_agents(text: str) -> list[str]:
    """Every agent a watch-agents notice lists, one ``- NAME: condition`` line each."""

    return re.findall(r"^- ([a-z][a-z0-9-]*):", text, re.MULTILINE)


def _name_pattern(names: Iterable[str]) -> re.Pattern[str]:
    alternatives = "|".join(sorted((re.escape(name) for name in names if name), key=len, reverse=True))
    return re.compile(r"(?<![A-Za-z0-9-])(" + alternatives + r")(?![A-Za-z0-9-])")


# One shell command head: an optional `env -C DIR`, a path ending in `agentctl` (or `wrkslots`),
# at most a few `--option [value]` pairs, then the verb and its first operand.
_HEAD = re.compile(
    r"^(?:(?:timeout\s+\S+|nice|nohup|exec|time|command)\s+)*(?:\S+=\S*\s+)*"
    r"(?:env(?:\s+-C\s+\S+)?\s+(?:\S+=\S*\s+)*)?(?:\S*/)?(agentctl|wrkslots)\S*"
    r"(?:\s+--?[A-Za-z][\w-]*(?:[= ](?!-)\S+)?){0,6}\s+([a-z][a-z-]*)((?:\s+\S+){0,4})"
)
_FOR_LOOP = re.compile(r"\bfor\s+(\w+)\s+in\s+([^;]{0,2000}?)\s*;?\s*do\b")
_HEREDOC = re.compile(r"<<-?\s*['\"]?(\w+)['\"]?")
# Longest tool-call command scanned; longer commands are mostly embedded documents.
COMMAND_SCAN_LIMIT = 16_384


_ASSIGNMENT = re.compile(r"(?:^|[;&|\s])([A-Za-z_]\w*)=([^\s;&|'\"]+)")


def _expand(command: str) -> str:
    """Substitute the command's own plain ``VAR=value`` assignments, so ``WS=.../wrkslots;
    $WS remove SLOT`` reads as wrkslots. Two passes resolve ``WS=$H/worktrees/wrkslots``."""

    values: dict[str, str] = {}
    for match in _ASSIGNMENT.finditer(command):
        values.setdefault(match.group(1), match.group(2))
    if not values:
        return command

    def substitute(text: str) -> str:
        return re.sub(r"\$\{?([A-Za-z_]\w*)\}?", lambda m: values.get(m.group(1), m.group(0)), text)

    for name, value in list(values.items()):
        values[name] = substitute(substitute(value))
    return substitute(command)


def _segments(command: str) -> Iterator[str]:
    """The simple commands of a shell command line, skipping heredoc bodies."""

    terminator: str | None = None
    for line in _expand(command[:COMMAND_SCAN_LIMIT]).splitlines():
        if terminator is not None:
            if line.strip() == terminator:
                terminator = None
            continue
        heredoc = _HEREDOC.search(line)
        if heredoc:
            terminator = heredoc.group(1)
        for segment in re.split(r"&&|\|\||[;|&]|\$\(|`", line):
            segment = segment.strip().lstrip("({ ").strip()
            if segment.startswith(("do ", "then ", "else ")):
                segment = segment.split(None, 1)[1] if " " in segment else ""
            if segment:
                yield segment


def _command_targets(command: str, pattern: re.Pattern[str]) -> list[tuple[str, str]]:
    """(verb, child) pairs one coordinator Bash command addresses.

    Only a simple command that starts with ``agentctl`` counts, and only its own operand:
    ``agentctl stop a; agentctl read b`` stops ``a`` and reads ``b``, and a name inside message
    text or a heredoc is not an action on that child. A ``for VAR in NAMES; do agentctl VERB
    $VAR`` loop applies the verb to each listed name.
    """

    loops = {match.group(1): pattern.findall(match.group(2)) for match in _FOR_LOOP.finditer(command)}
    pairs: list[tuple[str, str]] = []
    for segment in _segments(command):
        head = _HEAD.match(segment)
        if head is None or head.group(1) != "agentctl":
            continue
        verb = head.group(2) if head.group(2) in ACTING_VERBS else "read"
        operands = head.group(3).split()
        operand = operands[0].strip("'\"") if operands else ""
        variable = re.fullmatch(r"\$\{?(\w+)\}?", operand)
        if variable and variable.group(1) in loops:
            pairs.extend((verb, agent) for agent in loops[variable.group(1)])
        elif pattern.fullmatch(operand):
            pairs.append((verb, operand))
    return pairs


def _slot_targets(command: str) -> list[str]:
    """Slots a Bash command reads the handoff of, or removes, with wrkslots."""

    slots: list[str] = []
    for segment in _segments(command):
        head = _HEAD.match(segment)
        if head is None or head.group(1) != "wrkslots" or head.group(2) not in ("read-handoff", "remove"):
            continue
        operands = [token for token in head.group(3).split() if not token.startswith("-")]
        if operands and re.fullmatch(r"[a-z0-9][a-z0-9-]*", operands[0]):
            slots.append(operands[0])
    return slots


def _bash_command(block: Mapping[str, object]) -> str | None:
    if block.get("name") != "Bash":
        return None
    tool_input = block.get("input")
    return _string(tool_input.get("command")) if isinstance(tool_input, dict) else None


def load_coordinator(
    transcript: Path,
    names: Sequence[str],
    slots: Mapping[str, list[AgentRecord]],
    sweep_marker: str | None,
) -> tuple[list[tuple[float, float]], list[CoordinatorInput], list[Action]]:
    """The coordinator's turns, the prompts it received, and its Bash commands per child.

    A turn ends at its ``turn_duration`` record. A turn the harness never closed (the process
    was killed) ends at its last record when the next prompt starts a new one.
    """

    turns: list[tuple[float, float]] = []
    inputs: list[CoordinatorInput] = []
    actions: list[Action] = []
    started: float | None = None
    last: float | None = None
    pattern = _name_pattern(names) if names else None

    def begin(at: float) -> None:
        nonlocal started
        if started is not None and last is not None:
            turns.append((started, last))
        started = at

    for record in _json_lines(transcript):
        at = _iso_seconds(record.get("timestamp"))
        if at is None:
            continue
        kind = record.get("type")
        if kind == "user" and not record.get("isMeta"):
            text = _prompt_text(record)
            if text is None:
                last = at
                continue  # a tool result
            input_kind, agent = classify_input(text, names, sweep_marker)
            inputs.append(CoordinatorInput(at, input_kind, agent, False))
            if input_kind == "watch":
                inputs.extend(CoordinatorInput(at, "watch", other, False) for other in watch_agents(text)[1:])
            begin(at)
        elif kind == "attachment":
            attachment = record.get("attachment")
            if isinstance(attachment, dict) and attachment.get("type") == "queued_command":
                prompt = attachment.get("prompt")
                text = prompt if isinstance(prompt, str) else ""
                input_kind, agent = classify_input(text, names, sweep_marker)
                inputs.append(CoordinatorInput(at, input_kind, agent, True))
                if input_kind == "watch":
                    inputs.extend(CoordinatorInput(at, "watch", other, True) for other in watch_agents(text)[1:])
        elif kind == "system" and record.get("subtype") == _SCHEDULED_FIRE:
            prompt = _string(record.get("prompt")) or ""
            sweep = sweep_marker is None or sweep_marker in prompt
            inputs.append(CoordinatorInput(at, "sweep" if sweep else "timer", None, False))
            begin(at)
        elif kind == "system" and record.get("subtype") == _TURN_DURATION:
            if started is not None:
                turns.append((started, at))
            started = None
        elif kind == "assistant":
            started = at if started is None else started
            message = record.get("message")
            content = message.get("content") if isinstance(message, dict) else None
            for block in content if isinstance(content, list) else []:
                if not isinstance(block, dict) or block.get("type") != "tool_use":
                    continue
                command = _bash_command(block)
                if command is None:
                    continue
                if "agentctl" in command and pattern is not None:
                    actions.extend(Action(at, verb, agent) for verb, agent in _command_targets(command, pattern))
                if "wrkslots" in command:
                    for slot in _slot_targets(command):
                        owners = [agent for agent in slots.get(slot, []) if agent.created <= at]
                        if owners:
                            actions.append(Action(at, "remove", max(owners, key=lambda a: a.created).name))
        if kind in ("user", "assistant") and started is not None:
            last = at
    actions.sort(key=lambda action: action.at)
    return turns, inputs, actions


def claude_turns(path: Path) -> list[ChildTurn]:
    """Turns of a Claude Code transcript; ``turn_duration`` closes each one."""

    turns: list[ChildTurn] = []
    current: ChildTurn | None = None
    last_reply = ""
    last: float | None = None
    for record in _json_lines(path):
        at = _iso_seconds(record.get("timestamp"))
        if at is None:
            continue
        kind = record.get("type")
        if kind == "user" and not record.get("isMeta"):
            text = _prompt_text(record)
            if text is None:
                last = at
                continue
            if current is not None and last is not None:
                # The harness never closed that turn (it was killed): it ended at its last record.
                current.end = last
                current.reply = last_reply
                turns.append(current)
            trigger = "self" if text.startswith(_TASK_NOTIFICATION) else ("parent" if turns else "first")
            current = ChildTurn(at, trigger)
            last = at
        elif kind == "assistant":
            last = at
            message = record.get("message")
            content = message.get("content") if isinstance(message, dict) else None
            for block in content if isinstance(content, list) else []:
                if isinstance(block, dict) and block.get("type") == "text":
                    text = _string(block.get("text")) or ""
                    if text.strip():
                        last_reply = text
            if current is None:
                current = ChildTurn(at, "self")
        elif kind == "system" and record.get("subtype") == _TURN_DURATION and current is not None:
            current.end = at
            current.reply = last_reply
            turns.append(current)
            current = None
    if current is not None:
        turns.append(current)
    return turns


def muse_turns(path: Path) -> list[ChildTurn]:
    """Turns of a Muse session log: a ``started`` run event to its ``terminal`` event.

    A turn started with an empty prompt was started by the agent's own background work.
    """

    turns: list[ChildTurn] = []
    current: ChildTurn | None = None
    last_reply = ""
    for record in _json_lines(path):
        payload = record.get("payload")
        recorded = _number(record.get("recorded_at"))
        if not isinstance(payload, dict) or payload.get("kind") != "run" or recorded is None:
            continue
        event = payload.get("event")
        if not isinstance(event, dict):
            continue
        at = recorded / 1e6
        kind = event.get("kind")
        if kind == "started":
            prompt = _string(event.get("prompt")) or ""
            current = ChildTurn(at, "first" if not turns else ("parent" if prompt else "self"))
        elif kind == "assistant_message_committed":
            last_reply = _string(event.get("text")) or ""
        elif kind == "terminal" and current is not None:
            current.end = at
            current.reply = last_reply
            turns.append(current)
            current = None
    if current is not None:
        turns.append(current)
    return turns


def muse_session_cwd(path: Path) -> tuple[float, str] | None:
    """When a Muse session started and its working directory, from its ``route_facts`` record.

    The session log is the record of truth: Muse's session index forgets the workspace of a
    session whose directory has since gone away, which is every finished slot.
    """

    for count, record in enumerate(_json_lines(path)):
        if count > MUSE_HEADER_RECORDS:
            return None
        payload = record.get("payload")
        recorded = _number(record.get("recorded_at"))
        if isinstance(payload, dict) and payload.get("kind") == "route_facts" and recorded is not None:
            facts = payload.get("record")
            cwd = _string(facts.get("cwd")) if isinstance(facts, dict) else None
            return (recorded / 1e6, cwd.rstrip("/")) if cwd else None
    return None


def muse_logs(sessions: Path) -> dict[str, list[tuple[float, Path]]]:
    """Muse session logs under ``sessions`` by working directory, with their start times."""

    found: dict[str, list[tuple[float, Path]]] = {}
    for path in sorted(sessions.glob("*/*/*/*/session.jsonl")):
        header = muse_session_cwd(path)
        if header is not None:
            found.setdefault(header[1], []).append((header[0], path))
    return found


def verdict(reply: str) -> str:
    """What a child's last reply asks of its parent: DONE, BLOCKED, ask, await, or progress."""

    lines = [line.strip() for line in reply.splitlines() if line.strip()]
    for marker in ("DONE:", "BLOCKED:"):
        if any(line.startswith(marker) for line in lines[-30:]):
            return marker[:-1]
    lowered = reply.lower()
    if _ASK.search(lowered):
        return "ask"
    if _AWAIT.search(lowered):
        return "await"
    return "progress"


# ------------------------------------------------------------------------------ analysis


def _locate_logs(agents: list[AgentRecord], sources: Sources) -> None:
    """Find each child's log. A Muse log goes to at most one child: the first registered in its
    working directory within ``MUSE_START_WINDOW`` seconds before the log started."""

    muse = muse_logs(sources.muse_sessions) if sources.muse_sessions is not None else {}
    claimed: set[Path] = set()
    for agent in sorted(agents, key=lambda a: a.created):
        if agent.harness == "claude" and agent.native_session and sources.claude_projects is not None:
            hits = sorted(sources.claude_projects.glob(f"*/{agent.native_session}.jsonl"))
            agent.log = hits[0] if hits else None
        elif agent.harness == "muse" and agent.cwd:
            candidates = sorted(entry for entry in muse.get(agent.cwd.rstrip("/"), [])
                                if agent.created - 120 <= entry[0] <= agent.created + MUSE_START_WINDOW
                                and entry[1] not in claimed)
            agent.log = candidates[0][1] if candidates else None
            if agent.log is not None:
                claimed.add(agent.log)


def analyze(sources: Sources, since: float | None = None, until: float | None = None) -> Report:
    """Rebuild every child's idle stretches and the parent's response to each turn end."""

    if not sources.coordinator_transcript.is_file():
        raise ResponsivenessError(f"coordinator transcript not found: {sources.coordinator_transcript}")
    if not sources.registry.is_dir():
        raise ResponsivenessError(f"agentctl registry not found: {sources.registry}")
    agents = [agent for agent in load_agents(sources.registry, sources.coordinator)
              if not any(fnmatchcase(agent.name, pattern) for pattern in sources.exclude)]
    names = [agent.name for agent in agents]
    slots: dict[str, list[AgentRecord]] = {}
    for agent in agents:
        if agent.slot:
            slots.setdefault(agent.slot, []).append(agent)
    turns, inputs, actions = load_coordinator(
        sources.coordinator_transcript, names, slots, sources.sweep_marker)
    if not turns:
        raise ResponsivenessError("the coordinator transcript holds no completed turn")
    window_start = since if since is not None else turns[0][0]
    window_end = until if until is not None else turns[-1][1]
    messages = load_messages(sources.registry)
    by_agent: dict[str, list[Action]] = {name: [] for name in names}
    for action in actions:
        by_agent.setdefault(action.agent, []).append(action)
    for message in messages:
        if message.owner in by_agent and message.queued is not None:
            by_agent[message.owner].append(Action(message.queued, "send", message.owner))
    for agent_actions in by_agent.values():
        agent_actions.sort(key=lambda action: action.at)
    for agent in agents:
        agent.stopped = next((action.at for action in by_agent[agent.name]
                              if action.verb == "stop" and action.at > agent.created), None)
    _locate_logs(agents, sources)

    starts = [start for start, _ in turns]

    def busy(at: float) -> bool:
        index = bisect.bisect_right(starts, at) - 1
        return index >= 0 and turns[index][0] <= at <= turns[index][1]

    coordinator_queue = [message for message in messages if message.owner == sources.coordinator]
    child_messages = [(message, message_author(message.text, names)) for message in coordinator_queue
                      if not message.message_id.startswith((WATCH_ID_PREFIX, CHAT_ID_PREFIX))]
    watch = [(message, watch_agents(message.text)) for message in coordinator_queue
             if message.message_id.startswith(WATCH_ID_PREFIX)]
    child_queues: dict[str, list[QueueMessage]] = {}
    for message in messages:
        if message.owner != sources.coordinator:
            child_queues.setdefault(message.owner, []).append(message)
    sweeps = sorted(entry.at for entry in inputs if entry.kind == "sweep")
    chats = sorted(entry.at for entry in inputs if entry.kind == "chat")

    episodes: list[Episode] = []
    gaps: list[Gap] = []
    for agent in agents:
        if agent.log is None or not agent.log.is_file():
            continue
        child = claude_turns(agent.log) if agent.harness == "claude" else muse_turns(agent.log)
        agent_actions = by_agent[agent.name]
        for index, turn in enumerate(child):
            if turn.end is None or not window_start <= turn.end <= window_end:
                continue
            if agent.stopped is not None and turn.end > agent.stopped:
                continue
            following = child[index + 1] if index + 1 < len(child) else None
            resumed = following.start if following else None
            trigger = following.trigger if following else ("stop" if agent.stopped else "open")
            kind = verdict(turn.reply)
            gap_end = resumed if resumed is not None else agent.stopped
            if gap_end is not None:
                waiting = {"self": "self", "stop": "stop"}.get(trigger, "parent")
                gaps.append(Gap(agent.name, agent.harness, turn.end, min(gap_end, window_end), waiting, kind))
            if trigger == "self":
                continue
            horizon = gap_end if gap_end is not None else turn.end + OPEN_HORIZON_SECONDS
            looked = [action for action in agent_actions if turn.end < action.at <= horizon + 5]
            observed = looked[0].at if looked else None
            acting = next((action for action in looked if action.verb in ACTING_VERBS + ("remove",)), None)
            acted = acting.at if acting else None
            act_verb = acting.verb if acting else None
            own = [message for message, author in child_messages if author == agent.name
                   and message.queued is not None
                   and max(turn.start - 1, turn.end - CHILD_MESSAGE_LEAD_SECONDS) <= message.queued <= horizon]
            notices = [message for message, listed in watch if agent.name in listed
                       and message.queued is not None and turn.end < message.queued <= horizon]
            delivered: list[tuple[float, str]] = []
            for message, label, input_kind in [(m, "child-message", "child-message") for m in own] + [
                    (m, "watch-notice", "watch") for m in notices]:
                # agentctl leaves confirmed_at unset when it cannot verify a submission that did
                # arrive, so the prompt's arrival in the coordinator's transcript also counts.
                arrival = message.confirmed
                if arrival is None and message.queued is not None:
                    queued_at = message.queued
                    arrival = next((entry.at for entry in inputs if entry.kind == input_kind
                                    and entry.agent == agent.name and queued_at - 1 <= entry.at <= horizon), None)
                if arrival is not None:
                    delivered.append((arrival, label))
            delivered.extend((at, "sweep") for at in sweeps if turn.end < at <= horizon)
            sweep = next((at for at in sweeps if turn.end < at <= horizon), None)
            chat = next((at for at in chats if turn.end < at <= horizon), None)
            first_notice = min(delivered)[1] if delivered else None
            reference = observed if observed is not None else acted
            path = "none"
            if reference is not None:
                before = [(at, label) for at, label in delivered if at <= reference + 1]
                if before:
                    path = max(before)[1]
                elif chat is not None and chat <= reference:
                    path = "chat-turn"
                else:
                    path = "coordinator-own"
            elif trigger == "parent" and resumed is not None and resumed - turn.end <= PRE_QUEUED_SECONDS:
                resumed_at = resumed
                waiting_message = any(
                    m.queued is not None and m.queued <= turn.end + 1
                    and (m.confirmed is None or abs(m.confirmed - resumed_at) <= PRE_QUEUED_SECONDS)
                    for m in child_queues.get(agent.name, []))
                if waiting_message:
                    path, observed, acted, act_verb = "pre-queued", resumed, resumed, "send"
            queued = [m.queued for m in own + notices if m.queued is not None]
            notified = min(queued + ([sweep] if sweep is not None else []), default=None)
            episodes.append(Episode(
                agent=agent.name, harness=agent.harness, turn_end=turn.end, verdict=kind,
                next_trigger=trigger, resumed=resumed, path=path, first_notice=first_notice,
                parent_busy=busy(turn.end), notified=notified, observed=observed, acted=acted,
                act_verb=act_verb, reply=turn.reply[:240].replace("\n", " "),
            ))
    episodes.sort(key=lambda episode: episode.turn_end)
    gaps.sort(key=lambda gap: gap.start)
    return Report(sources.coordinator, window_start, window_end, turns, inputs, agents, episodes, gaps, messages)


# ------------------------------------------------------------------------------ reporting


def percentile(values: Sequence[float], fraction: float) -> float | None:
    """The sample at the rounded index ``fraction * (n - 1)``; ``None`` for no values."""

    if not values:
        return None
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, max(0, round(fraction * (len(ordered) - 1))))]


def distribution(values: Iterable[float | None]) -> dict[str, float | int | None]:
    """Count, p50, p90 and max of a latency sample, in minutes."""

    present = [value for value in values if value is not None]

    def minutes(value: float | None) -> float | None:
        return None if value is None else round(value / 60, 1)

    return {
        "n": len(present),
        "p50_min": minutes(percentile(present, 0.5)),
        "p90_min": minutes(percentile(present, 0.9)),
        "max_min": minutes(max(present) if present else None),
    }


def _split(episodes: Sequence[Episode], key: str) -> dict[str, dict[str, float | int | None]]:
    groups: dict[str, list[Episode]] = {}
    for episode in episodes:
        value = getattr(episode, key)
        groups.setdefault(str(value).lower() if isinstance(value, bool) else str(value), []).append(episode)
    return {name: distribution(e.latency(e.acted) for e in group) for name, group in sorted(groups.items())}


def summary(report: Report) -> dict[str, object]:
    """The report's figures as one JSON-ready object."""

    episodes = report.episodes
    idle: dict[str, float] = {"parent": 0.0, "self": 0.0, "stop": 0.0}
    for gap in report.gaps:
        idle[gap.waiting_for] += gap.end - gap.start
    span = report.window_end - report.window_start
    busy = sum(min(end, report.window_end) - max(start, report.window_start)
               for start, end in report.coordinator_turns
               if end > report.window_start and start < report.window_end)
    starts = [start for start, _ in report.coordinator_turns]
    wake: list[float] = []
    for episode in episodes:
        if not episode.parent_busy:
            index = bisect.bisect_right(starts, episode.turn_end)
            if index < len(starts):
                wake.append(starts[index] - episode.turn_end)
    delivered = [_Delivery(m.message_id, m.queued, m.confirmed) for m in report.messages
                 if m.owner == report.coordinator and m.queued is not None and m.confirmed is not None]
    return {
        "window": [_iso(report.window_start), _iso(report.window_end)],
        "children_with_logs": sum(1 for agent in report.agents if agent.log is not None),
        "episodes": len(episodes),
        "coordinator": {
            "turns": len(report.coordinator_turns),
            "busy_fraction": round(busy / span, 3) if span > 0 else None,
            "inputs": dict(sorted(Counter(
                entry.kind + ("/mid-turn" if entry.mid_turn else "") for entry in report.inputs
                if report.window_start <= entry.at <= report.window_end).items())),
            "next_turn_after_child_end_when_idle": distribution(wake),
        },
        "act_latency": distribution(e.latency(e.acted) for e in episodes),
        "observe_latency": distribution(e.latency(e.observed) for e in episodes),
        "notify_latency": distribution(
            None if e.notified is None else max(0.0, e.notified - e.turn_end) for e in episodes),
        "act_latency_by_path": _split(episodes, "path"),
        "act_latency_by_first_notice": _split(episodes, "first_notice"),
        "act_latency_by_harness": _split(episodes, "harness"),
        "act_latency_by_verdict": _split(episodes, "verdict"),
        "act_latency_by_parent_busy": _split(episodes, "parent_busy"),
        "queue_delivery_to_coordinator": {
            "from_children": distribution(m.confirmed - m.queued for m in delivered
                                          if not m.message_id.startswith((WATCH_ID_PREFIX, CHAT_ID_PREFIX))),
            "watch_notices": distribution(m.confirmed - m.queued for m in delivered
                                          if m.message_id.startswith(WATCH_ID_PREFIX)),
            "chat": distribution(m.confirmed - m.queued for m in delivered
                                 if m.message_id.startswith(CHAT_ID_PREFIX)),
        },
        "child_idle_hours": {key: round(value / 3600, 1) for key, value in sorted(idle.items())},
        "child_idle_waiting_on_parent_fraction": (
            round((idle["parent"] + idle["stop"]) / sum(idle.values()), 3) if sum(idle.values()) > 0 else None),
        "parent_gap": distribution(gap.end - gap.start for gap in report.gaps if gap.waiting_for == "parent"),
        "done_until_stop": distribution(gap.end - gap.start for gap in report.gaps
                                        if gap.waiting_for == "stop" and gap.verdict == "DONE"),
    }


@dataclass(frozen=True)
class _Delivery:
    message_id: str
    queued: float
    confirmed: float


def _iso(at: float) -> str:
    return datetime.fromtimestamp(at, timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def episode_rows(report: Report) -> list[dict[str, object]]:
    """One JSON-ready row per episode."""

    def minutes(value: float | None) -> float | None:
        return None if value is None else round(value / 60, 1)

    return [{
        "agent": e.agent, "harness": e.harness, "turn_end": _iso(e.turn_end), "verdict": e.verdict,
        "next": e.next_trigger, "path": e.path, "first_notice": e.first_notice, "parent_busy": e.parent_busy,
        "notify_min": minutes(None if e.notified is None else max(0.0, e.notified - e.turn_end)),
        "observe_min": minutes(e.latency(e.observed)), "act_min": minutes(e.latency(e.acted)),
        "act_verb": e.act_verb, "reply": e.reply,
    } for e in report.episodes]


def format_markdown(report: Report) -> str:
    """A short human-readable report."""

    figures = summary(report)
    lines = [
        "# Parent-child responsiveness",
        "",
        f"Window {figures['window'][0]} to {figures['window'][1]}; "  # type: ignore[index]
        f"{figures['children_with_logs']} children with logs; {figures['episodes']} episodes.",
        "",
        "| Split | Group | n | p50 min | p90 min | max min |",
        "|---|---|---:|---:|---:|---:|",
    ]
    for split in ("act_latency_by_path", "act_latency_by_first_notice", "act_latency_by_harness",
                  "act_latency_by_verdict", "act_latency_by_parent_busy"):
        groups = figures[split]
        assert isinstance(groups, dict)
        for group, stats in groups.items():
            assert isinstance(stats, dict)
            lines.append(f"| {split.removeprefix('act_latency_by_')} | {group} | {stats['n']} | "
                         f"{stats['p50_min']} | {stats['p90_min']} | {stats['max_min']} |")
    lines += ["", "```json", json.dumps({key: figures[key] for key in (
        "coordinator", "act_latency", "notify_latency", "queue_delivery_to_coordinator", "child_idle_hours",
        "child_idle_waiting_on_parent_fraction", "parent_gap", "done_until_stop")}, indent=1), "```", ""]
    return "\n".join(lines)


def run(sources: Sources, since: float | None, until: float | None, output_format: str,
        episodes_file: Path | None) -> str:
    """Analyze, optionally write per-episode rows, and return the formatted report."""

    if output_format not in ("json", "markdown"):
        raise ResponsivenessError(f"unsupported format {output_format!r}")
    report = analyze(sources, since, until)
    if episodes_file is not None:
        episodes_file.write_text(json.dumps(episode_rows(report), indent=1) + "\n", encoding="utf-8")
    if output_format == "json":
        return json.dumps(summary(report), indent=1) + "\n"
    return format_markdown(report)
