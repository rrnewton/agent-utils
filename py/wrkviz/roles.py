"""Lane-session role classification, measured from records a team already writes.

Every persistent agent lane does one of a few jobs -- it coordinates other lanes, it makes a
change, or it judges a change -- but nothing in the agent manager's registry says which. This
module rebuilds the answer, without calling a model, from three record families:

* the registry: who created whom, who is the designated coordinator, which directory a lane
  ran in, and the queued briefs that tasked it;
* the lane's own log, when one is found: how its tool use splits between editing, committing,
  reading, managing other lanes and relaying chat;
* an optional overrides file: roles the owner has already declared, which always win.

The unit classified is one lane-session -- one registry record, with its token and its native
session -- never a native subagent inside a lane's own harness session, and never an agent
name's whole lifetime. A lane that the evidence cannot place is reported ``unclassified``
rather than guessed at. Reads only; calls no model; writes nothing but the report.

This is a disposable sidecar: no archive field, no timeline field and no second
implementation carry the label yet. Confidence is carried in integer basis points so a later
implementation can reproduce the number exactly. Version 1 is library-only: there is no
``wrkviz roles`` subcommand; callers use this module's API (``classify_fleet``,
``classify_sources`` and ``run``) directly.
"""

from __future__ import annotations

import json
import re
from collections import Counter
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass, field, replace
from pathlib import Path

from wrkviz.discover import launches as _log_launches
from wrkviz.responsiveness import muse_logs, verdict as _reply_verdict

#: Roles a lane can be given. ``unclassified`` is an outcome, not a role: the evidence was
#: missing, contradictory or too thin to act on.
ROLES = ("coordinator", "implementor", "reviewer", "unclassified")
#: Coordinator kinds. A liaison faces chat and holds no implementor or reviewer reports; a
#: lieutenant coordinates lanes of its own but reports to another coordinator; a lead reports
#: to no coordinator.
COORDINATOR_KINDS = ("lead", "liaison", "lieutenant")

# Scoring bands. Topology -- who launched whom -- is the most trustworthy evidence a lane
# leaves, observed behaviour and the brief that tasked it come next, and a lane's name or
# launch profile may only corroborate: prefixes record which harness a lane runs, and
# profiles record model policy, so neither may carry a label on its own.
_TOPOLOGY = 3
_BEHAVIOUR = 2
_BRIEF = 2
_NAME = 1
#: A winner below this score, or too close to its runner-up, abstains.
_MIN_TOP_SCORE = 5.0
_MIN_MARGIN = 2.0
_MIN_CONFIDENCE = 0.55
#: The score at which the evidence mass stops discounting confidence.
_FULL_MASS = 9.0
#: Log lines scanned for one lane's tool mix. The mix is a shape, not a census, so a lane's
#: first stretch of log is enough to read it and the cap bounds the cost on long coordinators.
TOOL_MIX_LINE_CAP = 40_000
#: Characters of each queued brief scanned for role wording.
BRIEF_SCAN_CHARS = 8_000


class RolesError(ValueError):
    """A source is missing or malformed in a way that makes the report meaningless."""


@dataclass(frozen=True)
class ToolMix:
    """How one lane's logged tool use splits, as matched-line counts per kind."""

    lines: int = 0
    edit: int = 0
    commit: int = 0
    read_search: int = 0
    test_build: int = 0
    lane_management: int = 0
    lane_starts: int = 0
    chat: int = 0
    review_verdict: int = 0

    @property
    def total(self) -> int:
        """Logged lines that matched any tool kind; the denominator of every share."""

        return (
            self.edit + self.commit + self.read_search + self.test_build
            + self.lane_management + self.chat + self.review_verdict
        )

    def share(self, count: int) -> float:
        """One count as a fraction of ``total``; zero when nothing matched."""

        return count / self.total if self.total else 0.0


@dataclass(frozen=True)
class LaneSession:
    """One registry record with everything known about the lane-session it started."""

    registry: str
    name: str
    token: str | None = None
    harness: str = "unknown"
    profile: str | None = None
    cwd: str | None = None
    created_at: float = 0.0
    lifecycle: str | None = None
    native_session: str | None = None
    parent_name: str | None = None
    parent_registry: str | None = None
    designated_coordinator: bool = False
    in_restore: bool = False
    brief: str | None = None
    incoming: tuple[str, ...] = ()
    last_reply: str | None = None
    tool_mix: ToolMix = ToolMix()
    has_log: bool = False

    @property
    def key(self) -> str:
        """``registry:name@token``, unique per record generation within one report."""

        return f"{self.registry}:{self.name}@{self.token or 'current'}"

    @property
    def in_slot(self) -> bool:
        """Whether the lane ran in a delegated worktree slot, judged by its directory."""

        return self.cwd is not None and "/worktrees/slots/" in self.cwd.replace("\\", "/")


