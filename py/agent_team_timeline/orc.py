"""Read-only Orc SQLite snapshots and provider-neutral timeline normalization."""

from __future__ import annotations

import hashlib
import json
import os
import re
import secrets
import sqlite3
import stat
from collections import Counter, defaultdict
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass, replace
from datetime import datetime, timezone
from pathlib import Path, PurePosixPath

from agent_team_timeline.model import (
    Agent,
    Edge,
    Event,
    SourceSnapshot,
    TeamData,
    ToolCall,
    Turn,
)


_SAFE_COMPONENT = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]*")
_ORC_TOOL = re.compile(r"\borc\.([A-Za-z_][A-Za-z0-9_]*)\s*\(")
_AUXILIARY_POLICY_LEGACY = "legacy-message-prefix-v1"
_AUXILIARY_POLICY_STABLE_SPAWNS = "stable-spawn-subset-v1"
_AUXILIARY_POLICY_NOT_APPLICABLE = "not-applicable"
_AUXILIARY_DEGRADATION_REASON = (
    "conversation-history-rewritten-stable-spawns-preserved"
)
_EMPTY_SHA256 = hashlib.sha256(b"").hexdigest()
_SNAPSHOT_OBJECT_ROOT = ".objects"
_TASK_PROJECTION_ROOT = ".projections"
_TASK_PROJECTION_POLICY = "frozen-note-provenance-v2"
_TASK_PROJECTION_DEGRADATION_REASON = "task-metadata-rewritten-enrichment-preserved"
_SEMANTIC_IDENTITY_LEGACY = "legacy-raw-v1"
_SEMANTIC_IDENTITY_DETERMINISTIC = "normalized-v2"
_SOURCE_STATE_LIVE = "live"
_SOURCE_STATE_DETACHED = "detached"


class OrcParseError(ValueError):
    """Raised when an Orc source or its append-only history is invalid."""


@dataclass(frozen=True)
class OrcAuxiliaryStatus:
    """Bounded provenance for Orc's rewritable conversation-state projection."""

    policy: str
    message_count: int
    message_sha256: str
    stable_spawn_count: int
    stable_spawn_sha256: str
    rewrite_count: int
    last_rewrite_at: str | None
    degraded: bool
    degradation_reason: str | None

    def to_json_obj(self) -> dict[str, object]:
        """Return the manifest representation without raw conversation payloads."""

        return {
            "policy": self.policy,
            "message_count": self.message_count,
            "message_sha256": self.message_sha256,
            "stable_spawn_count": self.stable_spawn_count,
            "stable_spawn_sha256": self.stable_spawn_sha256,
            "rewrite_count": self.rewrite_count,
            "last_rewrite_at": self.last_rewrite_at,
            "degraded": self.degraded,
            "degradation_reason": self.degradation_reason,
        }

    @classmethod
    def from_json_obj(
        cls, raw: Mapping[str, object], where: str
    ) -> OrcAuxiliaryStatus:
        """Strictly decode schema-v2 auxiliary provenance."""

        _require_exact_keys(
            raw,
            {
                "policy",
                "message_count",
                "message_sha256",
                "stable_spawn_count",
                "stable_spawn_sha256",
                "rewrite_count",
                "last_rewrite_at",
                "degraded",
                "degradation_reason",
            },
            where,
        )
        policy = _required_string(raw.get("policy"), f"{where}.policy")
        if policy not in (
            _AUXILIARY_POLICY_STABLE_SPAWNS,
            _AUXILIARY_POLICY_NOT_APPLICABLE,
        ):
            raise OrcParseError(f"{where}.policy: unsupported policy {policy!r}")
        message_sha256 = _sha256_string(
            raw.get("message_sha256"), f"{where}.message_sha256"
        )
        stable_spawn_sha256 = _sha256_string(
            raw.get("stable_spawn_sha256"), f"{where}.stable_spawn_sha256"
        )
        rewrite_count = _nonnegative_integer(
            raw.get("rewrite_count"), f"{where}.rewrite_count"
        )
        last_rewrite_at = _optional_string(
            raw.get("last_rewrite_at"), f"{where}.last_rewrite_at"
        )
        degraded = _boolean(raw.get("degraded"), f"{where}.degraded")
        degradation_reason = _optional_string(
            raw.get("degradation_reason"), f"{where}.degradation_reason"
        )
        if rewrite_count == 0 and (
            last_rewrite_at is not None or degraded or degradation_reason is not None
        ):
            raise OrcParseError(
                f"{where}: zero rewrites cannot carry degradation metadata"
            )
        if rewrite_count > 0 and (
            last_rewrite_at is None
            or not degraded
            or degradation_reason != _AUXILIARY_DEGRADATION_REASON
        ):
            raise OrcParseError(
                f"{where}: rewrite history requires complete degradation metadata"
            )
        if policy == _AUXILIARY_POLICY_NOT_APPLICABLE and (
            rewrite_count != 0
            or _nonnegative_integer(
                raw.get("message_count"), f"{where}.message_count"
            )
            != 0
            or _nonnegative_integer(
                raw.get("stable_spawn_count"), f"{where}.stable_spawn_count"
            )
            != 0
            or message_sha256 != _EMPTY_SHA256
            or stable_spawn_sha256 != _EMPTY_SHA256
        ):
            raise OrcParseError(
                f"{where}: not-applicable auxiliary provenance must be empty"
            )
        return cls(
            policy=policy,
            message_count=_nonnegative_integer(
                raw.get("message_count"), f"{where}.message_count"
            ),
            message_sha256=message_sha256,
            stable_spawn_count=_nonnegative_integer(
                raw.get("stable_spawn_count"), f"{where}.stable_spawn_count"
            ),
            stable_spawn_sha256=stable_spawn_sha256,
            rewrite_count=rewrite_count,
            last_rewrite_at=last_rewrite_at,
            degraded=degraded,
            degradation_reason=degradation_reason,
        )


@dataclass(frozen=True)
class OrcTaskProjection:
    """Pointer and provenance for frozen per-note task enrichment."""

    policy: str
    path: str
    note_count: int
    sha256: str
    observed_enrichment_sha256: str
    rewrite_count: int
    last_rewrite_at: str | None
    degraded: bool
    degradation_reason: str | None

    def to_json_obj(self) -> dict[str, object]:
        """Return the bounded schema-v2 task projection."""

        return {
            "policy": self.policy,
            "path": self.path,
            "note_count": self.note_count,
            "sha256": self.sha256,
            "observed_enrichment_sha256": self.observed_enrichment_sha256,
            "rewrite_count": self.rewrite_count,
            "last_rewrite_at": self.last_rewrite_at,
            "degraded": self.degraded,
            "degradation_reason": self.degradation_reason,
        }

    @classmethod
    def from_json_obj(
        cls, raw: Mapping[str, object], where: str
    ) -> OrcTaskProjection:
        """Strictly decode schema-v2 task evidence."""

        _require_exact_keys(
            raw,
            {
                "policy",
                "path",
                "note_count",
                "sha256",
                "observed_enrichment_sha256",
                "rewrite_count",
                "last_rewrite_at",
                "degraded",
                "degradation_reason",
            },
            where,
        )
        policy = _required_string(raw.get("policy"), f"{where}.policy")
        if policy != _TASK_PROJECTION_POLICY:
            raise OrcParseError(f"{where}.policy: unsupported policy {policy!r}")
        sha256 = _sha256_string(raw.get("sha256"), f"{where}.sha256")
        path = _required_string(raw.get("path"), f"{where}.path")
        if path != _task_projection_relative(sha256):
            raise OrcParseError(f"{where}.path: invalid content-addressed projection path")
        rewrite_count = _nonnegative_integer(
            raw.get("rewrite_count"), f"{where}.rewrite_count"
        )
        last_rewrite_at = _optional_string(
            raw.get("last_rewrite_at"), f"{where}.last_rewrite_at"
        )
        degraded = _boolean(raw.get("degraded"), f"{where}.degraded")
        degradation_reason = _optional_string(
            raw.get("degradation_reason"), f"{where}.degradation_reason"
        )
        if rewrite_count == 0 and (
            last_rewrite_at is not None or degraded or degradation_reason is not None
        ):
            raise OrcParseError(
                f"{where}: zero rewrites cannot carry degradation metadata"
            )
        if rewrite_count > 0 and (
            last_rewrite_at is None
            or not degraded
            or degradation_reason != _TASK_PROJECTION_DEGRADATION_REASON
        ):
            raise OrcParseError(
                f"{where}: rewrite history requires complete degradation metadata"
            )
        return cls(
            policy=policy,
            path=path,
            note_count=_nonnegative_integer(
                raw.get("note_count"), f"{where}.note_count"
            ),
            sha256=sha256,
            observed_enrichment_sha256=_sha256_string(
                raw.get("observed_enrichment_sha256"),
                f"{where}.observed_enrichment_sha256",
            ),
            rewrite_count=rewrite_count,
            last_rewrite_at=last_rewrite_at,
            degraded=degraded,
            degradation_reason=degradation_reason,
        )


@dataclass(frozen=True)
class OrcSourceCopy:
    """One consistent SQLite backup and its logical append-prefix evidence."""

    source_path: str
    snapshot_path: str
    kind: str
    owner_session_id: str
    source_size: int
    snapshot_size: int
    sha256: str
    append_count: int
    append_max_id: int
    append_prefix_sha256: str
    semantic_identity_mode: str
    semantic_sha256: str
    semantic_complete_bytes: int
    semantic_baseline_path: str | None
    source_state: str
    task_source_ordinal: int | None
    auxiliary: OrcAuxiliaryStatus
    task_projection: OrcTaskProjection | None
    captured_at: str

    def to_json_obj(self) -> dict[str, object]:
        """Return this validated source-copy record as a JSON object."""

        return {
            "source_path": self.source_path,
            "snapshot_path": self.snapshot_path,
            "kind": self.kind,
            "owner_session_id": self.owner_session_id,
            "source_size": self.source_size,
            "snapshot_size": self.snapshot_size,
            "sha256": self.sha256,
            "append_count": self.append_count,
            "append_max_id": self.append_max_id,
            "append_prefix_sha256": self.append_prefix_sha256,
            "semantic_identity_mode": self.semantic_identity_mode,
            "semantic_sha256": self.semantic_sha256,
            "semantic_complete_bytes": self.semantic_complete_bytes,
            "semantic_baseline_path": self.semantic_baseline_path,
            "source_state": self.source_state,
            "task_source_ordinal": self.task_source_ordinal,
            "auxiliary": self.auxiliary.to_json_obj(),
            "task_projection": (
                self.task_projection.to_json_obj()
                if self.task_projection is not None
                else None
            ),
            "captured_at": self.captured_at,
        }

    @classmethod
    def from_json_obj(
        cls,
        raw: Mapping[str, object],
        where: str,
        manifest_schema_version: int,
    ) -> OrcSourceCopy:
        """Validate and decode one source-copy record from archive JSON."""

        source_path = _required_string(raw.get("source_path"), f"{where}.source_path")
        snapshot_path = _required_string(
            raw.get("snapshot_path"), f"{where}.snapshot_path"
        )
        _safe_relative(source_path)
        _safe_relative(snapshot_path)
        if manifest_schema_version == 2:
            _require_exact_keys(
                raw,
                {
                    "source_path",
                    "snapshot_path",
                    "kind",
                    "owner_session_id",
                    "source_size",
                    "snapshot_size",
                    "sha256",
                    "append_count",
                    "append_max_id",
                    "append_prefix_sha256",
                    "semantic_identity_mode",
                    "semantic_sha256",
                    "semantic_complete_bytes",
                    "semantic_baseline_path",
                    "source_state",
                    "task_source_ordinal",
                    "auxiliary",
                    "task_projection",
                    "captured_at",
                },
                where,
            )
        kind = _required_string(raw.get("kind"), f"{where}.kind")
        if kind not in ("session", "task"):
            raise OrcParseError(f"{where}.kind: unsupported Orc database kind {kind!r}")
        sha256 = _required_string(raw.get("sha256"), f"{where}.sha256")
        append_digest = _required_string(
            raw.get("append_prefix_sha256"), f"{where}.append_prefix_sha256"
        )
        _sha256_string(sha256, f"{where}.sha256")
        _sha256_string(append_digest, f"{where}.append_prefix_sha256")
        if manifest_schema_version == 1:
            if source_path != snapshot_path:
                raise OrcParseError(
                    f"{where}: schema-v1 source and snapshot paths must match"
                )
            auxiliary = OrcAuxiliaryStatus(
                policy=_AUXILIARY_POLICY_LEGACY,
                message_count=_nonnegative_integer(
                    raw.get("auxiliary_count"), f"{where}.auxiliary_count"
                ),
                message_sha256=_sha256_string(
                    raw.get("auxiliary_prefix_sha256"),
                    f"{where}.auxiliary_prefix_sha256",
                ),
                stable_spawn_count=0,
                stable_spawn_sha256=_EMPTY_SHA256,
                rewrite_count=0,
                last_rewrite_at=None,
                degraded=False,
                degradation_reason=None,
            )
            task_projection: OrcTaskProjection | None = None
            semantic_identity_mode = _SEMANTIC_IDENTITY_LEGACY
            semantic_sha256 = sha256
            semantic_complete_bytes = _nonnegative_integer(
                raw.get("snapshot_size"), f"{where}.snapshot_size"
            )
            semantic_baseline_path: str | None = snapshot_path
            source_state = _SOURCE_STATE_LIVE
            task_source_ordinal: int | None = 0 if kind == "task" else None
        elif manifest_schema_version == 2:
            expected_snapshot_path = _snapshot_object_relative(sha256)
            if snapshot_path != expected_snapshot_path:
                raise OrcParseError(
                    f"{where}.snapshot_path: expected content-addressed path "
                    f"{expected_snapshot_path!r}"
                )
            auxiliary = OrcAuxiliaryStatus.from_json_obj(
                _mapping(raw.get("auxiliary"), f"{where}.auxiliary"),
                f"{where}.auxiliary",
            )
            raw_task_projection = raw.get("task_projection")
            task_projection = (
                None
                if raw_task_projection is None
                else OrcTaskProjection.from_json_obj(
                    _mapping(raw_task_projection, f"{where}.task_projection"),
                    f"{where}.task_projection",
                )
            )
            semantic_sha256 = _sha256_string(
                raw.get("semantic_sha256"), f"{where}.semantic_sha256"
            )
            semantic_identity_mode = _required_string(
                raw.get("semantic_identity_mode"),
                f"{where}.semantic_identity_mode",
            )
            if semantic_identity_mode not in (
                _SEMANTIC_IDENTITY_LEGACY,
                _SEMANTIC_IDENTITY_DETERMINISTIC,
            ):
                raise OrcParseError(
                    f"{where}.semantic_identity_mode: unsupported mode "
                    f"{semantic_identity_mode!r}"
                )
            semantic_complete_bytes = _nonnegative_integer(
                raw.get("semantic_complete_bytes"),
                f"{where}.semantic_complete_bytes",
            )
            semantic_baseline_path = _optional_string(
                raw.get("semantic_baseline_path"),
                f"{where}.semantic_baseline_path",
            )
            if semantic_baseline_path is not None:
                _safe_relative(semantic_baseline_path)
            source_state = _required_string(
                raw.get("source_state"), f"{where}.source_state"
            )
            if source_state not in (_SOURCE_STATE_LIVE, _SOURCE_STATE_DETACHED):
                raise OrcParseError(
                    f"{where}.source_state: unsupported state {source_state!r}"
                )
            raw_ordinal = raw.get("task_source_ordinal")
            task_source_ordinal = (
                None
                if raw_ordinal is None
                else _nonnegative_integer(
                    raw_ordinal, f"{where}.task_source_ordinal"
                )
            )
        else:
            raise OrcParseError(
                f"{where}: unsupported Orc manifest schema {manifest_schema_version}"
            )
        if manifest_schema_version == 2 and (
            (kind == "session" and auxiliary.policy != _AUXILIARY_POLICY_STABLE_SPAWNS)
            or (kind == "task" and auxiliary.policy != _AUXILIARY_POLICY_NOT_APPLICABLE)
            or (kind == "session" and task_projection is not None)
            or (kind == "task" and task_projection is None)
            or (kind == "session" and source_state != _SOURCE_STATE_LIVE)
            or (kind == "session" and task_source_ordinal is not None)
            or (kind == "task" and task_source_ordinal is None)
            or (
                semantic_identity_mode == _SEMANTIC_IDENTITY_LEGACY
                and semantic_baseline_path is None
            )
            or (
                semantic_identity_mode == _SEMANTIC_IDENTITY_DETERMINISTIC
                and semantic_baseline_path is not None
            )
        ):
            raise OrcParseError(
                f"{where}: invalid schema-v2 projections for {kind} source"
            )
        return cls(
            source_path=source_path,
            snapshot_path=snapshot_path,
            kind=kind,
            owner_session_id=_required_string(
                raw.get("owner_session_id"), f"{where}.owner_session_id"
            ),
            source_size=_nonnegative_integer(
                raw.get("source_size"), f"{where}.source_size"
            ),
            snapshot_size=_nonnegative_integer(
                raw.get("snapshot_size"), f"{where}.snapshot_size"
            ),
            sha256=sha256,
            append_count=_nonnegative_integer(
                raw.get("append_count"), f"{where}.append_count"
            ),
            append_max_id=_nonnegative_integer(
                raw.get("append_max_id"), f"{where}.append_max_id"
            ),
            append_prefix_sha256=append_digest,
            semantic_identity_mode=semantic_identity_mode,
            semantic_sha256=semantic_sha256,
            semantic_complete_bytes=semantic_complete_bytes,
            semantic_baseline_path=semantic_baseline_path,
            source_state=source_state,
            task_source_ordinal=task_source_ordinal,
            auxiliary=auxiliary,
            task_projection=task_projection,
            captured_at=_required_string(raw.get("captured_at"), f"{where}.captured_at"),
        )