@dataclass(frozen=True)
class Signal:
    """One piece of evidence: what was observed, which label it supports, and how strongly."""

    kind: str
    detail: str
    label: str
    weight: float


@dataclass(frozen=True)
class Classification:
    """The verdict for one lane-session, with the evidence that produced it."""

    registry: str
    name: str
    token: str | None
    native_session: str | None
    harness: str
    role: str
    coordinator_kind: str | None
    confidence_bp: int
    signals: tuple[Signal, ...] = ()
    alternatives: tuple[tuple[str, float], ...] = ()

    @property
    def confidence(self) -> float:
        """Confidence as a fraction, converted from the stored basis points."""

        return self.confidence_bp / 10_000

    @property
    def key(self) -> str:
        """``registry:name@token``, unique per record generation within one report."""

        return f"{self.registry}:{self.name}@{self.token or 'current'}"

    def to_json_obj(self) -> dict[str, object]:
        """This verdict as plain values: identity, role, confidence, signals and alternatives."""

        return {
            "key": self.key,
            "registry": self.registry,
            "name": self.name,
            "token": self.token,
            "native_session": self.native_session,
            "harness": self.harness,
            "role": self.role,
            "coordinator_kind": self.coordinator_kind,
            "confidence_bp": self.confidence_bp,
            "signals": [
                {"kind": s.kind, "detail": s.detail, "label": s.label, "weight": s.weight}
                for s in self.signals
            ],
            "alternatives": [
                {"role": role, "score": score} for role, score in self.alternatives
            ],
        }


@dataclass(frozen=True)
class Sources:
    """Where the fleets to classify live, per registry label."""

    registries: Sequence[tuple[str, Path]]
    claude_projects: Sequence[tuple[str, Path]] = ()
    muse_sessions: Sequence[tuple[str, Path]] = ()
    overrides: Mapping[str, tuple[str, str | None]] = field(default_factory=dict)


# --------------------------------------------------------------------------- text patterns

_HEADER = re.compile(r"^\[[^\]]{0,300}\]\s*")
_SENDER = re.compile(r"\[from\s+([^\s\]]+)(?:\s+via\s+([^\s\]]+))?", re.IGNORECASE)
_CHAT_INGRESS = re.compile(
    r"<chat_message>|via\s+(?:slack|chat|gchat)\b|\bchat bridge\b|CHAT_REPLY_",
    re.IGNORECASE,
)
_REVIEW_WORD = re.compile(r"\breview\b", re.IGNORECASE)
_REVIEW_TARGET = re.compile(
    r"\bPR\s*#?\d+|\bpull request\b|/pull/\d+|\bD\d{5,}\b|\bdiff\b|\bcommit\s+[0-9a-f]{7,}\b"
    r"|\bbranch\b|\bhead\b",
    re.IGNORECASE,
)
_VERDICT_ASK = re.compile(
    r"\bverdict\b|\bapprove\b|request changes|findings only|\bblocking\b|\bLGTM\b",
    re.IGNORECASE,
)
_READ_ONLY = re.compile(r"read-only|do not edit|no code edits|recon only", re.IGNORECASE)
_AUDIT = re.compile(r"\baudit\b", re.IGNORECASE)
_IMPLEMENT = re.compile(
    r"\bimplement\b|\bfix\s+#|\bfix the\b|\bland\b|your slot|wrkslots adopt"
    r"|commit only in|write-handoff|make validate",
    re.IGNORECASE,
)
_MANAGED_VERDICT = re.compile(r"end (?:the |your )?final message with DONE:|DONE:/BLOCKED:",
                              re.IGNORECASE)
_ROLE_LINE = re.compile(r"^\s*Role:\s*`?([A-Za-z][\w -]{0,60})", re.IGNORECASE | re.MULTILINE)
_REVIEWER_NAME = re.compile(r"(^|[-_])rev(iew)?([-_]|$)|review", re.IGNORECASE)
# Boilerplate that rides in nearly every brief and names coordination without tasking it.
_BOILERPLATE = (
    "message agents only via agentctl send",
    "message agents through agentctl",
    "reply: agentctl send",
)


def _brief_body(text: str) -> str:
    """A queued brief without its sender header and standing boilerplate."""

    body = _HEADER.sub("", text.strip(), count=1)
    lowered = body.lower()
    for phrase in _BOILERPLATE:
        while phrase in lowered:
            at = lowered.index(phrase)
            body = body[:at] + body[at + len(phrase):]
            lowered = body.lower()
    return body[:BRIEF_SCAN_CHARS]


def _senders(texts: Iterable[str]) -> set[str]:
    found: set[str] = set()
    for text in texts:
        match = _SENDER.search(text)
        if match:
            found.add(match.group(1).lower())
    return found


def _chat_fraction(texts: Sequence[str]) -> float:
    if not texts:
        return 0.0
    return sum(1 for text in texts if _CHAT_INGRESS.search(text)) / len(texts)


def _reviewer_named(name: str) -> bool:
    lowered = name.lower()
    if "preview" in lowered:
        return False
    return bool(_REVIEWER_NAME.search(lowered)) or "review" in lowered


# ------------------------------------------------------------------------------- scoring


@dataclass(frozen=True)
class _Context:
    """Fleet facts about one lane that no single record holds."""

    child_names: frozenset[str] = frozenset()
    parent_is_coordinator: bool = False
    child_roles: frozenset[str] = frozenset()