@dataclass(frozen=True)
class OrcSnapshotResult:
    """Summary of source copies retained by one snapshot pass."""

    sources: tuple[OrcSourceCopy, ...]
    files_changed: int


@dataclass(frozen=True)
class _DiscoveryPlan:
    """Validated live/frozen source set and stable task namespaces for one ingest."""

    sources: tuple[_DiscoveredSource, ...]
    previous_by_path: Mapping[str, OrcSourceCopy]
    selected_session_ids: frozenset[str]
    task_ordinals: Mapping[str, int]
    session_task_paths: Mapping[str, frozenset[str]]


@dataclass(frozen=True)
class _PreparedCandidate:
    """One consistent live/frozen backup plus its validated prior state."""

    discovered: _DiscoveredSource
    previous: OrcSourceCopy | None
    owner_session_id: str
    source_path: Path
    previous_path: Path | None
    temporary_path: Path
    state: _LogicalState
    previous_state: _LogicalState | None


@dataclass(frozen=True)
class _SourceAdvance:
    """Provider-specific semantic/projection result for a prepared source."""

    auxiliary: OrcAuxiliaryStatus
    task_projection: OrcTaskProjection | None
    task_source_ordinal: int | None
    previous_identity: _SemanticIdentity | None
    current_identity: _SemanticIdentity
    staged_objects: tuple[tuple[Path, Path, str], ...]
    temporary_paths: tuple[Path, ...]


@dataclass(frozen=True)
class _SessionMeta:
    session_id: str
    parent_id: str | None
    name: str
    db_name: str | None
    created_at_ms: int
    updated_at_ms: int
    source_path: str


@dataclass(frozen=True)
class _LogicalState:
    append_count: int
    append_max_id: int
    append_prefix_sha256: str


@dataclass(frozen=True)
class _DiscoveredSource:
    """One live Orc database and every session that currently references it."""

    source_path: str
    kind: str
    owner_candidates: tuple[str, ...]
    source_state: str = _SOURCE_STATE_LIVE


@dataclass(frozen=True)
class _StableSpawn:
    """The immutable projection of one AgentBlock needed by normalization."""

    session_id: str
    parent_session_id: str | None
    message_id: int
    block_id: int
    timestamp_ms: int
    agent_id: str

    @property
    def key(self) -> tuple[str, int]:
        """Return the stable identity used for monotonic subset validation."""

        return (self.session_id, self.block_id)


@dataclass(frozen=True)
class _AuxiliaryObservation:
    """One bounded observation of mutable conversation history."""

    messages: tuple[object, ...]
    message_sha256: str
    stable_spawns: tuple[_StableSpawn, ...]
    stable_spawn_sha256: str


@dataclass(frozen=True)
class _TaskNoteEnrichment:
    """Frozen normalization metadata for one append-guarded task note."""

    note_id: int
    task_id: str
    server_author: str | None
    task_owner: str | None
    title: str


@dataclass(frozen=True)
class _TaskEnrichmentObservation:
    """Current task-note enrichment derived from a task database."""

    records: tuple[_TaskNoteEnrichment, ...]
    sha256: str


@dataclass(frozen=True)
class _SemanticIdentity:
    """Deterministic cache identity for one normalized Orc source state."""

    sha256: str
    complete_bytes: int


@dataclass(frozen=True)
class _Spawn:
    thread_id: str
    parent_thread_id: str
    official_name: str
    timestamp_ms: int
    source_line: int
    source_path: str


_CLASSIFICATION_VERSION = "authorship-v2"
_ORC_PERIODIC_REMINDER = (
    "This is your periodic reminder to make sure your running state is aligned "
    "with your overarching goals"
)
_ORC_TIME_ORIENTATION = re.compile(
    r"\AWe're working in eastern time and the current date/time is "
    r"`\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2} (?:EDT|EST)`, use that to orient "
)


def _required_string(value: object, where: str) -> str:
    if not isinstance(value, str) or not value:
        raise OrcParseError(f"{where}: expected a non-empty string")
    return value


def _optional_string(value: object, where: str) -> str | None:
    if value is None or value == "":
        return None
    if not isinstance(value, str):
        raise OrcParseError(f"{where}: expected a string or null")
    return value


def _boolean(value: object, where: str) -> bool:
    if not isinstance(value, bool):
        raise OrcParseError(f"{where}: expected a boolean")
    return value