def _signals(lane: LaneSession, context: _Context) -> list[Signal]:
    found: list[Signal] = []

    def add(kind: str, detail: str, label: str, weight: float) -> None:
        found.append(Signal(kind, detail, label, weight))

    # Topology: who launched whom, and where the lane ran.
    children = len(context.child_names)
    if children:
        add("topology-children", f"created or manages {children} lane(s)",
            "coordinator", float(_TOPOLOGY))
        if children >= 4:
            add("topology-children", "manages four or more lanes", "coordinator", 2.0)
    if context.parent_is_coordinator and children:
        add("topology-subcoordinator", "reports to a coordinator and holds lanes",
            "coordinator", 2.0)
    if lane.in_slot:
        add("topology-slot", "ran in a delegated worktree slot", "implementor", 2.0)
        add("topology-slot", "ran in a delegated worktree slot", "reviewer", 1.0)
    elif lane.cwd and lane.in_restore:
        add("topology-standing", "standing harness-root lane in the restore set",
            "coordinator", 1.0)

    # The brief that tasked the lane, and the shape of everything queued to it.
    brief_review = False
    if lane.brief:
        body = _brief_body(lane.brief)
        has_review = bool(_REVIEW_WORD.search(body))
        has_target = bool(_REVIEW_TARGET.search(body))
        brief_review = has_review and has_target
        if brief_review:
            add("brief-review", "brief asks for review of a named change",
                "reviewer", 2.0 * _BRIEF)
        elif has_review:
            add("brief-review", "brief mentions review without a named change",
                "reviewer", 1.0)
        if _VERDICT_ASK.search(body):
            add("brief-verdict-ask", "brief asks for a review verdict",
                "reviewer", float(_BRIEF))
        if _READ_ONLY.search(body):
            add("brief-read-only", "brief forbids edits", "reviewer",
                float(_BRIEF) if has_target else 1.0)
        elif _AUDIT.search(body) and not brief_review:
            add("brief-audit", "brief asks for an audit, not a change review",
                "reviewer", 1.0)
        implement_hits = len(_IMPLEMENT.findall(body))
        if implement_hits:
            add("brief-implement", "brief tasks an implementation in a slot",
                "implementor", float(min(2 * _BRIEF, _BRIEF * implement_hits)))
        if _MANAGED_VERDICT.search(body):
            add("brief-managed", "brief requires a DONE:/BLOCKED: verdict",
                "implementor", 1.0)
            add("brief-managed", "brief requires a DONE:/BLOCKED: verdict",
                "reviewer", 1.0)
        role_line = _ROLE_LINE.search(body)
        if role_line:
            declared = role_line.group(1).lower()
            for needle, label in (("review", "reviewer"), ("liaison", "liaison"),
                                  ("coordinat", "coordinator"), ("implement", "implementor")):
                if needle in declared:
                    add("brief-role-line", f"brief opens Role: {role_line.group(1).strip()}",
                        label, float(_BRIEF))
                    break
        if _CHAT_INGRESS.search(body):
            add("brief-chat", "brief is chat ingress", "liaison", float(_BRIEF))
    if lane.incoming:
        if _chat_fraction(lane.incoming) > 0.5:
            add("incoming-chat", "most queued input arrives via chat",
                "liaison", 2.0 * _BRIEF)
        if len(_senders(lane.incoming)) >= 4:
            add("incoming-senders", "queued input comes from four or more senders",
                "coordinator", float(_BRIEF))
    if lane.last_reply and _reply_verdict(lane.last_reply) in ("DONE", "BLOCKED"):
        add("reply-verdict", "last reply ends in a managed verdict", "implementor", 1.0)
        add("reply-verdict", "last reply ends in a managed verdict", "reviewer", 1.0)

    # Observed behaviour in the lane's own log.
    mix = lane.tool_mix
    if mix.total:
        if mix.share(mix.edit) >= 0.25:
            add("behaviour-edits", "a quarter or more of tool use edits files",
                "implementor", 2.0 * _BEHAVIOUR)
        elif mix.share(mix.edit) >= 0.08:
            add("behaviour-edits", "tool use includes file edits",
                "implementor", float(_BEHAVIOUR))
        if mix.commit:
            add("behaviour-commits", "authored at least one commit",
                "implementor", float(_BEHAVIOUR))
        if mix.edit and mix.share(mix.test_build) >= 0.1:
            add("behaviour-tests", "runs tests or builds alongside edits",
                "implementor", float(_BEHAVIOUR))
        if mix.edit == 0 and mix.share(mix.read_search) >= 0.4:
            # Reading without editing is also how research looks; it only reads as review
            # when the brief named a change to judge.
            add("behaviour-reads", "reads and searches without editing", "reviewer",
                2.0 * _BEHAVIOUR if brief_review else 1.0)
        if mix.review_verdict:
            add("behaviour-review-verdict", "posts review verdicts",
                "reviewer", float(_BEHAVIOUR))
        if mix.lane_starts >= 2 or mix.share(mix.lane_management) >= 0.15:
            add("behaviour-lane-management", "starts, stops or tasks other lanes",
                "coordinator", 2.0 * _BEHAVIOUR)
        elif mix.lane_management:
            add("behaviour-lane-management", "manages other lanes at least once",
                "coordinator", float(_BEHAVIOUR))
        if mix.share(mix.chat) >= 0.25:
            add("behaviour-chat", "a quarter or more of tool use relays chat",
                "liaison", 2.0 * _BEHAVIOUR)

    # Name and launch profile: corroboration only, never a label on their own.
    lowered = lane.name.lower()
    if _reviewer_named(lowered):
        add("name", "name carries a review token", "reviewer", float(_NAME))
    if "coord" in lowered:
        add("name", "name carries a coordinator token", "coordinator", float(_NAME))
    if lowered == "liaison" or re.search(r"(^|[-_])liaison([-_]|$)", lowered):
        add("name", "named as a liaison", "liaison", float(_NAME))
    if lowered.startswith(("mu-", "cl-", "cx-")):
        add("name", "harness-prefixed lane name", "implementor", float(_NAME))
    profile = (lane.profile or "").lower()
    if profile == "coordinator" or profile.endswith("-coordinator"):
        add("profile", "launched with a coordinator profile", "coordinator", float(_NAME))
    elif "reviewer" in profile:
        add("profile", "launched with a reviewer profile", "reviewer", float(_NAME))
    elif profile == "worker" or profile.endswith("-worker"):
        add("profile", "launched with a worker profile", "implementor", float(_NAME))
    return found


def _declared(
    lane: LaneSession, overrides: Mapping[str, tuple[str, str | None]]
) -> tuple[str, str | None] | None:
    """An owner's declared role for this lane, by session, then fleet key, then name."""

    for key in (
        lane.native_session or "",
        f"{lane.registry}:{lane.name}",
        lane.name,
    ):
        if key and key in overrides:
            return overrides[key]
    return None


def _verdict(
    lane: LaneSession,
    role: str,
    kind: str | None,
    confidence_bp: int,
    signals: tuple[Signal, ...] = (),
    alternatives: tuple[tuple[str, float], ...] = (),
) -> Classification:
    return Classification(
        registry=lane.registry, name=lane.name, token=lane.token,
        native_session=lane.native_session, harness=lane.harness,
        role=role, coordinator_kind=kind, confidence_bp=confidence_bp,
        signals=signals, alternatives=alternatives,
    )


def _classify(
    lane: LaneSession,
    context: _Context,
    overrides: Mapping[str, tuple[str, str | None]],
) -> Classification:
    declared = _declared(lane, overrides)
    if declared is not None:
        role, kind = declared
        return _verdict(lane, role, kind, 10_000, signals=(
            Signal("declared", "role declared in the overrides file", role, 0.0),))
    if lane.designated_coordinator:
        kind = "lieutenant" if context.parent_is_coordinator else "lead"
        return _verdict(lane, "coordinator", kind, 9_800, signals=(
            Signal("designated-coordinator",
                   "named as the registry's designated coordinator",
                   "coordinator", 0.0),))
    if (
        lane.lifecycle == "launch_failed"
        and not lane.has_log and not lane.brief and not lane.incoming
    ):
        return _verdict(lane, "unclassified", None, 0, signals=(
            Signal("no-session", "launch failed and no session ever ran",
                   "unclassified", 0.0),))

    signals = _signals(lane, context)
    scores: dict[str, float] = {}
    for signal in signals:
        scores[signal.label] = scores.get(signal.label, 0.0) + signal.weight
    notes: list[Signal] = []
    working_reports = context.child_roles & {"implementor", "reviewer"}
    if scores.get("liaison") and (working_reports or lane.in_slot):
        # A liaison holds no implementor or reviewer reports and does no delegated slot
        # work; either fact retires the label whatever the chat evidence says.
        reason = ("holds implementor or reviewer reports" if working_reports
                  else "ran in a delegated slot")
        notes.append(Signal("liaison-blocked", f"liaison retired: lane {reason}",
                            "liaison", -scores["liaison"]))
        scores["liaison"] = 0.0
    ranked = sorted(
        ((label, score) for label, score in scores.items() if score > 0),
        key=lambda item: (-item[1], item[0]),
    )
    if not ranked:
        return _verdict(lane, "unclassified", None, 0,
                        signals=tuple([*signals, *notes]))
    top_label, top = ranked[0]
    second = ranked[1][1] if len(ranked) > 1 else 0.0
    margin_share = top / (top + second) if second else 1.0
    mass = min(1.0, top / _FULL_MASS)
    confidence = margin_share * (0.35 + 0.65 * mass)
    if top_label == "liaison" and context.child_names:
        # Reports exist but none classified as working lanes: the liaison definition is
        # partly negative, so unplaced reports weaken it instead of retiring it.
        notes.append(Signal("liaison-degraded",
                            "lane holds reports of unknown role", "liaison", 0.0))
        confidence *= 0.7
    emitted = tuple([*signals, *notes])
    if top < _MIN_TOP_SCORE or top - second < _MIN_MARGIN or confidence < _MIN_CONFIDENCE:
        return _verdict(lane, "unclassified", None, round(confidence * 10_000),
                        signals=emitted, alternatives=tuple(ranked))
    if top_label == "liaison":
        role, kind = "coordinator", "liaison"
    elif top_label == "coordinator":
        role = "coordinator"
        kind = "lieutenant" if context.parent_is_coordinator else "lead"
    else:
        role, kind = top_label, None
    return _verdict(lane, role, kind, round(confidence * 10_000),
                    signals=emitted, alternatives=tuple(ranked[1:]))


def _contexts(
    lanes: Sequence[LaneSession],
    coordinator_names: frozenset[tuple[str, str]],
    prior: Mapping[str, Classification],
) -> dict[str, _Context]:
    """Per-lane fleet facts: distinct child lane names, and whether the parent coordinates."""

    def parent_lanes(lane: LaneSession) -> list[LaneSession]:
        if not lane.parent_name:
            return []
        same = [other for other in lanes
                if other.registry == lane.registry and other.name == lane.parent_name]
        if same:
            return same
        elsewhere = [other for other in lanes if other.name == lane.parent_name]
        by_registry = {other.registry for other in elsewhere}
        return elsewhere if len(by_registry) == 1 else []

    contexts: dict[str, _Context] = {}
    for lane in lanes:
        children = {
            other.name for other in lanes
            if other.registry == lane.registry and other.parent_name == lane.name
        }
        parents = parent_lanes(lane)
        parent_coordinates = bool(parents) and (
            any((p.registry, p.name) in coordinator_names for p in parents)
            or any(prior[p.key].role == "coordinator" for p in parents if p.key in prior)
        )
        child_roles = {
            prior[other.key].role for other in lanes
            if other.registry == lane.registry and other.parent_name == lane.name
            and other.key in prior
        }
        contexts[lane.key] = _Context(
            child_names=frozenset(children),
            parent_is_coordinator=parent_coordinates,
            child_roles=frozenset(child_roles),
        )
    return contexts