def _nonnegative_integer(value: object, where: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        raise OrcParseError(f"{where}: expected a non-negative integer")
    return value


def _integer(value: object, where: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise OrcParseError(f"{where}: expected an integer")
    return value


def _sha256_string(value: object, where: str) -> str:
    text = _required_string(value, where)
    if re.fullmatch(r"[0-9a-f]{64}", text) is None:
        raise OrcParseError(f"{where}: expected a SHA-256 digest")
    return text


def _mapping(value: object, where: str) -> dict[str, object]:
    if not isinstance(value, dict):
        raise OrcParseError(f"{where}: expected an object")
    return {str(key): item for key, item in value.items()}


def _optional_json_mapping(value: object, where: str) -> dict[str, object]:
    """Decode one optional JSON object stored in an Orc SQLite text column."""

    text = _optional_string(value, where)
    if text is None:
        return {}
    try:
        decoded: object = json.loads(text)
    except json.JSONDecodeError as error:
        raise OrcParseError(f"{where}: invalid JSON ({error})") from error
    return _mapping(decoded, where)


def _nested_mapping(raw: Mapping[str, object], key: str) -> dict[str, object]:
    value = raw.get(key)
    return _mapping(value, key) if isinstance(value, dict) else {}


def _first_string(*values: object) -> str | None:
    for value in values:
        if isinstance(value, str) and value:
            return value
    return None


def _is_scheduled_orc_input(text: str) -> bool:
    """Recognize only two exact scheduled-message families seen in Orc logs."""

    return text.startswith(_ORC_PERIODIC_REMINDER) or bool(
        _ORC_TIME_ORIENTATION.match(text)
    )


def _orc_input_provenance(
    user_source: Mapping[str, object],
    extra: Mapping[str, object],
    text: str,
    message_id: str,
    owner_gchat_senders: frozenset[str] = frozenset(),
) -> tuple[str, str | None, str, str, str]:
    """Return event kind, author, ingress, author kind, and native identity."""

    orc = _nested_mapping(user_source, "Orc")
    if "Orc" in user_source:
        author = _first_string(orc.get("sender_session"), extra.get("sender_session"))
        return "inter_agent_message", author, "orc", "agent", message_id

    gchat = _nested_mapping(user_source, "GChat")
    if "GChat" in user_source:
        explicit_owner = gchat.get("is_owner")
        if not isinstance(explicit_owner, bool):
            explicit_owner = extra.get("is_owner")
        author = _first_string(
            gchat.get("sender_unixname"),
            extra.get("sender_unixname"),
            gchat.get("sender_display_name"),
            extra.get("sender_display_name"),
            gchat.get("sender_name"),
            extra.get("sender_name"),
        )
        if not isinstance(explicit_owner, bool) and author is not None:
            if author in owner_gchat_senders:
                explicit_owner = True
            elif owner_gchat_senders:
                explicit_owner = False
        author_kind = "owner_human" if explicit_owner is True else "unknown"
        native_id = _first_string(
            gchat.get("message_name"), extra.get("message_name"), message_id
        )
        if native_id is None:
            raise OrcParseError("GChat input lacks a native message identity")
        event_kind = "external_message" if explicit_owner is False else "user_prompt"
        if explicit_owner is False:
            author_kind = "other_human"
        return event_kind, author, "gchat", author_kind, native_id

    submitted = _nested_mapping(user_source, "Submitted")
    submitted_source = submitted.get("source")
    is_tui = submitted_source == "Tui"
    is_web = isinstance(submitted_source, dict) and "Web" in submitted_source
    if (is_tui or is_web) and _is_scheduled_orc_input(text):
        return "system_input", "system", "scheduled", "system", message_id
    if is_tui:
        return "user_prompt", None, "tui", "unknown", message_id
    if is_web:
        return (
            "user_prompt",
            None,
            "submitted_web",
            "external_or_unknown",
            message_id,
        )
    return "user_prompt", None, "orc_unknown", "unknown", message_id


def _require_exact_keys(
    raw: Mapping[str, object], expected: set[str], where: str
) -> None:
    actual = set(raw)
    if actual == expected:
        return
    details: list[str] = []
    missing = sorted(expected - actual)
    unknown = sorted(actual - expected)
    if missing:
        details.append("missing " + ", ".join(missing))
    if unknown:
        details.append("unknown " + ", ".join(unknown))
    raise OrcParseError(f"{where}: invalid fields ({'; '.join(details)})")


def _array(value: object, where: str) -> list[object]:
    if not isinstance(value, list):
        raise OrcParseError(f"{where}: expected an array")
    return list(value)


def _row(value: object, where: str) -> tuple[object, ...]:
    if not isinstance(value, tuple):
        raise OrcParseError(f"{where}: SQLite returned an invalid row")
    return tuple(value)


def _one(connection: sqlite3.Connection, sql: str, where: str) -> tuple[object, ...]:
    raw: object = connection.execute(sql).fetchone()
    if raw is None:
        raise OrcParseError(f"{where}: expected one row")
    return _row(raw, where)


def _iso_ms(value: object, where: str) -> int:
    text = _required_string(value, where)
    normalized = text[:-1] + "+00:00" if text.endswith("Z") else text
    try:
        parsed = datetime.fromisoformat(normalized)
    except ValueError as error:
        raise OrcParseError(f"{where}: invalid ISO timestamp {text!r}") from error
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=timezone.utc)
    return int(parsed.timestamp() * 1000)


def _safe_component(value: str, where: str) -> str:
    if len(value) > 255 or _SAFE_COMPONENT.fullmatch(value) is None:
        raise OrcParseError(f"{where}: unsafe path component {value!r}")
    return value


def _safe_relative(value: str) -> PurePosixPath:
    relative = PurePosixPath(value)
    if (
        relative.is_absolute()
        or not relative.parts
        or any(part in ("", ".", "..") for part in relative.parts)
    ):
        raise OrcParseError(f"unsafe Orc snapshot path {value!r}")
    return relative


def _snapshot_path(snapshot_root: Path, relative: str) -> Path:
    safe = _safe_relative(relative)
    current = snapshot_root
    for index, part in enumerate(safe.parts):
        try:
            mode = current.lstat().st_mode
        except FileNotFoundError:
            mode = None
        if mode is not None and (
            stat.S_ISLNK(mode)
            or (index > 0 and not stat.S_ISDIR(mode))
            or (index == 0 and not stat.S_ISDIR(mode))
        ):
            raise OrcParseError(
                f"snapshot path component is a symlink or non-directory: {current}"
            )
        current = current / part
    return current


def _live_source_path(source_root: Path, relative: str) -> Path:
    """Resolve a source-relative path without following intermediate symlinks."""

    safe = _safe_relative(relative)
    current = source_root
    for index, part in enumerate(safe.parts):
        current = current / part
        try:
            mode = current.lstat().st_mode
        except FileNotFoundError:
            continue
        if stat.S_ISLNK(mode):
            raise OrcParseError(f"symlink in Orc live source path: {current}")
        if index < len(safe.parts) - 1 and not stat.S_ISDIR(mode):
            raise OrcParseError(f"non-directory in Orc live source path: {current}")
    return current


def _snapshot_object_relative(sha256: str) -> str:
    _sha256_string(sha256, "snapshot object digest")
    return f"{_SNAPSHOT_OBJECT_ROOT}/{sha256[:2]}/{sha256}.db"


def _task_projection_relative(sha256: str) -> str:
    _sha256_string(sha256, "task projection digest")
    return f"{_TASK_PROJECTION_ROOT}/{sha256[:2]}/{sha256}.json"


def _read_only(path: Path) -> sqlite3.Connection:
    if path.is_symlink() or not path.is_file():
        raise OrcParseError(f"Orc SQLite source is missing or not a regular file: {path}")
    try:
        connection = sqlite3.connect(path.resolve(strict=True).as_uri() + "?mode=ro", uri=True)
        connection.execute("PRAGMA query_only = ON")
        return connection
    except sqlite3.Error as error:
        raise OrcParseError(f"cannot open Orc SQLite source read-only at {path}: {error}") from error


def _require_tables(
    connection: sqlite3.Connection, names: Sequence[str], where: str
) -> None:
    rows = connection.execute(
        "SELECT name FROM sqlite_master WHERE type = 'table'"
    ).fetchall()
    found = {_required_string(_row(raw, where)[0], where) for raw in rows}
    missing = sorted(set(names) - found)
    if missing:
        raise OrcParseError(f"{where}: missing required tables: {', '.join(missing)}")


def _session_meta(path: Path, source_path: str) -> _SessionMeta:
    connection = _read_only(path)
    try:
        _require_tables(
            connection,
            ("session_meta", "content_blocks", "conversation_state"),
            str(path),
        )
        row = _one(
            connection,
            "SELECT id, parent_id, name, db_name, created_at, updated_at "
            "FROM session_meta LIMIT 1",
            str(path),
        )
    except sqlite3.Error as error:
        raise OrcParseError(
            f"failed to inspect Orc session metadata at {path}: {error}"
        ) from error
    finally:
        connection.close()
    session_id = _safe_component(_required_string(row[0], f"{path}: id"), f"{path}: id")
    parent_id = _optional_string(row[1], f"{path}: parent_id")
    if parent_id is not None:
        _safe_component(parent_id, f"{path}: parent_id")
    db_name = _optional_string(row[3], f"{path}: db_name")
    if db_name is not None:
        _safe_component(db_name, f"{path}: db_name")
    return _SessionMeta(
        session_id=session_id,
        parent_id=parent_id,
        name=_required_string(row[2], f"{path}: name"),
        db_name=db_name,
        created_at_ms=_iso_ms(row[4], f"{path}: created_at"),
        updated_at_ms=_iso_ms(row[5], f"{path}: updated_at"),
        source_path=source_path,
    )


def _associated_db_names(path: Path) -> tuple[str, ...]:
    connection = _read_only(path)
    try:
        table = connection.execute(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' "
            "AND name = 'associated_dbs'"
        ).fetchone()
        if table is None:
            return ()
        rows = connection.execute(
            "SELECT db_name FROM associated_dbs ORDER BY db_name"
        ).fetchall()
    except sqlite3.Error as error:
        raise OrcParseError(
            f"failed to inspect Orc associated task databases at {path}: {error}"
        ) from error
    finally:
        connection.close()
    names: list[str] = []
    for index, raw in enumerate(rows):
        row = _row(raw, f"{path}: associated_dbs[{index}]")
        names.append(
            _safe_component(
                _required_string(row[0], f"{path}: associated_dbs[{index}].db_name"),
                f"{path}: associated_dbs[{index}].db_name",
            )
        )
    return tuple(names)


def _session_task_relatives(
    database: Path, meta: _SessionMeta, _root_session_id: str
) -> tuple[tuple[str, bool, bool], ...]:
    """Return TaskGraph paths and whether their persisted reference requires a file."""

    if meta.parent_id is None and meta.db_name is not None:
        primary_name = meta.db_name
        primary_required = False
    else:
        primary_name = meta.session_id
        primary_required = False
    required_by_path: dict[str, bool] = {
        f".tg/{primary_name}.db": primary_required
    }
    for name in _associated_db_names(database):
        required_by_path[f".tg/{name}.db"] = False
    primary_path = f".tg/{primary_name}.db"
    return tuple(
        (path, required, path == primary_path)
        for path, required in sorted(required_by_path.items())
    )


def _discover_sources(
    source_root: Path, root_session_id: str
) -> tuple[_DiscoveredSource, ...]:
    root_id = _safe_component(root_session_id, "root session id")
    sessions_root = _live_source_path(source_root, ".orc/sessions")
    if not sessions_root.is_dir() or sessions_root.is_symlink():
        raise OrcParseError(f"missing Orc sessions directory: {sessions_root}")
    index_path = _live_source_path(source_root, ".orc/index.db")
    selected: set[str] | None = None
    index_parents: dict[str, str | None] | None = None
    if index_path.is_file() and not index_path.is_symlink():
        connection = _read_only(index_path)
        try:
            _require_tables(connection, ("sessions",), str(index_path))
            rows = connection.execute(
                "SELECT id, parent_id FROM sessions ORDER BY id"
            ).fetchall()
        except sqlite3.Error as error:
            raise OrcParseError(
                f"failed to inspect Orc session index at {index_path}: {error}"
            ) from error
        finally:
            connection.close()
        parents: dict[str, str | None] = {}
        for index, raw in enumerate(rows):
            row = _row(raw, f"{index_path}: sessions[{index}]")
            session_id = _safe_component(
                _required_string(row[0], f"{index_path}: sessions[{index}].id"),
                f"{index_path}: sessions[{index}].id",
            )
            parent = _optional_string(
                row[1], f"{index_path}: sessions[{index}].parent_id"
            )
            parents[session_id] = parent
        if root_id not in parents:
            raise OrcParseError(f"root Orc session {root_id!r} is absent from {index_path}")
        selected = {root_id}
        index_parents = parents
        changed = True
        while changed:
            changed = False
            for session_id, parent in parents.items():
                if parent in selected and session_id not in selected:
                    selected.add(session_id)
                    changed = True

    metas: dict[str, _SessionMeta] = {}
    candidate_names = sorted(selected) if selected is not None else [root_id]
    for session_name in candidate_names:
        session_dir = sessions_root / session_name
        if session_dir.is_symlink() or not session_dir.is_dir():
            if selected is not None:
                raise OrcParseError(f"selected Orc session directory is missing: {session_dir}")
            continue
        database = _live_source_path(
            source_root, f".orc/sessions/{session_name}/session.db"
        )
        if not database.is_file() or database.is_symlink():
            if selected is not None or session_name == root_id:
                raise OrcParseError(f"selected Orc session database is missing: {database}")
            continue
        relative = database.relative_to(source_root).as_posix()
        try:
            meta = _session_meta(database, relative)
        except OrcParseError:
            if selected is not None or session_name == root_id:
                raise
            continue
        if meta.session_id != session_name:
            raise OrcParseError(
                f"Orc session directory {session_name!r} contains session {meta.session_id!r}"
            )
        if index_parents is not None and meta.parent_id != index_parents[session_name]:
            raise OrcParseError(
                f"Orc session parent differs between index and session DB for "
                f"{session_name!r}"
            )
        metas[meta.session_id] = meta
    if root_id not in metas:
        raise OrcParseError(f"root Orc session {root_id!r} was not found under {sessions_root}")
    if selected is None:
        selected = {root_id}

    result: list[_DiscoveredSource] = []
    task_owner_evidence: dict[str, list[tuple[str, bool]]] = defaultdict(list)
    for session_id in sorted(selected):
        meta = metas[session_id]
        result.append(_DiscoveredSource(meta.source_path, "session", (session_id,)))
        session_database = _live_source_path(source_root, meta.source_path)
        for task_relative, required, is_primary in _session_task_relatives(
            session_database, meta, root_id
        ):
            task_path = _live_source_path(source_root, task_relative)
            if not task_path.is_file() or task_path.is_symlink():
                if required:
                    raise OrcParseError(
                        f"session {session_id!r} references task database "
                        f"{task_relative!r}, but {task_path} is missing"
                    )
                continue
            task_owner_evidence[task_relative].append((session_id, is_primary))

    def session_depth(session_id: str) -> int:
        depth = 0
        seen = {session_id}
        parent = metas[session_id].parent_id
        while parent is not None and parent in selected and parent not in seen:
            seen.add(parent)
            depth += 1
            parent = metas[parent].parent_id
        return depth

    result.extend(
        _DiscoveredSource(
            path,
            "task",
            tuple(
                session_id
                for session_id, _ in sorted(
                    evidence,
                    key=lambda item: (
                        not item[1],
                        metas[item[0]].parent_id is not None,
                        session_depth(item[0]),
                        metas[item[0]].created_at_ms,
                        item[0],
                    ),
                )
            ),
        )
        for path, evidence in sorted(task_owner_evidence.items())
    )
    return tuple(sorted(result, key=lambda source: source.source_path))


def _update_digest(digest: hashlib._Hash, value: object) -> None:
    if value is None:
        digest.update(b"N")
        return
    if isinstance(value, bool):
        digest.update(b"I1" if value else b"I0")
        return
    if isinstance(value, int):
        payload = str(value).encode("ascii")
        prefix = b"I"
    elif isinstance(value, float):
        payload = value.hex().encode("ascii")
        prefix = b"F"
    elif isinstance(value, str):
        payload = value.encode("utf-8")
        prefix = b"S"
    elif isinstance(value, bytes):
        payload = value
        prefix = b"B"
    else:
        raise OrcParseError(f"unsupported SQLite value type {type(value).__name__}")
    digest.update(prefix)
    digest.update(len(payload).to_bytes(8, "big"))
    digest.update(payload)


def _query_digest(
    connection: sqlite3.Connection, sql: str, parameters: tuple[object, ...] = ()
) -> tuple[int, str]:
    digest = hashlib.sha256()
    count = 0
    for raw in connection.execute(sql, parameters):
        row = _row(raw, "SQLite digest query")
        digest.update(b"R")
        for value in row:
            _update_digest(digest, value)
        count += 1
    return count, digest.hexdigest()


def _conversation_messages(connection: sqlite3.Connection) -> list[object]:
    row = _one(
        connection,
        "SELECT conversation_json FROM conversation_state WHERE id = 1",
        "conversation_state",
    )
    raw = _required_string(row[0], "conversation_state.conversation_json")
    try:
        parsed: object = json.loads(raw)
    except json.JSONDecodeError as error:
        raise OrcParseError(f"invalid conversation_state JSON: {error}") from error
    root = _mapping(parsed, "conversation_state")
    return _array(root.get("messages"), "conversation_state.messages")


def _messages_digest(messages: Sequence[object]) -> str:
    try:
        encoded = json.dumps(
            list(messages),
            ensure_ascii=False,
            sort_keys=True,
            separators=(",", ":"),
        ).encode("utf-8")
    except (TypeError, ValueError) as error:
        raise OrcParseError(f"conversation state is not canonical JSON: {error}") from error
    return hashlib.sha256(encoded).hexdigest()


def _stable_spawn_records(
    messages: Sequence[object], meta: _SessionMeta, where: str
) -> tuple[_StableSpawn, ...]:
    records: list[_StableSpawn] = []
    seen_keys: set[tuple[str, int]] = set()
    for message_index, raw_message in enumerate(messages):
        message_where = f"{where}: messages[{message_index}]"
        message = _mapping(raw_message, message_where)
        message_id = _integer(message.get("id"), f"{message_where}.id")
        timestamp_ms = _integer(
            message.get("created_at_ms"), f"{message_where}.created_at_ms"
        )
        blocks = _array(message.get("blocks"), f"{message_where}.blocks")
        for block_index, raw_block in enumerate(blocks):
            block_where = f"{message_where}.blocks[{block_index}]"
            block = _mapping(raw_block, block_where)
            if block.get("type") != "AgentBlock":
                continue
            block_id = _integer(block.get("id"), f"{block_where}.id")
            record = _StableSpawn(
                session_id=meta.session_id,
                parent_session_id=meta.parent_id,
                message_id=message_id,
                block_id=block_id,
                timestamp_ms=timestamp_ms,
                agent_id=_required_string(
                    block.get("agent_id"), f"{block_where}.agent_id"
                ),
            )
            if record.key in seen_keys:
                raise OrcParseError(
                    f"{where}: duplicate AgentBlock identity {record.key!r}"
                )
            seen_keys.add(record.key)
            records.append(record)
    return tuple(sorted(records, key=lambda record: record.key))


def _stable_spawn_digest(records: Sequence[_StableSpawn]) -> str:
    digest = hashlib.sha256()
    for record in records:
        digest.update(b"R")
        for value in (
            record.session_id,
            record.parent_session_id,
            record.message_id,
            record.block_id,
            record.timestamp_ms,
            record.agent_id,
        ):
            _update_digest(digest, value)
    return digest.hexdigest()


def _auxiliary_observation(
    path: Path, meta: _SessionMeta
) -> _AuxiliaryObservation:
    connection = _read_only(path)
    try:
        messages = tuple(_conversation_messages(connection))
    finally:
        connection.close()
    records = _stable_spawn_records(messages, meta, str(path))
    return _AuxiliaryObservation(
        messages=messages,
        message_sha256=_messages_digest(messages),
        stable_spawns=records,
        stable_spawn_sha256=_stable_spawn_digest(records),
    )


def _conversation_is_append_extension(
    previous: _AuxiliaryObservation, current: _AuxiliaryObservation
) -> bool:
    previous_count = len(previous.messages)
    return len(current.messages) >= previous_count and _messages_digest(
        current.messages[:previous_count]
    ) == previous.message_sha256


def _validate_stable_spawn_extension(
    previous: _AuxiliaryObservation,
    current: _AuxiliaryObservation,
    relative: str,
) -> None:
    previous_by_key = {record.key: record for record in previous.stable_spawns}
    current_by_key = {record.key: record for record in current.stable_spawns}
    missing = sorted(set(previous_by_key) - set(current_by_key))
    if missing:
        raise OrcParseError(
            f"Orc stable spawn evidence disappeared for {relative}: {missing[0]!r}"
        )
    changed = sorted(
        key
        for key, record in previous_by_key.items()
        if current_by_key[key] != record
    )
    if changed:
        raise OrcParseError(
            f"Orc stable spawn evidence was rewritten for {relative}: {changed[0]!r}"
        )


def _validate_session_meta_extension(
    previous: _SessionMeta, current: _SessionMeta, relative: str
) -> None:
    immutable_previous = (
        previous.session_id,
        previous.parent_id,
        previous.created_at_ms,
    )
    immutable_current = (
        current.session_id,
        current.parent_id,
        current.created_at_ms,
    )
    if immutable_current != immutable_previous:
        raise OrcParseError(
            f"Orc immutable session metadata was rewritten for {relative}"
        )
    if current.updated_at_ms < previous.updated_at_ms:
        raise OrcParseError(
            f"Orc session updated_at moved backwards for {relative}: "
            f"{previous.updated_at_ms} to {current.updated_at_ms}"
        )


def _task_projection_text(records: Sequence[_TaskNoteEnrichment]) -> str:
    value = {
        "schema_version": 1,
        "records": [
            {
                "note_id": record.note_id,
                "task_id": record.task_id,
                "server_author": record.server_author,
                "task_owner": record.task_owner,
                "title": record.title,
            }
            for record in records
        ],
    }
    return json.dumps(
        value,
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
    ) + "\n"


def _task_enrichment_observation(path: Path) -> _TaskEnrichmentObservation:
    connection = _read_only(path)
    try:
        rows = connection.execute(
            "SELECT n.id, n.task_id, n.author_unixname, t.owner, t.title "
            "FROM task_notes n LEFT JOIN tasks t ON t.local_id = n.task_id "
            "ORDER BY n.id"
        ).fetchall()
    except sqlite3.Error as error:
        raise OrcParseError(f"failed to inspect Orc tasks at {path}: {error}") from error
    finally:
        connection.close()
    records: list[_TaskNoteEnrichment] = []
    seen: set[int] = set()
    for raw in rows:
        row = _row(raw, str(path))
        note_id = _nonnegative_integer(row[0], f"{path}: note id")
        author = _optional_string(row[2], f"{path}: note author")
        task_owner = _optional_string(row[3], f"{path}: task owner")
        record = _TaskNoteEnrichment(
            note_id=note_id,
            task_id=_required_string(row[1], f"{path}: note task_id"),
            server_author=author,
            task_owner=task_owner,
            title=_required_string(row[4], f"{path}: task title"),
        )
        if note_id in seen:
            raise OrcParseError(f"{path}: duplicate task note id {note_id}")
        seen.add(note_id)
        records.append(record)
    frozen = tuple(records)
    digest = hashlib.sha256(_task_projection_text(frozen).encode("utf-8")).hexdigest()
    return _TaskEnrichmentObservation(frozen, digest)


def _load_task_projection(
    snapshot_root: Path, status: OrcTaskProjection
) -> tuple[_TaskNoteEnrichment, ...]:
    path = _snapshot_path(snapshot_root, status.path)
    if path.is_symlink() or not path.is_file():
        raise OrcParseError(f"task projection is missing or unsafe: {path}")
    raw_text = path.read_text(encoding="utf-8")
    if hashlib.sha256(raw_text.encode("utf-8")).hexdigest() != status.sha256:
        raise OrcParseError(f"task projection hash mismatch: {path}")
    try:
        parsed: object = json.loads(raw_text)
    except json.JSONDecodeError as error:
        raise OrcParseError(f"invalid task projection JSON at {path}: {error}") from error
    root = _mapping(parsed, str(path))
    _require_exact_keys(root, {"schema_version", "records"}, str(path))
    if root.get("schema_version") != 1:
        raise OrcParseError(f"unsupported task projection schema at {path}")
    records: list[_TaskNoteEnrichment] = []
    prior_id = -1
    for index, raw_record in enumerate(_array(root.get("records"), f"{path}.records")):
        record_obj = _mapping(raw_record, f"{path}.records[{index}]")
        _require_exact_keys(
            record_obj,
            {"note_id", "task_id", "server_author", "task_owner", "title"},
            f"{path}.records[{index}]",
        )
        record = _TaskNoteEnrichment(
            note_id=_nonnegative_integer(
                record_obj.get("note_id"), f"{path}.records[{index}].note_id"
            ),
            task_id=_required_string(
                record_obj.get("task_id"), f"{path}.records[{index}].task_id"
            ),
            server_author=_optional_string(
                record_obj.get("server_author"),
                f"{path}.records[{index}].server_author",
            ),
            task_owner=_optional_string(
                record_obj.get("task_owner"),
                f"{path}.records[{index}].task_owner",
            ),
            title=_required_string(
                record_obj.get("title"), f"{path}.records[{index}].title"
            ),
        )
        if record.note_id <= prior_id:
            raise OrcParseError(f"task projection note IDs are not strictly ordered: {path}")
        prior_id = record.note_id
        records.append(record)
    frozen = tuple(records)
    if len(frozen) != status.note_count:
        raise OrcParseError(f"task projection count mismatch: {path}")
    if _task_projection_text(frozen) != raw_text:
        raise OrcParseError(f"task projection is not canonical: {path}")
    return frozen


def _stage_task_projection(
    records: Sequence[_TaskNoteEnrichment], snapshot_root: Path
) -> tuple[Path, Path, str, str]:
    text = _task_projection_text(records)
    sha256 = hashlib.sha256(text.encode("utf-8")).hexdigest()
    relative = _task_projection_relative(sha256)
    target = _snapshot_path(snapshot_root, relative)
    temporary = snapshot_root / ".staging" / (
        f"task-projection-{os.getpid()}-{secrets.token_hex(8)}.json"
    )
    _ensure_snapshot_parent(temporary, snapshot_root)
    with temporary.open("x", encoding="utf-8", newline="") as handle:
        handle.write(text)
        handle.flush()
    return temporary, target, sha256, relative


def _semantic_identity(payload: Mapping[str, object]) -> _SemanticIdentity:
    try:
        encoded = json.dumps(
            dict(payload),
            ensure_ascii=False,
            sort_keys=True,
            separators=(",", ":"),
        ).encode("utf-8")
    except (TypeError, ValueError) as error:
        raise OrcParseError(f"semantic identity is not canonical JSON: {error}") from error
    return _SemanticIdentity(hashlib.sha256(encoded).hexdigest(), len(encoded))


def _session_semantic_identity(
    source_path: str,
    owner_session_id: str,
    state: _LogicalState,
    meta: _SessionMeta,
    auxiliary: _AuxiliaryObservation,
) -> _SemanticIdentity:
    return _semantic_identity(
        {
            "schema_version": 2,
            "kind": "session",
            "source_path": source_path,
            "owner_session_id": owner_session_id,
            "append_count": state.append_count,
            "append_max_id": state.append_max_id,
            "append_prefix_sha256": state.append_prefix_sha256,
            "stable_spawn_sha256": auxiliary.stable_spawn_sha256,
            "session_id": meta.session_id,
            "parent_id": meta.parent_id,
            "name": meta.name,
            "created_at_ms": meta.created_at_ms,
        }
    )


def _task_semantic_identity(
    source_path: str,
    owner_session_id: str,
    task_source_ordinal: int,
    state: _LogicalState,
    projection_sha256: str,
) -> _SemanticIdentity:
    return _semantic_identity(
        {
            "schema_version": 2,
            "kind": "task",
            "source_path": source_path,
            "owner_session_id": owner_session_id,
            "task_source_ordinal": task_source_ordinal,
            "append_count": state.append_count,
            "append_max_id": state.append_max_id,
            "append_prefix_sha256": state.append_prefix_sha256,
            "task_projection_sha256": projection_sha256,
        }
    )


def _validate_manifest_logical_state(
    source: OrcSourceCopy, state: _LogicalState, where: str
) -> None:
    if (
        state.append_count != source.append_count
        or state.append_max_id != source.append_max_id
        or state.append_prefix_sha256 != source.append_prefix_sha256
    ):
        raise OrcParseError(f"Orc logical state does not match its manifest: {where}")


def _validate_recorded_semantic_identity(
    source: OrcSourceCopy, identity: _SemanticIdentity, where: str
) -> None:
    if source.semantic_identity_mode != _SEMANTIC_IDENTITY_DETERMINISTIC:
        return
    if (
        source.semantic_sha256 != identity.sha256
        or source.semantic_complete_bytes != identity.complete_bytes
    ):
        raise OrcParseError(
            f"Orc deterministic semantic identity does not match artifacts: {where}"
        )


def _validate_legacy_semantic_identity(
    snapshot_root: Path,
    source: OrcSourceCopy,
    current_identity: _SemanticIdentity,
    projection_sha256: str | None,
) -> None:
    if source.semantic_identity_mode != _SEMANTIC_IDENTITY_LEGACY:
        return
    if source.semantic_baseline_path is None:
        raise OrcParseError(
            f"legacy Orc semantic identity lacks a baseline for {source.source_path}"
        )
    baseline_path = _snapshot_path(snapshot_root, source.semantic_baseline_path)
    if baseline_path.is_symlink() or not baseline_path.is_file():
        raise OrcParseError(f"legacy Orc semantic baseline is missing or unsafe: {baseline_path}")
    if (
        _sha256_file(baseline_path) != source.semantic_sha256
        or baseline_path.stat().st_size != source.semantic_complete_bytes
    ):
        raise OrcParseError(
            f"legacy Orc semantic baseline does not match its cache identity: {baseline_path}"
        )
    baseline_state = _logical_state(baseline_path, source.kind)
    if source.kind == "session":
        baseline_meta = _session_meta(baseline_path, source.source_path)
        baseline_auxiliary = _auxiliary_observation(baseline_path, baseline_meta)
        baseline_identity = _session_semantic_identity(
            source.source_path,
            source.owner_session_id,
            baseline_state,
            baseline_meta,
            baseline_auxiliary,
        )
    else:
        if projection_sha256 is None:
            raise OrcParseError(
                f"legacy task semantic identity lacks a projection for {source.source_path}"
            )
        baseline_identity = _task_semantic_identity(
            source.source_path,
            source.owner_session_id,
            source.task_source_ordinal or 0,
            baseline_state,
            projection_sha256,
        )
    if baseline_identity != current_identity:
        raise OrcParseError(
            f"legacy Orc semantic state changed without an identity transition: "
            f"{source.source_path}"
        )


def _validate_source_semantic_identity(
    snapshot_root: Path, source: OrcSourceCopy
) -> _SemanticIdentity:
    path = _snapshot_path(snapshot_root, source.snapshot_path)
    if path.is_symlink() or not path.is_file() or _sha256_file(path) != source.sha256:
        raise OrcParseError(f"Orc snapshot does not match its manifest: {path}")
    state = _logical_state(path, source.kind)
    _validate_manifest_logical_state(source, state, str(path))
    projection_sha256: str | None = None
    if source.kind == "session":
        meta = _session_meta(path, source.source_path)
        auxiliary = _auxiliary_observation(path, meta)
        if (
            source.auxiliary.message_count != len(auxiliary.messages)
            or source.auxiliary.message_sha256 != auxiliary.message_sha256
            or source.auxiliary.stable_spawn_count != len(auxiliary.stable_spawns)
            or source.auxiliary.stable_spawn_sha256
            != auxiliary.stable_spawn_sha256
        ):
            raise OrcParseError(
                f"Orc auxiliary evidence does not match its snapshot: {path}"
            )
        identity = _session_semantic_identity(
            source.source_path,
            source.owner_session_id,
            state,
            meta,
            auxiliary,
        )
    else:
        if source.task_projection is None:
            raise OrcParseError(f"task source lacks frozen enrichment: {source.source_path}")
        _load_task_projection(snapshot_root, source.task_projection)
        observation = _task_enrichment_observation(path)
        if (
            observation.sha256
            != source.task_projection.observed_enrichment_sha256
        ):
            raise OrcParseError(
                f"task enrichment observation does not match its snapshot: {path}"
            )
        projection_sha256 = source.task_projection.sha256
        identity = _task_semantic_identity(
            source.source_path,
            source.owner_session_id,
            source.task_source_ordinal or 0,
            state,
            projection_sha256,
        )
    _validate_recorded_semantic_identity(source, identity, source.source_path)
    _validate_legacy_semantic_identity(
        snapshot_root, source, identity, projection_sha256
    )
    return identity


def _logical_state(
    path: Path,
    kind: str,
    *,
    prefix_max_id: int | None = None,
    legacy_task_fields: bool = False,
) -> _LogicalState:
    connection = _read_only(path)
    try:
        check = _one(connection, "PRAGMA quick_check", str(path))
        if check != ("ok",):
            raise OrcParseError(f"SQLite quick_check failed for {path}: {check!r}")
        if kind == "session":
            _require_tables(
                connection,
                ("session_meta", "content_blocks", "conversation_state"),
                str(path),
            )
            count_row = _one(
                connection,
                "SELECT COUNT(*), COALESCE(MAX(rowid), 0) FROM content_blocks",
                str(path),
            )
            append_count = _nonnegative_integer(count_row[0], f"{path}: content count")
            append_max_id = _nonnegative_integer(count_row[1], f"{path}: max rowid")
            limit = append_max_id if prefix_max_id is None else prefix_max_id
            prefix_count, prefix_digest = _query_digest(
                connection,
                "SELECT rowid, id, message_id, session_id, block_index, created_at_ms, "
                "turn_index, role, block_type, content, searchable_text, code_input, "
                "code_output, code_exit_code, model, user_source, token_count, extra "
                "FROM content_blocks WHERE rowid <= ? ORDER BY rowid",
                (limit,),
            )
            return _LogicalState(
                append_count=append_count if prefix_max_id is None else prefix_count,
                append_max_id=append_max_id,
                append_prefix_sha256=prefix_digest,
            )
        if kind == "task":
            _require_tables(connection, ("tasks", "task_notes"), str(path))
            count_row = _one(
                connection,
                "SELECT COUNT(*), COALESCE(MAX(id), 0) FROM task_notes",
                str(path),
            )
            append_count = _nonnegative_integer(count_row[0], f"{path}: note count")
            append_max_id = _nonnegative_integer(count_row[1], f"{path}: max note id")
            limit = append_max_id if prefix_max_id is None else prefix_max_id
            selected_fields = (
                "id, task_id, content, created_at, server_comment_id, author_unixname"
                if legacy_task_fields
                else "id, task_id, content, created_at"
            )
            prefix_count, prefix_digest = _query_digest(
                connection,
                f"SELECT {selected_fields} FROM task_notes WHERE id <= ? ORDER BY id",
                (limit,),
            )
            return _LogicalState(
                append_count=append_count if prefix_max_id is None else prefix_count,
                append_max_id=append_max_id,
                append_prefix_sha256=prefix_digest,
            )
        raise OrcParseError(f"unsupported Orc source kind {kind!r}")
    except sqlite3.Error as error:
        raise OrcParseError(f"failed to inspect Orc SQLite source {path}: {error}") from error
    finally:
        connection.close()


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while True:
            chunk = handle.read(1024 * 1024)
            if not chunk:
                return digest.hexdigest()
            digest.update(chunk)


def _fsync_regular_file(path: Path) -> None:
    try:
        file_fd = os.open(path, os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW)
    except OSError as error:
        raise OrcParseError(f"cannot open snapshot object for fsync {path}: {error}") from error
    try:
        mode = os.fstat(file_fd).st_mode
        if not stat.S_ISREG(mode):
            raise OrcParseError(f"snapshot fsync target is not a regular file: {path}")
        os.fsync(file_fd)
    finally:
        os.close(file_fd)


def _fsync_directory(path: Path) -> None:
    try:
        directory_fd = os.open(
            path,
            os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC | os.O_NOFOLLOW,
        )
    except OSError as error:
        raise OrcParseError(f"cannot open snapshot directory for fsync {path}: {error}") from error
    try:
        os.fsync(directory_fd)
    finally:
        os.close(directory_fd)


def _ensure_snapshot_parent(path: Path, snapshot_root: Path) -> None:
    snapshot_root_existed = snapshot_root.exists()
    snapshot_root.mkdir(parents=True, exist_ok=True)
    if snapshot_root.is_symlink() or not snapshot_root.is_dir():
        raise OrcParseError(f"snapshot root is a symlink or non-directory: {snapshot_root}")
    if not snapshot_root_existed:
        _fsync_directory(snapshot_root.parent)
    relative = path.parent.relative_to(snapshot_root)
    current = snapshot_root
    for part in relative.parts:
        current = current / part
        if current.exists():
            if current.is_symlink() or not current.is_dir():
                raise OrcParseError(
                    f"snapshot path component is a symlink or non-directory: {current}"
                )
        else:
            current.mkdir(mode=0o700)
            _fsync_directory(current.parent)


def _backup_database(source: Path, destination: Path, snapshot_root: Path) -> None:
    _ensure_snapshot_parent(destination, snapshot_root)
    if destination.exists() and (destination.is_symlink() or not destination.is_file()):
        raise OrcParseError(f"snapshot target is a symlink or non-file: {destination}")
    source_connection = _read_only(source)
    try:
        destination_connection = sqlite3.connect(destination)
        try:
            source_connection.backup(destination_connection)
            destination_connection.commit()
        finally:
            destination_connection.close()
    except sqlite3.Error as error:
        raise OrcParseError(f"failed to back up Orc SQLite source {source}: {error}") from error
    finally:
        source_connection.close()


def _publish_snapshot_candidate(
    temporary: Path,
    target: Path,
    expected_sha256: str,
    snapshot_root: Path,
) -> bool:
    """Atomically publish one immutable object, or reuse an identical orphan."""

    _ensure_snapshot_parent(target, snapshot_root)
    _fsync_regular_file(temporary)
    try:
        mode = target.lstat().st_mode
    except FileNotFoundError:
        mode = None
    if mode is not None:
        if not stat.S_ISREG(mode) or stat.S_ISLNK(mode):
            raise OrcParseError(
                f"snapshot object target is a symlink or non-file: {target}"
            )
        if _sha256_file(target) != expected_sha256:
            raise OrcParseError(
                f"preexisting snapshot object has the wrong hash: {target}"
            )
        _fsync_regular_file(target)
        _fsync_directory(target.parent)
        temporary.unlink()
        return False
    try:
        os.link(temporary, target)
    except FileExistsError:
        if target.is_symlink() or not target.is_file():
            raise OrcParseError(
                f"snapshot object target is a symlink or non-file: {target}"
            )
        if _sha256_file(target) != expected_sha256:
            raise OrcParseError(
                f"raced snapshot object has the wrong hash: {target}"
            )
        _fsync_regular_file(target)
        _fsync_directory(target.parent)
        temporary.unlink()
        return False
    if _sha256_file(target) != expected_sha256:
        raise OrcParseError(f"published snapshot object failed hash verification: {target}")
    _fsync_directory(target.parent)
    temporary.unlink()
    return True


def _prune_managed_objects(
    snapshot_root: Path,
    root_name: str,
    extension: str,
    retained: set[str],
) -> int:
    object_root = snapshot_root / root_name
    try:
        root_mode = object_root.lstat().st_mode
    except FileNotFoundError:
        return 0
    if stat.S_ISLNK(root_mode) or not stat.S_ISDIR(root_mode):
        raise OrcParseError(f"Orc snapshot object root is unsafe: {object_root}")
    removed = 0
    for prefix in sorted(object_root.iterdir(), key=lambda path: path.name):
        prefix_mode = prefix.lstat().st_mode
        if stat.S_ISLNK(prefix_mode):
            raise OrcParseError(f"symlink in Orc snapshot object store: {prefix}")
        if not stat.S_ISDIR(prefix_mode) or re.fullmatch(r"[0-9a-f]{2}", prefix.name) is None:
            continue
        prefix_changed = False
        for candidate in sorted(prefix.iterdir(), key=lambda path: path.name):
            candidate_mode = candidate.lstat().st_mode
            if stat.S_ISLNK(candidate_mode):
                raise OrcParseError(f"symlink in Orc snapshot object store: {candidate}")
            match = re.fullmatch(
                rf"([0-9a-f]{{64}}){re.escape(extension)}", candidate.name
            )
            if (
                not stat.S_ISREG(candidate_mode)
                or match is None
                or match.group(1)[:2] != prefix.name
            ):
                continue
            relative = candidate.relative_to(snapshot_root).as_posix()
            if relative in retained:
                continue
            candidate.unlink()
            removed += 1
            prefix_changed = True
        if prefix_changed:
            _fsync_directory(prefix)
    return removed


def prune_orc_snapshot_objects(
    snapshot_root: Path, retained_sources: Sequence[OrcSourceCopy]
) -> int:
    """Remove unreferenced managed objects after a durable manifest commit."""

    retained_databases: set[str] = set()
    retained_projections: set[str] = set()
    for source in retained_sources:
        expected = _snapshot_object_relative(source.sha256)
        if source.snapshot_path != expected:
            raise OrcParseError(
                f"refusing Orc object GC for unmanaged snapshot path {source.snapshot_path!r}"
            )
        retained_databases.add(source.snapshot_path)
        retained_items: tuple[tuple[str, str], ...] = (
            (source.snapshot_path, source.sha256),
        )
        if source.semantic_identity_mode == _SEMANTIC_IDENTITY_LEGACY:
            if source.semantic_baseline_path is None:
                raise OrcParseError(
                    f"legacy Orc source lacks semantic baseline: {source.source_path}"
                )
            retained_items = (
                *retained_items,
                (source.semantic_baseline_path, source.semantic_sha256),
            )
            if source.semantic_baseline_path.startswith(
                f"{_SNAPSHOT_OBJECT_ROOT}/"
            ):
                retained_databases.add(source.semantic_baseline_path)
        if source.task_projection is not None:
            retained_projections.add(source.task_projection.path)
            retained_items = (
                *retained_items,
                (source.task_projection.path, source.task_projection.sha256),
            )
        for relative, expected_sha256 in retained_items:
            path = _snapshot_path(snapshot_root, relative)
            try:
                mode = path.lstat().st_mode
            except FileNotFoundError as error:
                raise OrcParseError(
                    f"retained Orc snapshot artifact is missing: {path}"
                ) from error
            if stat.S_ISLNK(mode) or not stat.S_ISREG(mode):
                raise OrcParseError(f"retained Orc snapshot artifact is unsafe: {path}")
            if _sha256_file(path) != expected_sha256:
                raise OrcParseError(
                    f"retained Orc snapshot artifact has wrong hash: {path}"
                )
            if (
                relative == source.semantic_baseline_path
                and path.stat().st_size != source.semantic_complete_bytes
            ):
                raise OrcParseError(
                    f"retained Orc semantic baseline has wrong size: {path}"
                )
    return _prune_managed_objects(
        snapshot_root,
        _SNAPSHOT_OBJECT_ROOT,
        ".db",
        retained_databases,
    ) + _prune_managed_objects(
        snapshot_root,
        _TASK_PROJECTION_ROOT,
        ".json",
        retained_projections,
    )


def prune_orc_staging(snapshot_root: Path) -> int:
    """Remove managed candidates left by a terminated ingest transaction."""

    staging = _snapshot_path(snapshot_root, ".staging")
    try:
        mode = staging.lstat().st_mode
    except FileNotFoundError:
        return 0
    if stat.S_ISLNK(mode) or not stat.S_ISDIR(mode):
        raise OrcParseError(f"Orc staging root is unsafe: {staging}")
    managed = re.compile(
        r"(?:orc|task-projection)-[0-9]+-[0-9a-f]{16}\.(?:db|json)"
    )
    removed = 0
    for candidate in sorted(staging.iterdir(), key=lambda item: item.name):
        candidate_mode = candidate.lstat().st_mode
        if stat.S_ISLNK(candidate_mode) or not stat.S_ISREG(candidate_mode):
            raise OrcParseError(f"unsafe entry in Orc staging directory: {candidate}")
        if managed.fullmatch(candidate.name) is None:
            continue
        candidate.unlink()
        removed += 1
    if removed:
        _fsync_directory(staging)
    return removed


def _plan_snapshot_sources(
    source_root: Path,
    root_session_id: str,
    snapshot_root: Path,
    previous_sources: Sequence[OrcSourceCopy],
) -> _DiscoveryPlan:
    live_discovered = list(_discover_sources(source_root, root_session_id))
    discovered_paths = {source.source_path for source in live_discovered}
    if len(discovered_paths) != len(live_discovered):
        raise OrcParseError("Orc source discovery produced duplicate source records")
    previous_by_path: dict[str, OrcSourceCopy] = {}
    for source in previous_sources:
        if source.source_path in previous_by_path:
            raise OrcParseError(
                f"duplicate previous Orc source record: {source.source_path}"
            )
        previous_by_path[source.source_path] = source
    selected_session_ids = frozenset(
        source.owner_candidates[0]
        for source in live_discovered
        if source.kind == "session"
    )
    newly_detached_paths: set[str] = set()
    session_task_paths: dict[str, frozenset[str]] = {}
    for discovered_source in live_discovered:
        if discovered_source.kind != "session":
            continue
        live_path = _live_source_path(source_root, discovered_source.source_path)
        live_meta = _session_meta(live_path, discovered_source.source_path)
        live_task_paths = frozenset(
            path
            for path, _, _ in _session_task_relatives(
                live_path, live_meta, root_session_id
            )
        )
        session_task_paths[discovered_source.source_path] = live_task_paths
        previous = previous_by_path.get(discovered_source.source_path)
        if previous is None:
            continue
        previous_path = _snapshot_path(snapshot_root, previous.snapshot_path)
        if (
            previous_path.is_symlink()
            or not previous_path.is_file()
            or _sha256_file(previous_path) != previous.sha256
        ):
            raise OrcParseError(
                f"previous Orc snapshot does not match its manifest: {previous_path}"
            )
        prior_meta = _session_meta(previous_path, discovered_source.source_path)
        prior_task_paths = {
            path
            for path, _, _ in _session_task_relatives(
                previous_path, prior_meta, root_session_id
            )
        }
        newly_detached_paths.update(prior_task_paths - live_task_paths)

    planned_live_task_paths = {
        path
        for paths in session_task_paths.values()
        for path in paths
        if _live_source_path(source_root, path).is_file()
    }
    discovered_live_task_paths = {
        item.source_path for item in live_discovered if item.kind == "task"
    }
    if planned_live_task_paths != discovered_live_task_paths:
        raise OrcParseError(
            "Orc task database references changed during source discovery; retry ingest"
        )

    frozen_paths = {
        source.source_path
        for source in previous_sources
        if source.kind == "task"
        and source.source_path not in discovered_paths
        and (
            source.source_state == _SOURCE_STATE_DETACHED
            or source.source_path in newly_detached_paths
        )
    }
    disappeared = sorted(set(previous_by_path) - discovered_paths - frozen_paths)
    if disappeared:
        raise OrcParseError(
            "previously observed Orc source disappeared: " + ", ".join(disappeared)
        )
    for path in sorted(frozen_paths):
        previous = previous_by_path[path]
        live_discovered.append(
            _DiscoveredSource(
                path,
                "task",
                (previous.owner_session_id,),
                _SOURCE_STATE_DETACHED,
            )
        )
    discovered = tuple(sorted(live_discovered, key=lambda item: item.source_path))
    used_task_ordinals: dict[str, set[int]] = defaultdict(set)
    for prior_source in previous_sources:
        if (
            prior_source.kind == "task"
            and prior_source.task_source_ordinal is not None
        ):
            used_task_ordinals[prior_source.owner_session_id].add(
                prior_source.task_source_ordinal
            )
    task_ordinals: dict[str, int] = {}
    for discovered_item in discovered:
        if discovered_item.kind != "task":
            continue
        previous = previous_by_path.get(discovered_item.source_path)
        owner = (
            previous.owner_session_id
            if previous is not None
            else discovered_item.owner_candidates[0]
        )
        if previous is not None and previous.task_source_ordinal is not None:
            ordinal = previous.task_source_ordinal
        else:
            ordinal = 0
            while ordinal in used_task_ordinals[owner]:
                ordinal += 1
        task_ordinals[discovered_item.source_path] = ordinal
        used_task_ordinals[owner].add(ordinal)
    return _DiscoveryPlan(
        discovered,
        previous_by_path,
        selected_session_ids,
        task_ordinals,
        session_task_paths,
    )


def _prepare_snapshot_candidate(
    source_root: Path,
    snapshot_root: Path,
    discovered: _DiscoveredSource,
    previous: OrcSourceCopy | None,
    selected_session_ids: frozenset[str],
    temporary: Path,
) -> _PreparedCandidate:
    relative = discovered.source_path
    if previous is None:
        owner_session_id = discovered.owner_candidates[0]
        previous_path: Path | None = None
        source_path = _live_source_path(source_root, relative)
    else:
        if previous.kind != discovered.kind:
            raise OrcParseError(
                f"Orc source kind changed for {relative}: "
                f"{previous.kind!r} to {discovered.kind!r}"
            )
        if (
            previous.owner_session_id not in discovered.owner_candidates
            and previous.owner_session_id not in selected_session_ids
        ):
            raise OrcParseError(
                f"Orc source ownership changed for {relative}: prior owner "
                f"{previous.owner_session_id!r} no longer references the source"
            )
        owner_session_id = previous.owner_session_id
        previous_path = _snapshot_path(snapshot_root, previous.snapshot_path)
        if (
            previous_path.is_symlink()
            or not previous_path.is_file()
            or _sha256_file(previous_path) != previous.sha256
        ):
            raise OrcParseError(
                f"previous Orc snapshot does not match its manifest: {previous_path}"
            )
        source_path = (
            previous_path
            if discovered.source_state == _SOURCE_STATE_DETACHED
            else _live_source_path(source_root, relative)
        )
    _backup_database(source_path, temporary, snapshot_root)
    state = _logical_state(temporary, discovered.kind)
    previous_state: _LogicalState | None = None
    if previous is not None:
        if previous_path is None:
            raise AssertionError("validated previous snapshot path is missing")
        if discovered.kind == "task" and previous.task_projection is None:
            legacy_state = _logical_state(
                previous_path, discovered.kind, legacy_task_fields=True
            )
            if (
                legacy_state.append_count != previous.append_count
                or legacy_state.append_max_id != previous.append_max_id
                or legacy_state.append_prefix_sha256 != previous.append_prefix_sha256
            ):
                raise OrcParseError(
                    f"legacy Orc task prefix does not match its manifest: {relative}"
                )
            previous_state = _logical_state(previous_path, discovered.kind)
        else:
            previous_state = _logical_state(previous_path, discovered.kind)
            _validate_manifest_logical_state(previous, previous_state, relative)
        prefix = _logical_state(
            temporary,
            discovered.kind,
            prefix_max_id=previous.append_max_id,
        )
        if state.append_count < previous.append_count:
            raise OrcParseError(
                f"Orc {discovered.kind} append history shrank for {relative}: "
                f"{previous.append_count} to {state.append_count} rows"
            )
        if prefix.append_count != previous.append_count:
            raise OrcParseError(
                f"Orc {discovered.kind} append prefix lost rows for {relative}"
            )
        if prefix.append_prefix_sha256 != previous_state.append_prefix_sha256:
            raise OrcParseError(
                f"Orc {discovered.kind} existing append prefix was rewritten for "
                f"{relative}"
            )
    return _PreparedCandidate(
        discovered,
        previous,
        owner_session_id,
        source_path,
        previous_path,
        temporary,
        state,
        previous_state,
    )


def _advance_session_candidate(
    candidate: _PreparedCandidate,
    snapshot_root: Path,
    captured_at: str,
) -> _SourceAdvance:
    relative = candidate.discovered.source_path
    current_meta = _session_meta(candidate.temporary_path, relative)
    if current_meta.session_id != candidate.owner_session_id:
        raise OrcParseError(
            f"Orc session identity changed for {relative}: "
            f"{candidate.owner_session_id!r} to {current_meta.session_id!r}"
        )
    current_auxiliary = _auxiliary_observation(candidate.temporary_path, current_meta)
    rewrite_detected = False
    rewrite_count = 0
    last_rewrite_at: str | None = None
    degraded = False
    previous_identity: _SemanticIdentity | None = None
    previous = candidate.previous
    if previous is not None:
        if candidate.previous_path is None or candidate.previous_state is None:
            raise AssertionError("validated previous session state is missing")
        previous_meta = _session_meta(candidate.previous_path, relative)
        _validate_session_meta_extension(previous_meta, current_meta, relative)
        previous_auxiliary = _auxiliary_observation(
            candidate.previous_path, previous_meta
        )
        if previous.auxiliary.policy == _AUXILIARY_POLICY_LEGACY:
            if (
                previous.auxiliary.message_count != len(previous_auxiliary.messages)
                or previous.auxiliary.message_sha256
                != previous_auxiliary.message_sha256
            ):
                raise OrcParseError(
                    "previous Orc auxiliary message evidence does not match "
                    f"its snapshot for {relative}"
                )
            previous_identity = _session_semantic_identity(
                relative,
                candidate.owner_session_id,
                candidate.previous_state,
                previous_meta,
                previous_auxiliary,
            )
            _validate_legacy_semantic_identity(
                snapshot_root, previous, previous_identity, None
            )
        else:
            previous_identity = _validate_source_semantic_identity(
                snapshot_root, previous
            )
        _validate_stable_spawn_extension(
            previous_auxiliary, current_auxiliary, relative
        )
        rewrite_detected = not _conversation_is_append_extension(
            previous_auxiliary, current_auxiliary
        )
        rewrite_count = previous.auxiliary.rewrite_count + int(rewrite_detected)
        last_rewrite_at = (
            captured_at if rewrite_detected else previous.auxiliary.last_rewrite_at
        )
        degraded = previous.auxiliary.degraded or rewrite_detected
    auxiliary = OrcAuxiliaryStatus(
        policy=_AUXILIARY_POLICY_STABLE_SPAWNS,
        message_count=len(current_auxiliary.messages),
        message_sha256=current_auxiliary.message_sha256,
        stable_spawn_count=len(current_auxiliary.stable_spawns),
        stable_spawn_sha256=current_auxiliary.stable_spawn_sha256,
        rewrite_count=rewrite_count,
        last_rewrite_at=last_rewrite_at,
        degraded=degraded,
        degradation_reason=_AUXILIARY_DEGRADATION_REASON if degraded else None,
    )
    current_identity = _session_semantic_identity(
        relative,
        candidate.owner_session_id,
        candidate.state,
        current_meta,
        current_auxiliary,
    )
    return _SourceAdvance(
        auxiliary,
        None,
        None,
        previous_identity,
        current_identity,
        (),
        (),
    )


def _advance_task_candidate(
    candidate: _PreparedCandidate,
    snapshot_root: Path,
    captured_at: str,
    task_source_ordinal: int,
) -> _SourceAdvance:
    relative = candidate.discovered.source_path
    auxiliary = OrcAuxiliaryStatus(
        policy=_AUXILIARY_POLICY_NOT_APPLICABLE,
        message_count=0,
        message_sha256=_EMPTY_SHA256,
        stable_spawn_count=0,
        stable_spawn_sha256=_EMPTY_SHA256,
        rewrite_count=0,
        last_rewrite_at=None,
        degraded=False,
        degradation_reason=None,
    )
    current_enrichment = _task_enrichment_observation(candidate.temporary_path)
    if len(current_enrichment.records) != candidate.state.append_count:
        raise OrcParseError(
            f"task enrichment count does not match guarded notes for {relative}"
        )
    previous_identity: _SemanticIdentity | None = None
    projection_rewrite_count = 0
    projection_last_rewrite_at: str | None = None
    projection_degraded = False
    previous = candidate.previous
    if previous is None:
        merged_records = current_enrichment.records
    else:
        if candidate.previous_path is None or candidate.previous_state is None:
            raise AssertionError("validated previous task state is missing")
        previous_records = (
            _task_enrichment_observation(candidate.previous_path).records
            if previous.task_projection is None
            else _load_task_projection(snapshot_root, previous.task_projection)
        )
        previous_projection_sha256 = hashlib.sha256(
            _task_projection_text(previous_records).encode("utf-8")
        ).hexdigest()
        if previous.task_projection is None:
            previous_identity = _task_semantic_identity(
                relative,
                candidate.owner_session_id,
                task_source_ordinal,
                candidate.previous_state,
                previous_projection_sha256,
            )
            _validate_legacy_semantic_identity(
                snapshot_root,
                previous,
                previous_identity,
                previous_projection_sha256,
            )
        else:
            previous_identity = _validate_source_semantic_identity(
                snapshot_root, previous
            )
        if len(previous_records) != previous.append_count:
            raise OrcParseError(
                f"previous task projection count does not match guarded notes for {relative}"
            )
        current_by_id = {
            record.note_id: record for record in current_enrichment.records
        }
        missing = sorted(
            record.note_id
            for record in previous_records
            if record.note_id not in current_by_id
        )
        if missing:
            raise OrcParseError(
                f"guarded task note is missing from enrichment for {relative}: {missing[0]}"
            )
        for record in previous_records:
            if current_by_id[record.note_id].task_id != record.task_id:
                raise OrcParseError(
                    f"guarded task_id changed for note {record.note_id} in {relative}"
                )
        previous_ids = {record.note_id for record in previous_records}
        appended_records = tuple(
            record
            for record in current_enrichment.records
            if record.note_id not in previous_ids
        )
        merged_records = (*previous_records, *appended_records)
        if any(
            merged_records[index - 1].note_id >= merged_records[index].note_id
            for index in range(1, len(merged_records))
        ):
            raise OrcParseError(
                f"new task note IDs do not extend the frozen projection for {relative}"
            )
        current_prior = tuple(
            current_by_id[record.note_id] for record in previous_records
        )
        current_prior_sha256 = hashlib.sha256(
            _task_projection_text(current_prior).encode("utf-8")
        ).hexdigest()
        prior_observed_sha256 = (
            hashlib.sha256(
                _task_projection_text(previous_records).encode("utf-8")
            ).hexdigest()
            if previous.task_projection is None
            else previous.task_projection.observed_enrichment_sha256
        )
        rewrite_detected = current_prior_sha256 != prior_observed_sha256
        projection_rewrite_count = (
            0
            if previous.task_projection is None
            else previous.task_projection.rewrite_count
        ) + int(rewrite_detected)
        projection_last_rewrite_at = (
            captured_at
            if rewrite_detected
            else (
                None
                if previous.task_projection is None
                else previous.task_projection.last_rewrite_at
            )
        )
        projection_degraded = rewrite_detected or (
            previous.task_projection is not None
            and previous.task_projection.degraded
        )
    (
        projection_temporary,
        projection_target,
        projection_sha256,
        projection_relative,
    ) = _stage_task_projection(merged_records, snapshot_root)
    task_projection = OrcTaskProjection(
        policy=_TASK_PROJECTION_POLICY,
        path=projection_relative,
        note_count=len(merged_records),
        sha256=projection_sha256,
        observed_enrichment_sha256=current_enrichment.sha256,
        rewrite_count=projection_rewrite_count,
        last_rewrite_at=projection_last_rewrite_at,
        degraded=projection_degraded,
        degradation_reason=(
            _TASK_PROJECTION_DEGRADATION_REASON
            if projection_degraded
            else None
        ),
    )
    current_identity = _task_semantic_identity(
        relative,
        candidate.owner_session_id,
        task_source_ordinal,
        candidate.state,
        projection_sha256,
    )
    return _SourceAdvance(
        auxiliary,
        task_projection,
        task_source_ordinal,
        previous_identity,
        current_identity,
        ((projection_temporary, projection_target, projection_sha256),),
        (projection_temporary,),
    )


def _publish_staged_objects(
    staged_objects: Sequence[tuple[Path, Path, str]], snapshot_root: Path
) -> int:
    changed = 0
    for temporary, target, sha256 in staged_objects:
        changed += int(
            _publish_snapshot_candidate(
                temporary, target, sha256, snapshot_root
            )
        )
    return changed


def snapshot_orc_lineage(
    source_root: Path,
    root_session_id: str,
    snapshot_root: Path,
    previous_sources: Sequence[OrcSourceCopy],
    captured_at: str,
) -> OrcSnapshotResult:
    """Publish immutable SQLite objects after validating provider-specific monotonicity."""

    plan = _plan_snapshot_sources(
        source_root, root_session_id, snapshot_root, previous_sources
    )
    discovered = plan.sources
    previous_by_path = plan.previous_by_path
    selected_session_ids = plan.selected_session_ids
    task_ordinals = plan.task_ordinals

    staged_objects: list[tuple[Path, Path, str]] = []
    result_sources: list[OrcSourceCopy] = []
    temporary_paths: list[Path] = []
    try:
        for discovered_source in discovered:
            relative = discovered_source.source_path
            kind = discovered_source.kind
            previous = previous_by_path.get(relative)
            staging_root = snapshot_root / ".staging"
            temporary = staging_root / (
                f"orc-{os.getpid()}-{secrets.token_hex(8)}.db"
            )
            temporary_paths.append(temporary)
            candidate = _prepare_snapshot_candidate(
                source_root,
                snapshot_root,
                discovered_source,
                previous,
                selected_session_ids,
                temporary,
            )
            owner_session_id = candidate.owner_session_id
            previous_target = candidate.previous_path
            source_path = candidate.source_path
            full = candidate.state
            previous_state = candidate.previous_state
            if kind == "session":
                staged_meta = _session_meta(temporary, relative)
                staged_task_paths = frozenset(
                    path
                    for path, _, _ in _session_task_relatives(
                        temporary, staged_meta, root_session_id
                    )
                )
                if staged_task_paths != plan.session_task_paths[relative]:
                    raise OrcParseError(
                        "Orc task database references changed during snapshot; "
                        "retry ingest"
                    )
            advance = (
                _advance_session_candidate(candidate, snapshot_root, captured_at)
                if kind == "session"
                else _advance_task_candidate(
                    candidate,
                    snapshot_root,
                    captured_at,
                    task_ordinals[relative],
                )
            )
            temporary_paths.extend(advance.temporary_paths)
            staged_objects.extend(advance.staged_objects)
            task_source_ordinal = advance.task_source_ordinal
            auxiliary = advance.auxiliary
            task_projection = advance.task_projection
            previous_identity = advance.previous_identity
            current_identity = advance.current_identity
            snapshot_size = temporary.stat().st_size
            snapshot_sha256 = _sha256_file(temporary)
            snapshot_relative = _snapshot_object_relative(snapshot_sha256)
            target = _snapshot_path(snapshot_root, snapshot_relative)
            effective_captured_at = (
                previous.captured_at
                if previous is not None and previous.sha256 == snapshot_sha256
                else captured_at
            )
            semantic_changed = (
                previous_identity is None or previous_identity != current_identity
            )
            if (
                previous is not None
                and previous.semantic_identity_mode == _SEMANTIC_IDENTITY_LEGACY
                and not semantic_changed
            ):
                semantic_identity_mode = _SEMANTIC_IDENTITY_LEGACY
                semantic_sha256 = previous.semantic_sha256
                semantic_complete_bytes = previous.semantic_complete_bytes
                semantic_baseline_path = previous.semantic_baseline_path
            else:
                semantic_identity_mode = _SEMANTIC_IDENTITY_DETERMINISTIC
                semantic_sha256 = current_identity.sha256
                semantic_complete_bytes = current_identity.complete_bytes
                semantic_baseline_path = None
            staged_objects.append((temporary, target, snapshot_sha256))
            result_sources.append(
                OrcSourceCopy(
                    source_path=relative,
                    snapshot_path=snapshot_relative,
                    kind=kind,
                    owner_session_id=owner_session_id,
                    source_size=source_path.stat().st_size,
                    snapshot_size=snapshot_size,
                    sha256=snapshot_sha256,
                    append_count=full.append_count,
                    append_max_id=full.append_max_id,
                    append_prefix_sha256=full.append_prefix_sha256,
                    semantic_identity_mode=semantic_identity_mode,
                    semantic_sha256=semantic_sha256,
                    semantic_complete_bytes=semantic_complete_bytes,
                    semantic_baseline_path=semantic_baseline_path,
                    source_state=discovered_source.source_state,
                    task_source_ordinal=task_source_ordinal,
                    auxiliary=auxiliary,
                    task_projection=task_projection,
                    captured_at=effective_captured_at,
                )
            )

        changed = _publish_staged_objects(staged_objects, snapshot_root)
        return OrcSnapshotResult(
            sources=tuple(result_sources), files_changed=changed
        )
    finally:
        for temporary in temporary_paths:
            if temporary.exists():
                temporary.unlink()


def _conversation_spawns(
    path: Path, meta: _SessionMeta
) -> tuple[_Spawn, ...]:
    observation = _auxiliary_observation(path, meta)
    result = [
        _Spawn(
            thread_id=f"orc-agent-{meta.session_id[:8]}-{record.block_id}",
            parent_thread_id=meta.session_id,
            official_name=record.agent_id,
            timestamp_ms=record.timestamp_ms,
            source_line=record.block_id,
            source_path=meta.source_path,
        )
        for record in observation.stable_spawns
    ]
    return tuple(
        sorted(result, key=lambda item: (item.timestamp_ms, item.source_line))
    )


def _content_records(
    path: Path, meta: _SessionMeta
) -> tuple[tuple[Event, ...], tuple[ToolCall, ...], tuple[Turn, ...]]:
    connection = _read_only(path)
    try:
        rows = connection.execute(
            "SELECT rowid, id, message_id, created_at_ms, turn_index, role, block_type, "
            "content, code_input, code_output, code_exit_code, user_source, extra "
            "FROM content_blocks "
            "ORDER BY created_at_ms, rowid"
        ).fetchall()
    finally:
        connection.close()
    owner_gchat_senders: set[str] = set()
    for raw in rows:
        row = _row(raw, str(path))
        role = _required_string(row[5], f"{path}: role")
        if role != "user":
            continue
        user_source = _optional_json_mapping(
            row[11], f"{path}: content block user_source"
        )
        gchat = _nested_mapping(user_source, "GChat")
        if "GChat" not in user_source:
            continue
        extra = _optional_json_mapping(
            row[12], f"{path}: content block extra"
        )
        explicit_owner = gchat.get("is_owner")
        if not isinstance(explicit_owner, bool):
            explicit_owner = extra.get("is_owner")
        if explicit_owner is not True:
            continue
        sender_aliases = (
            gchat.get("sender_unixname"),
            extra.get("sender_unixname"),
            gchat.get("sender_display_name"),
            extra.get("sender_display_name"),
            gchat.get("sender_name"),
            extra.get("sender_name"),
        )
        owner_gchat_senders.update(
            value for value in sender_aliases if isinstance(value, str) and value
        )
    events: list[Event] = []
    tools: list[ToolCall] = []
    turn_bounds: dict[int, tuple[int, int, str | None]] = {}
    for raw in rows:
        row = _row(raw, str(path))
        rowid = _integer(row[0], f"{path}: rowid")
        block_id = _required_string(row[1], f"{path}: block id")
        message_id = _required_string(row[2], f"{path}: message id")
        timestamp_ms = _integer(row[3], f"{path}: created_at_ms")
        turn_index = _integer(row[4], f"{path}: turn_index")
        role = _required_string(row[5], f"{path}: role")
        block_type = _required_string(row[6], f"{path}: block_type")
        content = _optional_string(row[7], f"{path}: content")
        turn_id = f"orc-turn-{meta.session_id[:8]}-{turn_index}"
        prior = turn_bounds.get(turn_index)
        if prior is None:
            turn_bounds[turn_index] = (timestamp_ms, timestamp_ms + 1, content)
        else:
            last_message = content if role == "assistant" and content else prior[2]
            turn_bounds[turn_index] = (
                min(prior[0], timestamp_ms),
                max(prior[1], timestamp_ms + 1),
                last_message,
            )
        event_kind: str | None = None
        event_role: str | None = None
        event_author: str | None = None
        event_recipient: str | None = None
        ingress_kind: str | None = None
        author_kind: str | None = None
        source_native_id: str | None = message_id
        if block_type == "text" and role == "user":
            user_source = _optional_json_mapping(
                row[11], f"{path}: content block {block_id} user_source"
            )
            extra = _optional_json_mapping(
                row[12], f"{path}: content block {block_id} extra"
            )
            (
                event_kind,
                event_author,
                ingress_kind,
                author_kind,
                source_native_id,
            ) = _orc_input_provenance(
                user_source,
                extra,
                content or "",
                message_id,
                frozenset(owner_gchat_senders),
            )
            if event_kind == "inter_agent_message":
                event_role = None
                event_recipient = meta.session_id
            elif event_kind == "system_input":
                event_role = "system"
            else:
                event_role = "user"
        elif block_type == "text" and role in ("assistant", "notification"):
            event_kind = "assistant_message"
            event_role = "assistant"
            event_author = meta.session_id
            ingress_kind = "orc"
            author_kind = "agent"
        if event_kind is not None and content:
            events.append(
                Event(
                    event_id=f"orc-block-{block_id}",
                    thread_id=meta.session_id,
                    turn_id=turn_id,
                    timestamp_ms=timestamp_ms,
                    kind=event_kind,
                    role=event_role,
                    phase=None,
                    text=content,
                    content_availability="plaintext",
                    encrypted_content=None,
                    author=event_author,
                    recipient=event_recipient,
                    source_line=rowid,
                    ingress_kind=ingress_kind,
                    author_kind=author_kind,
                    source_native_id=source_native_id,
                    classification_version=_CLASSIFICATION_VERSION,
                )
            )
        if block_type == "code_execution":
            code_input = _optional_string(row[8], f"{path}: code_input")
            code_output = _optional_string(row[9], f"{path}: code_output")
            exit_code = row[10]
            counts = Counter(_ORC_TOOL.findall(code_input or ""))
            status = (
                "failed"
                if isinstance(exit_code, int) and not isinstance(exit_code, bool) and exit_code != 0
                else "completed"
            )
            tools.append(
                ToolCall(
                    call_id=f"orc-code-{block_id}",
                    item_id=block_id,
                    thread_id=meta.session_id,
                    turn_id=turn_id,
                    name="code_execution",
                    namespace="orc",
                    started_at_ms=timestamp_ms,
                    ended_at_ms=timestamp_ms + 1,
                    status=status,
                    input_text=code_input,
                    output_text=code_output,
                    nested_tools=tuple(sorted(counts.items())),
                    source_line=rowid,
                )
            )
    turns = tuple(
        Turn(
            turn_id=f"orc-turn-{meta.session_id[:8]}-{turn_index}",
            thread_id=meta.session_id,
            started_at_ms=bounds[0],
            ended_at_ms=bounds[1],
            status="completed",
            first_token_ms=None,
            error=None,
            last_agent_message=bounds[2],
        )
        for turn_index, bounds in sorted(turn_bounds.items())
    )
    return tuple(events), tuple(tools), turns


def _select_spawn(
    spawns: Mapping[tuple[str, str], Sequence[_Spawn]],
    coordinator_id: str,
    owner: str,
    timestamp_ms: int,
) -> _Spawn | None:
    eligible = [
        spawn
        for spawn in spawns.get((coordinator_id, owner), ())
        if spawn.timestamp_ms <= timestamp_ms
    ]
    return eligible[-1] if eligible else None


def _task_records(
    path: Path,
    source_path: str,
    coordinator_id: str,
    task_source_ordinal: int,
    spawns: Mapping[tuple[str, str], Sequence[_Spawn]],
    enrichments: Mapping[int, _TaskNoteEnrichment],
) -> tuple[tuple[Event, ...], tuple[Turn, ...], tuple[_Spawn, ...]]:
    connection = _read_only(path)
    try:
        rows = connection.execute(
            "SELECT id, task_id, content, created_at FROM task_notes "
            "ORDER BY created_at, id"
        ).fetchall()
    finally:
        connection.close()
    events: list[Event] = []
    turns: list[Turn] = []
    inferred: dict[str, _Spawn] = {}
    for raw in rows:
        row = _row(raw, str(path))
        note_id = _integer(row[0], f"{path}: note id")
        task_id = _required_string(row[1], f"{path}: task id")
        enrichment = enrichments.get(note_id)
        if enrichment is None:
            raise OrcParseError(f"missing frozen enrichment for task note {note_id}")
        if enrichment.task_id != task_id:
            raise OrcParseError(f"frozen task_id mismatch for task note {note_id}")
        content = _optional_string(row[2], f"{path}: note content")
        if content is None:
            continue
        timestamp_ms = _iso_ms(row[3], f"{path}: note created_at")
        title = enrichment.title
        namespace = "" if task_source_ordinal == 0 else f"-s{task_source_ordinal}"
        turn_id = f"orc-note-turn-{coordinator_id[:8]}{namespace}-{note_id}"
        event_id = f"orc-note-{coordinator_id[:8]}{namespace}-{note_id}"
        if enrichment.server_author is not None:
            text = (
                f"[{task_id} · {title} · external author: "
                f"{enrichment.server_author}]\n\n{content}"
            )
            events.append(
                Event(
                    event_id=event_id,
                    thread_id=coordinator_id,
                    turn_id=turn_id,
                    timestamp_ms=timestamp_ms,
                    kind="external_message",
                    role="user",
                    phase=None,
                    text=text,
                    content_availability="plaintext",
                    encrypted_content=None,
                    author=enrichment.server_author,
                    recipient=None,
                    source_line=note_id,
                )
            )
            turns.append(
                Turn(
                    turn_id=turn_id,
                    thread_id=coordinator_id,
                    started_at_ms=timestamp_ms,
                    ended_at_ms=timestamp_ms + 1000,
                    status="completed",
                    first_token_ms=None,
                    error=None,
                    last_agent_message=None,
                )
            )
            continue
        owner = enrichment.task_owner
        inferred_key = owner or "__unattributed__"
        spawn = (
            None
            if owner is None
            else _select_spawn(spawns, coordinator_id, owner, timestamp_ms)
        )
        if spawn is None:
            spawn = inferred.get(inferred_key)
        if spawn is None:
            if owner is None:
                official_name = "Unattributed Task Work"
                thread_id = f"orc-unattributed-{coordinator_id[:8]}{namespace or '-s0'}"
            else:
                official_name = owner
                owner_slug = (
                    re.sub(r"[^A-Za-z0-9._-]+", "-", owner).strip("-") or "agent"
                )
                digest = hashlib.sha256(
                    f"{coordinator_id}\0{owner}".encode("utf-8")
                ).hexdigest()[:12]
                thread_id = f"orc-owner-{owner_slug[:80]}-{digest}"
            spawn = _Spawn(
                thread_id=thread_id,
                parent_thread_id=coordinator_id,
                official_name=official_name,
                timestamp_ms=timestamp_ms,
                source_line=note_id,
                source_path=source_path,
            )
            inferred[inferred_key] = spawn
        provenance = " · unattributed local task work" if owner is None else ""
        text = f"[{task_id} · {title}{provenance}]\n\n{content}"
        events.append(
            Event(
                event_id=event_id,
                thread_id=spawn.thread_id,
                turn_id=turn_id,
                timestamp_ms=timestamp_ms,
                kind="inter_agent_message",
                role="agent",
                phase=None,
                text=text,
                content_availability="plaintext",
                encrypted_content=None,
                author=spawn.thread_id,
                recipient=coordinator_id,
                source_line=note_id,
            )
        )
        turns.append(
            Turn(
                turn_id=turn_id,
                thread_id=spawn.thread_id,
                started_at_ms=timestamp_ms,
                ended_at_ms=timestamp_ms + 1000,
                status="completed",
                first_token_ms=None,
                error=None,
                last_agent_message=text,
            )
        )
    return tuple(events), tuple(turns), tuple(inferred.values())


def _agent_depth(
    thread_id: str, parents: Mapping[str, str | None]
) -> int:
    depth = 0
    seen = {thread_id}
    parent = parents.get(thread_id)
    while parent is not None and parent not in seen:
        seen.add(parent)
        depth += 1
        parent = parents.get(parent)
    return depth


def load_orc_team(
    snapshot_root: Path,
    root_session_id: str,
    team_slug: str,
    display_timezone: str,
    source_copies: Sequence[OrcSourceCopy],
) -> TeamData:
    """Normalize validated archive-local Orc SQLite backups into ``TeamData``."""

    session_sources = [source for source in source_copies if source.kind == "session"]
    task_sources = [source for source in source_copies if source.kind == "task"]
    source_paths = [source.source_path for source in source_copies]
    if len(set(source_paths)) != len(source_paths):
        raise OrcParseError("snapshot set contains duplicate Orc source records")
    for source in source_copies:
        if source.auxiliary.policy != _AUXILIARY_POLICY_LEGACY:
            _validate_source_semantic_identity(snapshot_root, source)
    metas: dict[str, _SessionMeta] = {}
    session_snapshot_paths: dict[str, str] = {}
    for source in session_sources:
        path = _snapshot_path(snapshot_root, source.snapshot_path)
        meta = _session_meta(path, source.source_path)
        if meta.session_id != source.owner_session_id:
            raise OrcParseError(
                f"session snapshot {source.source_path!r} contains {meta.session_id!r}, "
                f"not manifest owner {source.owner_session_id!r}"
            )
        if meta.session_id in metas:
            raise OrcParseError(f"duplicate Orc session id {meta.session_id!r}")
        metas[meta.session_id] = meta
        session_snapshot_paths[meta.session_id] = source.snapshot_path
    if root_session_id not in metas:
        raise OrcParseError(f"snapshot set does not contain root session {root_session_id!r}")
    prefixes: dict[str, str] = {}
    for session_id in sorted(metas):
        prefix = session_id[:8]
        prior = prefixes.get(prefix)
        if prior is not None and prior != session_id:
            raise OrcParseError(
                f"Orc session-id prefix collision {prefix!r}: {prior!r} and {session_id!r}"
            )
        prefixes[prefix] = session_id
    for source in task_sources:
        if source.owner_session_id not in metas:
            raise OrcParseError(
                f"task source {source.source_path!r} has unknown owner "
                f"{source.owner_session_id!r}"
            )

    session_events: list[Event] = []
    tools: list[ToolCall] = []
    turns: list[Turn] = []
    all_spawns: list[_Spawn] = []
    for session_id in sorted(metas):
        meta = metas[session_id]
        path = _snapshot_path(snapshot_root, session_snapshot_paths[session_id])
        content_events, session_tools, session_turns = _content_records(path, meta)
        session_events.extend(content_events)
        tools.extend(session_tools)
        turns.extend(session_turns)
        all_spawns.extend(_conversation_spawns(path, meta))

    spawns_by_name: dict[tuple[str, str], list[_Spawn]] = defaultdict(list)
    for spawn in all_spawns:
        spawns_by_name[(spawn.parent_thread_id, spawn.official_name)].append(spawn)
    for values in spawns_by_name.values():
        values.sort(key=lambda item: (item.timestamp_ms, item.source_line))

    task_events: list[Event] = []
    inferred_spawns: list[_Spawn] = []
    for source in task_sources:
        path = _snapshot_path(snapshot_root, source.snapshot_path)
        frozen_records = (
            _task_enrichment_observation(path).records
            if source.task_projection is None
            else _load_task_projection(snapshot_root, source.task_projection)
        )
        note_events, task_turns, inferred = _task_records(
            path,
            source.source_path,
            source.owner_session_id,
            source.task_source_ordinal or 0,
            spawns_by_name,
            {record.note_id: record for record in frozen_records},
        )
        task_events.extend(note_events)
        turns.extend(task_turns)
        inferred_spawns.extend(inferred)
    all_spawns.extend(inferred_spawns)
    spawn_ids: dict[str, _Spawn] = {}
    for spawn in all_spawns:
        prior_spawn = spawn_ids.get(spawn.thread_id)
        if prior_spawn is not None:
            if (
                spawn.thread_id.startswith("orc-owner-")
                and spawn.parent_thread_id == prior_spawn.parent_thread_id
                and spawn.official_name == prior_spawn.official_name
            ):
                if (spawn.timestamp_ms, spawn.source_path, spawn.source_line) < (
                    prior_spawn.timestamp_ms,
                    prior_spawn.source_path,
                    prior_spawn.source_line,
                ):
                    spawn_ids[spawn.thread_id] = spawn
                continue
            raise OrcParseError(
                f"Orc agent id collision {spawn.thread_id!r} between "
                f"{prior_spawn.source_path!r} and {spawn.source_path!r}"
            )
        if spawn.thread_id in metas:
            raise OrcParseError(
                f"Orc generated agent id collides with session id {spawn.thread_id!r}"
            )
        spawn_ids[spawn.thread_id] = spawn
    all_spawns = list(spawn_ids.values())

    normalized_events = sorted(
        [*session_events, *task_events],
        key=lambda item: (item.timestamp_ms, item.event_id),
    )
    events_by_thread: dict[str, list[Event]] = defaultdict(list)
    for event in normalized_events:
        events_by_thread[event.thread_id].append(event)

    parents: dict[str, str | None] = {
        meta.session_id: meta.parent_id for meta in metas.values()
    }
    for spawn in all_spawns:
        parents[spawn.thread_id] = spawn.parent_thread_id

    agents: list[Agent] = []
    for session_id in sorted(metas):
        meta = metas[session_id]
        parent = meta.parent_id if meta.parent_id in metas else None
        agent_path = "/root" if session_id == root_session_id else f"/root/{meta.name}"
        own = events_by_thread.get(session_id, [])
        activity_ends = [meta.created_at_ms + 1]
        activity_ends.extend(event.timestamp_ms + 1 for event in own)
        activity_ends.extend(
            (tool.ended_at_ms or tool.started_at_ms) + 1
            for tool in tools
            if tool.thread_id == session_id
        )
        activity_ends.extend(
            (turn.ended_at_ms or turn.started_at_ms) + 1
            for turn in turns
            if turn.thread_id == session_id
        )
        activity_ends.extend(
            spawn.timestamp_ms + 1
            for spawn in all_spawns
            if spawn.parent_thread_id == session_id
        )
        ended = max(activity_ends)
        agents.append(
            Agent(
                thread_id=session_id,
                parent_thread_id=parent,
                agent_path=agent_path,
                nickname=meta.name if session_id != root_session_id else None,
                role="coordinator",
                depth=_agent_depth(session_id, parents),
                started_at_ms=meta.created_at_ms,
                ended_at_ms=max(meta.created_at_ms + 1, ended),
                status="completed",
                source_path=meta.source_path,
            )
        )

    spawns_by_official: dict[tuple[str, str], list[_Spawn]] = defaultdict(list)
    for spawn in all_spawns:
        spawns_by_official[(spawn.parent_thread_id, spawn.official_name)].append(spawn)
    for values in spawns_by_official.values():
        values.sort(key=lambda item: (item.timestamp_ms, item.source_line))
    for spawn_key in sorted(spawns_by_official):
        values = spawns_by_official[spawn_key]
        official_name = spawn_key[1]
        for index, spawn in enumerate(values):
            own = events_by_thread.get(spawn.thread_id, [])
            activity_end = max(
                (event.timestamp_ms + 1000 for event in own),
                default=spawn.timestamp_ms + 1000,
            )
            next_start = values[index + 1].timestamp_ms if index + 1 < len(values) else None
            ended = min(activity_end, next_start) if next_start is not None else activity_end
            parent_path = next(
                (
                    agent.agent_path
                    for agent in agents
                    if agent.thread_id == spawn.parent_thread_id
                ),
                "/root",
            )
            agents.append(
                Agent(
                    thread_id=spawn.thread_id,
                    parent_thread_id=spawn.parent_thread_id,
                    agent_path=f"{parent_path.rstrip('/')}/{official_name}",
                    nickname=None,
                    role="worker",
                    depth=_agent_depth(spawn.thread_id, parents),
                    started_at_ms=spawn.timestamp_ms,
                    ended_at_ms=max(spawn.timestamp_ms + 1, ended),
                    status="completed",
                    source_path=spawn.source_path,
                )
            )

    propagated_ends = {
        agent.thread_id: agent.ended_at_ms or agent.started_at_ms + 1
        for agent in agents
    }
    for agent in sorted(agents, key=lambda item: item.depth, reverse=True):
        if agent.parent_thread_id is None:
            continue
        child_end = propagated_ends[agent.thread_id]
        propagated_ends[agent.parent_thread_id] = max(
            propagated_ends.get(agent.parent_thread_id, child_end), child_end
        )
    agents = [
        replace(agent, ended_at_ms=propagated_ends[agent.thread_id])
        for agent in agents
    ]

    agent_ids = {agent.thread_id for agent in agents}
    filtered_events = tuple(
        event for event in normalized_events if event.thread_id in agent_ids
    )
    edges = [
        Edge(
            edge_id=f"orc-spawn-{spawn.thread_id}",
            call_id=f"orc-spawn-{spawn.source_line}",
            from_thread_id=spawn.parent_thread_id,
            to_thread_id=spawn.thread_id,
            kind="spawn",
            timestamp_ms=spawn.timestamp_ms,
            message_text=None,
            content_availability="none",
            encrypted_content=None,
            source_line=spawn.source_line,
        )
        for spawn in sorted(
            all_spawns, key=lambda item: (item.timestamp_ms, item.thread_id)
        )
        if spawn.parent_thread_id in agent_ids
    ]
    edges.extend(
        Edge(
            edge_id=f"orc-message-{event.event_id}",
            call_id=event.event_id,
            from_thread_id=event.thread_id,
            to_thread_id=event.recipient,
            kind="message",
            timestamp_ms=event.timestamp_ms,
            message_text=event.text,
            content_availability=event.content_availability,
            encrypted_content=event.encrypted_content,
            source_line=event.source_line,
        )
        for event in task_events
        if event.kind == "inter_agent_message"
        and event.thread_id in agent_ids
        and event.thread_id != event.recipient
        and event.recipient is not None
        and event.recipient in agent_ids
    )
    source_snapshots = tuple(
        SourceSnapshot(
            path=source.source_path,
            thread_id=source.owner_session_id,
            size_bytes=source.snapshot_size,
            mtime_ns=_snapshot_path(snapshot_root, source.snapshot_path).stat().st_mtime_ns,
            sha256=source.sha256,
            complete_bytes=source.snapshot_size,
            line_count=source.append_count,
            semantic_sha256=source.semantic_sha256,
            semantic_complete_bytes=source.semantic_complete_bytes,
        )
        for source in sorted(source_copies, key=lambda item: item.source_path)
    )
    return TeamData(
        team_slug=team_slug,
        provider="orc",
        root_thread_id=root_session_id,
        display_timezone=display_timezone,
        sources=source_snapshots,
        agents=tuple(sorted(agents, key=lambda item: (item.started_at_ms, item.thread_id))),
        turns=tuple(sorted(turns, key=lambda item: (item.started_at_ms, item.turn_id))),
        events=filtered_events,
        tool_calls=tuple(
            sorted(tools, key=lambda item: (item.started_at_ms, item.call_id))
        ),
        edges=tuple(sorted(edges, key=lambda item: (item.timestamp_ms, item.edge_id))),
    )


__all__ = [
    "OrcParseError",
    "OrcSnapshotResult",
    "OrcSourceCopy",
    "load_orc_team",
    "prune_orc_snapshot_objects",
    "snapshot_orc_lineage",
]