def classify_fleet(
    lanes: Sequence[LaneSession],
    overrides: Mapping[str, tuple[str, str | None]] | None = None,
) -> list[Classification]:
    """Classify every lane-session in a fleet, in input order.

    Two passes: the first knows only designated coordinators, the second also knows each
    lane's parent and children by their first-pass roles, which is what lieutenant (parent
    coordinates) and liaison (no working reports) are defined by.
    """

    declared = dict(overrides or {})
    designated = frozenset(
        (lane.registry, lane.name) for lane in lanes if lane.designated_coordinator
    )
    first: dict[str, Classification] = {}
    contexts = _contexts(lanes, designated, first)
    for lane in lanes:
        first[lane.key] = _classify(lane, contexts[lane.key], declared)
    contexts = _contexts(lanes, designated, first)
    return [_classify(lane, contexts[lane.key], declared) for lane in lanes]


# ------------------------------------------------------------------------------- reading


def _read_json(path: Path) -> Mapping[str, object] | None:
    try:
        record = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    return record if isinstance(record, dict) else None


def _string(raw: object) -> str | None:
    return raw if isinstance(raw, str) and raw else None


def _number(raw: object) -> float:
    return float(raw) if isinstance(raw, (int, float)) and not isinstance(raw, bool) else 0.0


def _creator(record: Mapping[str, object]) -> tuple[str | None, str | None]:
    """``(parent name, parent registry)`` from a record's creator fields, either shape."""

    creator = record.get("created_by") or record.get("managed_by")
    if isinstance(creator, dict):
        return _string(creator.get("agent")), _string(creator.get("registry"))
    return _string(creator), None


def _native_session(record: Mapping[str, object]) -> str | None:
    native = record.get("native_session")
    session = _string(native.get("value")) if isinstance(native, dict) else None
    if session:
        return session
    session = _string(record.get("session_value"))
    if session:
        return session
    arguments = record.get("arguments")
    if isinstance(arguments, list):
        for flag, value in zip(arguments, arguments[1:]):
            if flag in ("--session-id", "--resume"):
                found = _string(value)
                if found:
                    return found
    return None


def _record_dirs(registry: Path) -> list[Path]:
    """Every agent record directory, live in either layout, then the archive."""

    found: list[Path] = []
    for pattern in ("agents/*/agent.json", "*/agent.json", "archive/**/agent.json"):
        for path in sorted(registry.glob(pattern)):
            if path.parent not in found:
                found.append(path.parent)
    return found


def _queued_texts(directory: Path, cap: int = 200) -> list[tuple[float, str]]:
    rows: list[tuple[float, str]] = []
    for path in sorted((directory / "queue").glob("*/*.json")):
        record = _read_json(path)
        if record is None:
            continue
        text = _string(record.get("text"))
        if text and text.strip():
            rows.append((_number(record.get("queued_at")), text))
    rows.sort(key=lambda row: row[0])
    return rows[:cap]


def load_lanes(registry: Path, label: str) -> list[LaneSession]:
    """Every lane-session one registry records, oldest first, with its queued briefs."""

    if not registry.is_dir():
        raise RolesError(f"agent registry not found: {registry}")
    coordinator = _read_json(registry / "coordinator.json") or {}
    designated_name = _string(coordinator.get("name"))
    designated_token = _string(coordinator.get("token"))
    restore = _read_json(registry / "restore.json") or {}
    agents_raw = restore.get("agents")
    standing: set[str] = {
        name for name in agents_raw if isinstance(name, str)
    } if isinstance(agents_raw, list) else set()
    if isinstance(restore.get("coordinator"), str):
        standing.add(str(restore["coordinator"]))

    lanes: list[LaneSession] = []
    seen: set[Path] = set()
    for directory in _record_dirs(registry):
        if directory in seen:
            continue
        seen.add(directory)
        record = _read_json(directory / "agent.json")
        if record is None:
            continue
        name = _string(record.get("name"))
        if name is None:
            continue
        token = _string(record.get("token"))
        parent_name, parent_registry = _creator(record)
        queued = _queued_texts(directory)
        texts = tuple(text for _, text in queued)
        lanes.append(LaneSession(
            registry=label,
            name=name,
            token=token,
            harness=_string(record.get("harness")) or "unknown",
            profile=_string(record.get("profile")),
            cwd=_string(record.get("cwd")),
            created_at=_number(record.get("created_at")),
            lifecycle=_string(record.get("lifecycle")),
            native_session=_native_session(record),
            parent_name=parent_name,
            parent_registry=parent_registry,
            designated_coordinator=(
                name == designated_name
                and (designated_token is None or token == designated_token)
            ),
            in_restore=name in standing,
            brief=texts[0] if texts else None,
            incoming=texts,
        ))
    lanes.sort(key=lambda lane: (lane.created_at, lane.name, lane.token or ""))
    return lanes


_MIX_PATTERNS: tuple[tuple[str, re.Pattern[bytes]], ...] = (
    ("edit", re.compile(
        rb"edit_file|write_file|apply_patch|\"name\":\s*\"(?:Edit|Write)\"")),
    ("commit", re.compile(rb"\bgit\s+commit\b")),
    ("read_search", re.compile(
        rb"read_file|\"name\":\s*\"Read\"|\bgit\s+(?:diff|show|log)\b"
        rb"|\bgh\s+pr\s+(?:view|diff)\b|\brg\s|\bgrep\s")),
    ("test_build", re.compile(
        rb"make validate|\bpytest\b|\bcargo\s+(?:test|build)\b|\bmake\s+(?:test|check)\b")),
    ("lane_management", re.compile(
        rb"agentctl\S*\s+(?:start|stop|send)\b|wrkslots\S*\s+(?:create|remove)\b|land-main")),
    ("chat", re.compile(rb"CHAT_REPLY_|agentctl\S*\s+chat\s+reply|<chat_message>")),
    ("review_verdict", re.compile(
        rb"\bgh\s+pr\s+(?:review|comment)\b|request changes|\bLGTM\b", re.IGNORECASE)),
)


def scan_tool_mix(path: Path, line_cap: int = TOOL_MIX_LINE_CAP) -> ToolMix:
    """Count how a lane's log lines split across tool kinds; counts only, no text retained.

    Raw-line matching is deliberately approximate: it reads the *shape* of a lane's work
    across harnesses without a per-harness parser, and every verdict built on it also
    carries topology and brief evidence. Lane starts are counted exactly, reusing the
    discovery scanner.
    """

    counts: Counter[str] = Counter()
    lines = 0
    try:
        with path.open("rb") as handle:
            for line in handle:
                lines += 1
                if lines > line_cap:
                    lines -= 1
                    break
                for kind, pattern in _MIX_PATTERNS:
                    if pattern.search(line):
                        counts[kind] += 1
    except OSError:
        return ToolMix()
    try:
        starts = len({name for _, name in _log_launches(path)})
    except Exception:  # a truncated tail must not lose the counts already gathered
        starts = 0
    return ToolMix(
        lines=lines,
        edit=counts["edit"], commit=counts["commit"],
        read_search=counts["read_search"], test_build=counts["test_build"],
        lane_management=counts["lane_management"], lane_starts=starts,
        chat=counts["chat"], review_verdict=counts["review_verdict"],
    )


def _with_logs(
    lanes: list[LaneSession],
    claude_projects: Path | None,
    muse_sessions: Path | None,
) -> list[LaneSession]:
    """Attach each lane's tool mix from its own log, when the log can be found.

    Claude lanes join by native session id; Muse lanes by working directory and start
    window, the same join the responsiveness report uses. A lane whose log is not found
    keeps an empty mix and is classified on registry and brief evidence alone.
    """

    muse = muse_logs(muse_sessions) if muse_sessions is not None else {}
    claimed: set[Path] = set()
    enriched: list[LaneSession] = []
    for lane in lanes:
        log: Path | None = None
        if lane.harness == "claude" and lane.native_session and claude_projects is not None:
            hits = sorted(claude_projects.glob(f"*/{lane.native_session}.jsonl"))
            log = hits[0] if hits else None
        elif lane.harness == "muse" and lane.native_session and muse_sessions is not None:
            hits = sorted(muse_sessions.glob(f"*/*/*/{lane.native_session}/session.jsonl"))
            log = hits[0] if hits else None
        if log is None and lane.harness == "muse" and lane.cwd:
            candidates = sorted(
                entry for entry in muse.get(lane.cwd.rstrip("/"), [])
                if lane.created_at - 120 <= entry[0] <= lane.created_at + 600
                and entry[1] not in claimed
            )
            log = candidates[0][1] if candidates else None
        if log is not None and log.is_file():
            claimed.add(log)
            enriched.append(replace(lane, tool_mix=scan_tool_mix(log), has_log=True))
        else:
            enriched.append(lane)
    return enriched


def load_overrides(path: Path) -> dict[str, tuple[str, str | None]]:
    """Declared roles from a JSON file, keyed by session id, ``registry:name``, or name.

    Each value is either a role on its own, such as ``"reviewer"``, or an object naming
    ``role``, ``coordinator_kind``, or both. A coordinator kind without a role means
    that kind of coordinator. Anything else is an error: a misspelt declaration must
    not silently become no declaration.
    """

    try:
        raw = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        raise RolesError(f"cannot read overrides file {path}: {error}") from error
    if not isinstance(raw, dict):
        raise RolesError(f"overrides file {path} must hold a JSON object")
    overrides: dict[str, tuple[str, str | None]] = {}
    for key, value in raw.items():
        if not isinstance(key, str) or not key:
            raise RolesError(f"overrides key must be a non-empty string, got {key!r}")
        if isinstance(value, str):
            role, kind = value, None
        elif isinstance(value, dict):
            role_raw: object = value.get("role")
            kind_raw: object = value.get("coordinator_kind")
            if not isinstance(role_raw, str) and not isinstance(kind_raw, str):
                raise RolesError(f"override for {key!r} names neither role nor kind")
            role = role_raw if isinstance(role_raw, str) else "coordinator"
            kind = kind_raw if isinstance(kind_raw, str) else None
        else:
            raise RolesError(f"override for {key!r} must be a role or an object")
        if role not in ROLES:
            raise RolesError(f"override for {key!r}: unknown role {role!r}")
        if kind is not None and (role != "coordinator" or kind not in COORDINATOR_KINDS):
            raise RolesError(f"override for {key!r}: unknown coordinator kind {kind!r}")
        overrides[key] = (role, kind)
    return overrides


def classify_sources(sources: Sources) -> list[Classification]:
    """Load every registry in ``sources``, attach logs where found, and classify the fleet."""

    lanes: list[LaneSession] = []
    claude_by_label = dict(sources.claude_projects)
    muse_by_label = dict(sources.muse_sessions)
    for label, registry in sources.registries:
        loaded = load_lanes(registry, label)
        lanes.extend(_with_logs(
            loaded, claude_by_label.get(label), muse_by_label.get(label)))
    return classify_fleet(lanes, sources.overrides)


def report_obj(rows: Sequence[Classification]) -> dict[str, object]:
    """The full report: every verdict with its evidence, and a per-role census."""

    census: Counter[str] = Counter()
    for row in rows:
        label = row.role if row.coordinator_kind is None else (
            f"coordinator/{row.coordinator_kind}")
        census[label] += 1
    return {
        "schema_version": 1,
        "unit": "lane-session",
        "counts": dict(sorted(census.items())),
        "lanes": [row.to_json_obj() for row in rows],
    }


def format_markdown(rows: Sequence[Classification]) -> str:
    """A short human report: the census, then every lane that earned a label."""

    census = report_obj(rows)["counts"]
    assert isinstance(census, dict)
    lines = [
        "# Lane roles",
        "",
        f"{len(rows)} lane-session(s): "
        + ", ".join(f"{label} {count}" for label, count in census.items()) + ".",
        "",
        "| lane | role | kind | confidence |",
        "|---|---|---|---:|",
    ]
    for row in rows:
        if row.role == "unclassified":
            continue
        lines.append(
            f"| {row.key} | {row.role} | {row.coordinator_kind or ''} | "
            f"{row.confidence:.2f} |"
        )
    return "\n".join(lines) + "\n"


def run(sources: Sources, output_format: str, json_file: Path | None) -> str:
    """Classify, optionally write the full JSON report, and return the formatted report."""

    if output_format not in ("json", "markdown"):
        raise RolesError(f"unsupported format {output_format!r}")
    rows = classify_sources(sources)
    report = report_obj(rows)
    if json_file is not None:
        json_file.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    if output_format == "json":
        return json.dumps(report, indent=2) + "\n"
    return format_markdown(rows)


__all__ = [
    "COORDINATOR_KINDS",
    "ROLES",
    "Classification",
    "LaneSession",
    "RolesError",
    "Signal",
    "Sources",
    "ToolMix",
    "classify_fleet",
    "classify_sources",
    "format_markdown",
    "load_lanes",
    "load_overrides",
    "report_obj",
    "run",
    "scan_tool_mix",
]
