#!/usr/bin/env python3
"""Measure long spoken reads over the provider-neutral ``vibe-talk-v1`` protocol.

This is the repeatable live acceptance check for #47 read-aloud-overlong.  A run opens a fresh
WebSocket session for every trial, sends the caller's prompt byte-for-byte, and records only
content-free timing and size facts.  Live runs may consume provider time; ``--self-test`` is the
offline deterministic check used by the ordinary validation suite.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import re
import sys
import tempfile
import time
import wave
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Protocol, Sequence, TypeAlias, cast
from urllib.parse import urlsplit

JsonValue: TypeAlias = (
    str | int | float | bool | None | list["JsonValue"] | dict[str, "JsonValue"]
)
JsonObject: TypeAlias = dict[str, JsonValue]

SAMPLE_RATE = 24_000
SAMPLE_WIDTH = 2
CHANNELS = 1
BYTES_PER_SECOND = SAMPLE_RATE * SAMPLE_WIDTH * CHANNELS
DEFAULT_TRIALS = 10
DEFAULT_MAX_GAP_SECONDS = 15.0
DEFAULT_MAX_AUDIO_SECONDS_PER_CHAR = 0.09
DEFAULT_SESSION_TIMEOUT_SECONDS = 15.0
DEFAULT_FIRST_AUDIO_TIMEOUT_SECONDS = 15.0
DEFAULT_TURN_TIMEOUT_SECONDS = 300.0
URL_ENV = "VIBE_TALK_VOICE_WEBSOCKET_URL"

EXIT_OK = 0
EXIT_USAGE = 2
EXIT_ACCEPTANCE = 10
EXIT_DEPENDENCY = 11
EXIT_SELF_TEST = 12


class AcceptanceFailure(Exception):
    """A content-free failure classification suitable for a saved report."""

    def __init__(self, code: str, detail: str) -> None:
        super().__init__(detail)
        self.code = code
        self.detail = detail


class Socket(Protocol):
    """The small part of a websockets client connection this runner uses."""

    async def recv(self) -> str | bytes:
        """Receive one data frame."""

    async def send(self, message: str | bytes) -> None:
        """Send one data frame."""

    async def close(self) -> None:
        """Close the connection."""


def utc_now() -> str:
    """An RFC 3339 UTC timestamp."""
    return datetime.now(timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z")


def tidy_speech(text: str) -> str:
    """Use the page's whitespace rule for one transcript hypothesis."""
    return " ".join(text.split())


def spoken_words(text: str) -> list[str]:
    """Use the page's whole-word approximation for hypothesis reconciliation."""
    cleaned = re.sub(r"[^\w\s']", " ", text.casefold(), flags=re.UNICODE).replace("_", " ")
    return cleaned.split()


def contains_words(haystack: Sequence[str], needle: Sequence[str]) -> bool:
    """Whether ``needle`` occurs contiguously in ``haystack``."""
    if not needle:
        return True
    return any(
        list(haystack[start : start + len(needle)]) == list(needle)
        for start in range(len(haystack) - len(needle) + 1)
    )


def revision_distance(before: Sequence[str], after: Sequence[str]) -> int:
    """Edit distance from all of ``before`` to the closest prefix of ``after``."""
    row = list(range(len(after) + 1))
    for before_index, before_word in enumerate(before, start=1):
        next_row = [before_index]
        for after_index, after_word in enumerate(after, start=1):
            next_row.append(
                min(
                    row[after_index] + 1,
                    next_row[after_index - 1] + 1,
                    row[after_index - 1] + (before_word != after_word),
                )
            )
        row = next_row
    return min(row)


def merge_hypothesis(held: str, frame: str) -> str:
    """Merge a final transcript frame exactly as the browser page does."""
    before = tidy_speech(held)
    after = tidy_speech(frame)
    if not before or not after:
        return before or after
    old_words = spoken_words(before)
    new_words = spoken_words(after)
    if contains_words(new_words, old_words):
        return after
    if contains_words(old_words, new_words):
        return before
    correction_limit = max(1, len(old_words) // 4)
    if (
        len(old_words) >= 3
        and len(new_words) >= 3
        and revision_distance(old_words, new_words) <= correction_limit
    ):
        return after
    return f"{before} {after}"


@dataclass
class TranscriptEntry:
    """The best text held for one protocol ``(turn, role)`` row."""

    role: str
    settled: str = ""
    pending: str = ""

    def text(self) -> str:
        """The row as it would currently appear in the page."""
        return merge_hypothesis(self.settled, self.pending) if self.pending else self.settled


@dataclass
class TranscriptBook:
    """Provider-streaming hypotheses reconciled into final protocol rows."""

    entries: dict[str, TranscriptEntry] = field(default_factory=dict)
    order: list[str] = field(default_factory=list)
    anonymous_turn: int = 0

    def role_texts(self, role: str) -> tuple[str, ...]:
        """Current rows for ``role``, in arrival order."""
        return tuple(
            self.entries[key].text()
            for key in self.order
            if self.entries[key].role == role and self.entries[key].text()
        )

    def role_characters(self, role: str) -> int:
        """Unicode characters supplied by the provider, excluding our row separators."""
        return sum(len(text) for text in self.role_texts(role))

    def apply(self, role: str, turn: int | None, text: str, is_final: bool) -> bool:
        """Apply a frame and say whether that role's visible transcript changed."""
        if not text:
            return False
        before = self.role_texts(role)
        anonymous = turn is None
        key = f"anon:{self.anonymous_turn}:{role}" if anonymous else f"turn:{turn}:{role}"
        entry = self.entries.get(key)
        if entry is None:
            entry = TranscriptEntry(role=role)
            self.entries[key] = entry
            self.order.append(key)
        if is_final:
            entry.settled = merge_hypothesis(entry.settled, text)
            entry.pending = ""
        else:
            entry.pending = text
        changed = self.role_texts(role) != before
        if anonymous and is_final:
            self.anonymous_turn += 1
        return changed


@dataclass
class TrialRecorder:
    """Content-free response facts and metric state for one prompt."""

    save_pcm: bool
    transcript: TranscriptBook = field(default_factory=TranscriptBook)
    events: list[JsonObject] = field(default_factory=list)
    audio_bytes: int = 0
    audio_frames: int = 0
    max_gap_seconds: float = 0.0
    max_wall_gap_seconds: float = 0.0
    _gap_started_at: float | None = None
    _gap_pcm_seconds: float = 0.0
    _pcm: bytearray = field(default_factory=bytearray)

    def record_audio(self, data: bytes, at_seconds: float, at_utc: str) -> None:
        """Record one raw 24 kHz mono s16le frame and advance the gap metric."""
        if not data:
            raise AcceptanceFailure("invalid_pcm", "the provider sent an empty PCM frame")
        if len(data) % SAMPLE_WIDTH != 0:
            raise AcceptanceFailure(
                "invalid_pcm",
                "the provider sent an odd number of bytes for 16-bit PCM",
            )
        duration = len(data) / BYTES_PER_SECOND
        self.audio_bytes += len(data)
        self.audio_frames += 1
        if self.save_pcm:
            self._pcm.extend(data)
        if self._gap_started_at is None:
            self._gap_started_at = at_seconds
            self._gap_pcm_seconds = 0.0
        self._gap_pcm_seconds += duration
        # The acceptance metric is PCM duration accrued without transcript progress: it measures
        # how long audio actually flows and is independent of network batching. Preserve the wall
        # span too, because the timestamped diagnostic distinguishes an upstream stall from a
        # transport burst. A transcript CHANGE resets both; repeats and user recognition do not
        # pretend the assistant advanced.
        wall_span = max(0.0, at_seconds - self._gap_started_at) + duration
        self.max_gap_seconds = max(self.max_gap_seconds, self._gap_pcm_seconds)
        self.max_wall_gap_seconds = max(self.max_wall_gap_seconds, wall_span)
        self.events.append(
            {
                "kind": "audio",
                "at_seconds": round(at_seconds, 6),
                "at_utc": at_utc,
                "bytes": len(data),
                "audio_seconds": round(duration, 6),
            }
        )

    def record_transcript(
        self,
        role: str,
        text: str,
        turn: int | None,
        is_final: bool,
        at_seconds: float,
        at_utc: str,
    ) -> None:
        """Timestamp a transcript change without saving the words."""
        changed = self.transcript.apply(role, turn, text, is_final)
        if not changed:
            return
        if role == "assistant":
            self._gap_started_at = None
            self._gap_pcm_seconds = 0.0
        self.events.append(
            {
                "kind": "transcript_change",
                "at_seconds": round(at_seconds, 6),
                "at_utc": at_utc,
                "role": role,
                "turn": turn,
                "final": is_final,
                "characters": self.transcript.role_characters(role),
            }
        )

    def record_completion(
        self,
        turn: int | None,
        interrupted: bool,
        at_seconds: float,
        at_utc: str,
    ) -> None:
        """Record the response boundary."""
        self.events.append(
            {
                "kind": "turn_complete",
                "at_seconds": round(at_seconds, 6),
                "at_utc": at_utc,
                "turn": turn,
                "interrupted": interrupted,
            }
        )

    @property
    def audio_seconds(self) -> float:
        """Duration encoded by all received PCM."""
        return self.audio_bytes / BYTES_PER_SECOND

    @property
    def assistant_characters(self) -> int:
        """Characters in the reconciled assistant transcript at completion."""
        return self.transcript.role_characters("assistant")

    @property
    def audio_seconds_per_character(self) -> float | None:
        """Encoded audio duration divided by final assistant characters."""
        if self.assistant_characters == 0:
            return None
        return self.audio_seconds / self.assistant_characters

    def write_wav(self, path: Path) -> None:
        """Write the captured PCM as a standard WAV for optional ASR replay."""
        with wave.open(str(path), "wb") as output:
            output.setnchannels(CHANNELS)
            output.setsampwidth(SAMPLE_WIDTH)
            output.setframerate(SAMPLE_RATE)
            output.writeframes(self._pcm)


@dataclass(frozen=True)
class Thresholds:
    """The two quantitative acceptance bounds."""

    max_gap_seconds: float
    max_audio_seconds_per_character: float


@dataclass
class TrialOutcome:
    """One saved trial result."""

    index: int
    started_at: str
    finished_at: str
    elapsed_seconds: float
    greeting: bool | None
    completed: bool
    interrupted: bool
    completion_turn: int | None
    recorder: TrialRecorder
    thresholds: Thresholds
    failure_code: str | None = None
    failure_detail: str | None = None
    wav_file: str | None = None

    def checks(self) -> dict[str, bool]:
        """Every condition required for this trial to pass."""
        ratio = self.recorder.audio_seconds_per_character
        return {
            "completed": self.completed,
            "not_interrupted": not self.interrupted,
            "audio_present": self.recorder.audio_bytes > 0,
            "assistant_transcript_present": self.recorder.assistant_characters > 0,
            "audio_without_transcript_gap": (
                self.recorder.max_gap_seconds <= self.thresholds.max_gap_seconds
            ),
            "audio_seconds_per_transcript_character": (
                ratio is not None
                and ratio <= self.thresholds.max_audio_seconds_per_character
            ),
            "no_protocol_or_transport_failure": self.failure_code is None,
        }

    @property
    def passed(self) -> bool:
        """Whether all acceptance conditions passed."""
        return all(self.checks().values())

    def to_json(self) -> JsonObject:
        """Machine-readable, content-free representation."""
        ratio = self.recorder.audio_seconds_per_character
        checks: JsonObject = {name: value for name, value in self.checks().items()}
        events: list[JsonValue] = [event for event in self.recorder.events]
        return {
            "trial": self.index,
            "status": "pass" if self.passed else "fail",
            "started_at": self.started_at,
            "finished_at": self.finished_at,
            "elapsed_seconds": round(self.elapsed_seconds, 6),
            "session_greeting": self.greeting,
            "completed": self.completed,
            "interrupted": self.interrupted,
            "completion_turn": self.completion_turn,
            "audio_bytes": self.recorder.audio_bytes,
            "audio_frames": self.recorder.audio_frames,
            "audio_seconds": round(self.recorder.audio_seconds, 6),
            "assistant_transcript_characters": self.recorder.assistant_characters,
            "max_audio_without_transcript_seconds": round(
                self.recorder.max_gap_seconds, 6
            ),
            "max_wall_audio_without_transcript_seconds": round(
                self.recorder.max_wall_gap_seconds, 6
            ),
            "audio_seconds_per_transcript_character": (
                round(ratio, 9) if ratio is not None else None
            ),
            "checks": checks,
            "failure_code": self.failure_code,
            "failure_detail": self.failure_detail,
            "wav_file": self.wav_file,
            "events": events,
        }


def json_object(raw: str) -> JsonObject:
    """Parse one protocol control frame as an object."""
    try:
        value = json.loads(raw)
    except json.JSONDecodeError as error:
        raise AcceptanceFailure("invalid_json", "the provider sent invalid JSON") from error
    if not isinstance(value, dict) or not all(isinstance(key, str) for key in value):
        raise AcceptanceFailure("invalid_control_frame", "a control frame was not a JSON object")
    return cast(JsonObject, value)


def string_field(message: JsonObject, name: str) -> str | None:
    """A string field, rejecting incompatible known-frame shapes."""
    value = message.get(name)
    if value is None:
        return None
    if not isinstance(value, str):
        raise AcceptanceFailure("invalid_control_frame", f"{name} was not a string")
    return value


def bool_field(message: JsonObject, name: str, default: bool) -> bool:
    """A boolean field with a protocol default."""
    value = message.get(name)
    if value is None:
        return default
    if not isinstance(value, bool):
        raise AcceptanceFailure("invalid_control_frame", f"{name} was not a boolean")
    return value


def turn_field(message: JsonObject) -> int | None:
    """The optional non-negative protocol turn number."""
    value = message.get("turn")
    if value is None:
        return None
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        raise AcceptanceFailure("invalid_control_frame", "turn was not a non-negative integer")
    return value


async def receive_before(socket: Socket, deadline: float, phase: str) -> str | bytes:
    """Receive one frame before an absolute monotonic deadline."""
    timeout_code = f"{phase.replace(' ', '_')}_timeout"
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        raise AcceptanceFailure(timeout_code, f"timed out while waiting for {phase}")
    try:
        return await asyncio.wait_for(socket.recv(), timeout=remaining)
    except asyncio.TimeoutError as error:
        raise AcceptanceFailure(timeout_code, f"timed out while waiting for {phase}") from error
    except Exception as error:  # noqa: BLE001 - transports expose several version-specific errors
        raise AcceptanceFailure(
            "socket_closed",
            f"the socket ended while waiting for {phase} ({type(error).__name__})",
        ) from error


async def wait_for_session(socket: Socket, timeout_seconds: float) -> bool:
    """Wait for the required first protocol control frame."""
    deadline = time.monotonic() + timeout_seconds
    while True:
        raw = await receive_before(socket, deadline, "session_started")
        if isinstance(raw, bytes):
            raise AcceptanceFailure(
                "audio_before_session",
                "the provider sent audio before session_started",
            )
        message = json_object(raw)
        kind = string_field(message, "type")
        if kind == "session_started":
            return bool_field(message, "greeting", False)
        if kind == "error":
            raise AcceptanceFailure("provider_error", "the provider rejected session startup")
        # Unknown frame types are forward-compatible by contract and therefore ignored.


async def wait_for_boundary(socket: Socket, timeout_seconds: float, phase: str) -> None:
    """Drain one unmeasured turn through its non-interrupted completion."""
    deadline = time.monotonic() + timeout_seconds
    while True:
        raw = await receive_before(socket, deadline, phase)
        if isinstance(raw, bytes):
            continue
        message = json_object(raw)
        kind = string_field(message, "type")
        if kind == "error":
            raise AcceptanceFailure("provider_error", f"the provider failed during {phase}")
        if kind == "turn_complete":
            if bool_field(message, "interrupted", False):
                raise AcceptanceFailure("interrupted_boundary", f"{phase} was interrupted")
            return


async def prepare_session(socket: Socket, timeout_seconds: float) -> bool:
    """Start a session and isolate any promised greeting from the measured prompt.

    The protocol says a voice client begins with ``audio_start`` and withholds microphone frames
    during a greeting. Once that greeting completes, close the empty microphone segment and drain
    its own required completion before typing. Therefore the first boundary after the prompt can
    only belong to the prompt.
    """
    greeting = await wait_for_session(socket, timeout_seconds)
    if not greeting:
        return False
    await socket.send(json.dumps({"type": "audio_start"}))
    await wait_for_boundary(socket, timeout_seconds, "greeting completion")
    await socket.send(json.dumps({"type": "audio_end"}))
    await wait_for_boundary(socket, timeout_seconds, "audio segment closure")
    return True


async def measure_prompt(
    socket: Socket,
    prompt: str,
    recorder: TrialRecorder,
    first_audio_timeout_seconds: float,
    timeout_seconds: float,
) -> tuple[bool, bool, int | None]:
    """Send ``prompt`` byte-for-byte and collect through exactly one response boundary."""
    await socket.send(json.dumps({"type": "prompt", "text": prompt}))
    prompt_started = time.monotonic()
    first_audio_deadline = prompt_started + first_audio_timeout_seconds
    turn_deadline = prompt_started + timeout_seconds
    while True:
        waiting_for_audio = recorder.audio_frames == 0
        deadline = min(first_audio_deadline, turn_deadline) if waiting_for_audio else turn_deadline
        phase = "first audio" if waiting_for_audio else "turn completion"
        raw = await receive_before(socket, deadline, phase)
        received = time.monotonic()
        at_seconds = received - prompt_started
        at_utc = utc_now()
        if isinstance(raw, bytes):
            recorder.record_audio(raw, at_seconds, at_utc)
            continue
        message = json_object(raw)
        kind = string_field(message, "type")
        if kind == "transcript":
            role = string_field(message, "role")
            text = string_field(message, "text")
            if role not in {"user", "assistant"} or text is None:
                raise AcceptanceFailure(
                    "invalid_transcript",
                    "a transcript frame lacked a supported role or text",
                )
            recorder.record_transcript(
                role=role,
                text=text,
                turn=turn_field(message),
                is_final=bool_field(message, "final", True),
                at_seconds=at_seconds,
                at_utc=at_utc,
            )
        elif kind == "turn_complete":
            interrupted = bool_field(message, "interrupted", False)
            completion_turn = turn_field(message)
            recorder.record_completion(
                completion_turn,
                interrupted,
                at_seconds,
                at_utc,
            )
            return True, interrupted, completion_turn
        elif kind == "error":
            raise AcceptanceFailure("provider_error", "the provider returned an error frame")
        # Unknown frame types remain forward-compatible and do not affect the metrics.


async def close_socket(socket: Socket) -> None:
    """Best-effort bounded protocol and WebSocket shutdown."""
    try:
        await asyncio.wait_for(socket.send(json.dumps({"type": "quit"})), timeout=1.0)
    except Exception:  # noqa: BLE001 - the measured result already exists
        pass
    try:
        await asyncio.wait_for(socket.close(), timeout=2.0)
    except Exception:  # noqa: BLE001 - shutdown cannot change the measured result
        pass


async def run_trial(
    index: int,
    url: str,
    prompt: str,
    thresholds: Thresholds,
    session_timeout_seconds: float,
    first_audio_timeout_seconds: float,
    turn_timeout_seconds: float,
    save_wav: bool,
    output_dir: Path,
) -> TrialOutcome:
    """Open one fresh session, send one exact prompt, and measure its response."""
    recorder = TrialRecorder(save_pcm=save_wav)
    started_at = utc_now()
    trial_started = time.monotonic()
    socket: Socket | None = None
    greeting: bool | None = None
    completed = False
    interrupted = False
    completion_turn: int | None = None
    failure_code: str | None = None
    failure_detail: str | None = None
    wav_file: str | None = None
    try:
        try:
            import websockets
        except ImportError as error:
            raise AcceptanceFailure(
                "missing_dependency",
                "the Python websockets package is not installed",
            ) from error
        try:
            opened = await websockets.connect(
                url,
                max_size=None,
                open_timeout=session_timeout_seconds,
                close_timeout=2,
            )
            socket = cast(Socket, opened)
        except Exception as error:  # noqa: BLE001 - connection errors vary across library releases
            raise AcceptanceFailure(
                "connect_failed",
                f"the WebSocket could not be opened ({type(error).__name__})",
            ) from error

        greeting = await prepare_session(socket, session_timeout_seconds)
        completed, interrupted, completion_turn = await measure_prompt(
            socket,
            prompt,
            recorder,
            first_audio_timeout_seconds,
            turn_timeout_seconds,
        )
        if interrupted:
            failure_code = "interrupted"
            failure_detail = "the prompt completed as interrupted"
    except AcceptanceFailure as error:
        failure_code = error.code
        failure_detail = error.detail
    except Exception as error:  # noqa: BLE001 - preserve partial evidence on every unexpected path
        failure_code = "runner_error"
        failure_detail = f"the runner failed ({type(error).__name__})"
    finally:
        if socket is not None:
            await close_socket(socket)

    if save_wav and recorder.audio_bytes:
        wav_name = f"trial-{index:02d}.wav"
        recorder.write_wav(output_dir / wav_name)
        wav_file = wav_name
    return TrialOutcome(
        index=index,
        started_at=started_at,
        finished_at=utc_now(),
        elapsed_seconds=time.monotonic() - trial_started,
        greeting=greeting,
        completed=completed,
        interrupted=interrupted,
        completion_turn=completion_turn,
        recorder=recorder,
        thresholds=thresholds,
        failure_code=failure_code,
        failure_detail=failure_detail,
        wav_file=wav_file,
    )


def summary(outcomes: Sequence[TrialOutcome]) -> JsonObject:
    """Aggregate only observed trials, including a useful running report."""
    gaps = [outcome.recorder.max_gap_seconds for outcome in outcomes]
    wall_gaps = [outcome.recorder.max_wall_gap_seconds for outcome in outcomes]
    ratios = [
        ratio
        for outcome in outcomes
        if (ratio := outcome.recorder.audio_seconds_per_character) is not None
    ]
    return {
        "trials_finished": len(outcomes),
        "trials_passed": sum(outcome.passed for outcome in outcomes),
        "trials_failed": sum(not outcome.passed for outcome in outcomes),
        "max_audio_without_transcript_seconds": round(max(gaps), 6) if gaps else None,
        "max_wall_audio_without_transcript_seconds": (
            round(max(wall_gaps), 6) if wall_gaps else None
        ),
        "max_audio_seconds_per_transcript_character": (
            round(max(ratios), 9) if ratios else None
        ),
        "all_passed": bool(outcomes) and all(outcome.passed for outcome in outcomes),
    }


def build_report(
    status: str,
    run_started_at: str,
    finished_at: str | None,
    prompt: str,
    trial_count: int,
    thresholds: Thresholds,
    session_timeout_seconds: float,
    first_audio_timeout_seconds: float,
    turn_timeout_seconds: float,
    save_wav: bool,
    outcomes: Sequence[TrialOutcome],
) -> JsonObject:
    """Build the durable report without recording the prompt or transcript text."""
    return {
        "schema_version": 1,
        "status": status,
        "started_at": run_started_at,
        "finished_at": finished_at,
        "configuration": {
            "trials": trial_count,
            "prompt_characters": len(prompt),
            "prompt_utf8_bytes": len(prompt.encode("utf-8")),
            "pcm_sample_rate_hz": SAMPLE_RATE,
            "pcm_channels": CHANNELS,
            "pcm_sample_width_bits": SAMPLE_WIDTH * 8,
            "max_audio_without_transcript_seconds": thresholds.max_gap_seconds,
            "max_audio_seconds_per_transcript_character": (
                thresholds.max_audio_seconds_per_character
            ),
            "session_timeout_seconds": session_timeout_seconds,
            "first_audio_timeout_seconds": first_audio_timeout_seconds,
            "turn_timeout_seconds": turn_timeout_seconds,
            "wav_saved": save_wav,
        },
        "summary": summary(outcomes),
        "trials": [outcome.to_json() for outcome in outcomes],
    }


def write_report(path: Path, report: JsonObject) -> None:
    """Replace the report atomically so interruption leaves the last complete trial visible."""
    temporary = path.with_suffix(".json.tmp")
    temporary.write_text(
        json.dumps(report, indent=2, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )
    os.replace(temporary, path)


async def run_acceptance(
    url: str,
    prompt: str,
    trial_count: int,
    thresholds: Thresholds,
    session_timeout_seconds: float,
    first_audio_timeout_seconds: float,
    turn_timeout_seconds: float,
    save_wav: bool,
    output_dir: Path,
) -> tuple[list[TrialOutcome], Path]:
    """Run every requested fresh-session trial, preserving the report after each one."""
    output_dir.mkdir(parents=True, exist_ok=False)
    report_path = output_dir / "results.json"
    run_started_at = utc_now()
    outcomes: list[TrialOutcome] = []
    write_report(
        report_path,
        build_report(
            "running",
            run_started_at,
            None,
            prompt,
            trial_count,
            thresholds,
            session_timeout_seconds,
            first_audio_timeout_seconds,
            turn_timeout_seconds,
            save_wav,
            outcomes,
        ),
    )
    for index in range(1, trial_count + 1):
        outcome = await run_trial(
            index,
            url,
            prompt,
            thresholds,
            session_timeout_seconds,
            first_audio_timeout_seconds,
            turn_timeout_seconds,
            save_wav,
            output_dir,
        )
        outcomes.append(outcome)
        ratio = outcome.recorder.audio_seconds_per_character
        ratio_text = "n/a" if ratio is None else f"{ratio:.4f}"
        print(
            f"trial {index}/{trial_count}: {'PASS' if outcome.passed else 'FAIL'}; "
            f"complete={outcome.completed}; interrupted={outcome.interrupted}; "
            f"audio={outcome.recorder.audio_seconds:.2f}s; "
            f"transcript={outcome.recorder.assistant_characters} chars; "
            f"max-gap={outcome.recorder.max_gap_seconds:.2f}s; ratio={ratio_text}",
            flush=True,
        )
        write_report(
            report_path,
            build_report(
                "running",
                run_started_at,
                None,
                prompt,
                trial_count,
                thresholds,
                session_timeout_seconds,
                first_audio_timeout_seconds,
                turn_timeout_seconds,
                save_wav,
                outcomes,
            ),
        )
    status = "pass" if all(outcome.passed for outcome in outcomes) else "fail"
    write_report(
        report_path,
        build_report(
            status,
            run_started_at,
            utc_now(),
            prompt,
            trial_count,
            thresholds,
            session_timeout_seconds,
            first_audio_timeout_seconds,
            turn_timeout_seconds,
            save_wav,
            outcomes,
        ),
    )
    return outcomes, report_path


def check(condition: bool, detail: str, failures: list[str]) -> None:
    """Collect one self-test failure without hiding later controls."""
    if not condition:
        failures.append(detail)


@dataclass
class ScriptedSocket:
    """In-memory protocol peer for deterministic offline controls."""

    frames: list[str | bytes]
    sent: list[str | bytes] = field(default_factory=list)
    closed: bool = False

    async def recv(self) -> str | bytes:
        """Return the next scripted server frame."""
        if not self.frames:
            raise RuntimeError("scripted socket exhausted")
        return self.frames.pop(0)

    async def send(self, message: str | bytes) -> None:
        """Remember a client frame exactly."""
        self.sent.append(message)

    async def close(self) -> None:
        """Remember closure."""
        self.closed = True


async def protocol_self_test() -> tuple[ScriptedSocket, TrialRecorder, bool, bool, int | None]:
    """Drive greeting isolation and one exact prompt without a network socket."""
    exact_prompt = "Read this exactly:\nCafé — line two.\n"
    socket = ScriptedSocket(
        frames=[
            json.dumps({"type": "session_started", "greeting": True}),
            json.dumps(
                {"type": "transcript", "role": "assistant", "text": "hello", "turn": 1}
            ),
            bytes(BYTES_PER_SECOND // 100),
            json.dumps({"type": "turn_complete", "turn": 1}),
            json.dumps({"type": "turn_complete", "turn": 2}),
            json.dumps(
                {
                    "type": "transcript",
                    "role": "assistant",
                    "text": "measured response",
                    "turn": 3,
                }
            ),
            bytes(BYTES_PER_SECOND),
            json.dumps({"type": "turn_complete", "turn": 3}),
        ]
    )
    greeting = await prepare_session(socket, 1.0)
    recorder = TrialRecorder(save_pcm=False)
    completed, interrupted, turn = await measure_prompt(
        socket,
        exact_prompt,
        recorder,
        1.0,
        1.0,
    )
    return socket, recorder, greeting and completed, interrupted, turn


def self_test() -> int:
    """Deterministic offline controls for transcript reconciliation and both metrics."""
    failures: list[str] = []
    check(merge_hypothesis("one two", "one two three") == "one two three", "cumulative hypothesis", failures)
    check(merge_hypothesis("one two three", "one two") == "one two three", "late repeat", failures)
    check(
        merge_hypothesis("read the old message", "read the new message") == "read the new message",
        "corrected hypothesis",
        failures,
    )
    check(merge_hypothesis("first fragment", "second fragment") == "first fragment second fragment", "fragment append", failures)

    book = TranscriptBook()
    check(book.apply("assistant", 1, "one two", False), "partial hypothesis changed", failures)
    check(not book.apply("assistant", 1, "one two", False), "duplicate partial changed", failures)
    check(book.apply("assistant", 1, "one new two", False), "corrected partial did not change", failures)
    check(book.apply("assistant", 1, "one new two final", True), "final hypothesis did not change", failures)
    first_turn_chars = len("one new two final")
    check(book.role_characters("assistant") == first_turn_chars, "one reconciled turn", failures)
    book.apply("assistant", 2, "one new two final", True)
    check(book.role_characters("assistant") == first_turn_chars * 2, "later repeated turn missing", failures)
    book.apply("assistant", None, "anonymous one", True)
    book.apply("assistant", None, "anonymous two", True)
    check(
        book.role_characters("assistant")
        == first_turn_chars * 2 + len("anonymous one") + len("anonymous two"),
        "anonymous final frames were not separate rows",
        failures,
    )

    recorder = TrialRecorder(save_pcm=True)
    recorder.record_audio(bytes(BYTES_PER_SECOND), 1.0, "t1")
    recorder.record_audio(bytes(BYTES_PER_SECOND), 5.0, "t5")
    recorder.record_transcript("assistant", "read the long message", 1, False, 6.0, "t6")
    recorder.record_audio(bytes(BYTES_PER_SECOND // 2), 8.0, "t8")
    recorder.record_transcript("assistant", "read the long message exactly", 1, True, 9.0, "t9")
    recorder.record_completion(1, False, 9.1, "t9")
    check(abs(recorder.audio_seconds - 2.5) < 1e-9, "PCM duration", failures)
    check(abs(recorder.max_gap_seconds - 2.0) < 1e-9, "PCM transcript gap", failures)
    check(abs(recorder.max_wall_gap_seconds - 5.0) < 1e-9, "wall-clock gap diagnostic", failures)
    check(recorder.assistant_characters == len("read the long message exactly"), "final transcript characters", failures)

    repeats = TrialRecorder(save_pcm=False)
    repeats.record_transcript("assistant", "same words", 1, False, 0.0, "t0")
    repeats.record_audio(bytes(BYTES_PER_SECOND), 1.0, "t1")
    repeats.record_transcript("assistant", "same words", 1, False, 2.0, "t2")
    repeats.record_audio(bytes(BYTES_PER_SECOND), 4.0, "t4")
    check(abs(repeats.max_gap_seconds - 2.0) < 1e-9, "repeat did not preserve gap", failures)
    repeats.record_transcript("user", "unrelated recognition", 1, True, 5.0, "t5")
    repeats.record_audio(bytes(BYTES_PER_SECOND), 6.0, "t6")
    check(abs(repeats.max_gap_seconds - 3.0) < 1e-9, "user transcript did not preserve gap", failures)

    burst = TrialRecorder(save_pcm=False)
    burst.record_audio(bytes(BYTES_PER_SECOND * 16), 0.0, "t0")
    check(abs(burst.max_gap_seconds - 16.0) < 1e-9, "buffered PCM duration", failures)
    idle = TrialRecorder(save_pcm=False)
    idle.record_audio(bytes(BYTES_PER_SECOND), 0.0, "t0")
    idle.record_audio(bytes(BYTES_PER_SECOND), 100.0, "t100")
    check(abs(idle.max_gap_seconds - 2.0) < 1e-9, "wall idle inflated PCM gap", failures)
    check(idle.max_wall_gap_seconds > 100.0, "wall diagnostic lost idle span", failures)

    ratio_at_boundary = recorder.audio_seconds_per_character
    if ratio_at_boundary is None:
        failures.append("ratio missing at threshold boundary")
        ratio_at_boundary = 0.0
    thresholds = Thresholds(
        max_gap_seconds=recorder.max_gap_seconds,
        max_audio_seconds_per_character=ratio_at_boundary,
    )
    passing = TrialOutcome(
        index=1,
        started_at="start",
        finished_at="finish",
        elapsed_seconds=9.1,
        greeting=False,
        completed=True,
        interrupted=False,
        completion_turn=1,
        recorder=recorder,
        thresholds=thresholds,
    )
    check(passing.passed, "inclusive thresholds", failures)
    passing.interrupted = True
    check(not passing.passed, "interrupted completion rejected", failures)
    passing.interrupted = False
    strict = TrialOutcome(
        index=2,
        started_at="start",
        finished_at="finish",
        elapsed_seconds=9.1,
        greeting=False,
        completed=True,
        interrupted=False,
        completion_turn=1,
        recorder=recorder,
        thresholds=Thresholds(
            max_gap_seconds=recorder.max_gap_seconds - 0.001,
            max_audio_seconds_per_character=ratio_at_boundary,
        ),
    )
    check(not strict.passed, "over-threshold gap rejected", failures)
    strict_ratio = TrialOutcome(
        index=3,
        started_at="start",
        finished_at="finish",
        elapsed_seconds=9.1,
        greeting=False,
        completed=True,
        interrupted=False,
        completion_turn=1,
        recorder=recorder,
        thresholds=Thresholds(
            max_gap_seconds=recorder.max_gap_seconds,
            max_audio_seconds_per_character=max(0.0, ratio_at_boundary - 0.000_001),
        ),
    )
    check(not strict_ratio.passed, "over-threshold ratio accepted", failures)

    try:
        TrialRecorder(save_pcm=False).record_audio(b"\0", 0.0, "t0")
        failures.append("odd PCM frame accepted")
    except AcceptanceFailure:
        pass
    try:
        TrialRecorder(save_pcm=False).record_audio(b"", 0.0, "t0")
        failures.append("empty PCM frame accepted")
    except AcceptanceFailure:
        pass

    with tempfile.TemporaryDirectory(prefix="voice-read-acceptance-") as temporary:
        wav_path = Path(temporary) / "capture.wav"
        recorder.write_wav(wav_path)
        with wave.open(str(wav_path), "rb") as captured:
            check(captured.getframerate() == SAMPLE_RATE, "WAV sample rate", failures)
            check(captured.getnchannels() == CHANNELS, "WAV channels", failures)
            check(captured.getsampwidth() == SAMPLE_WIDTH, "WAV sample width", failures)
            check(captured.getnframes() == recorder.audio_bytes // SAMPLE_WIDTH, "WAV frame count", failures)

    socket, measured, completed, interrupted, turn = asyncio.run(protocol_self_test())
    sent = [json_object(frame) for frame in socket.sent if isinstance(frame, str)]
    check(completed and not interrupted and turn == 3, "scripted prompt did not complete", failures)
    check(
        [frame.get("type") for frame in sent] == ["audio_start", "audio_end", "prompt"],
        "greeting and audio boundary were not isolated before the prompt",
        failures,
    )
    check(
        sent[-1].get("text") == "Read this exactly:\nCafé — line two.\n",
        "prompt was not sent exactly",
        failures,
    )
    check(measured.assistant_characters == len("measured response"), "greeting entered metrics", failures)
    check(abs(measured.audio_seconds - 1.0) < 1e-9, "greeting audio entered metrics", failures)

    interrupted_socket = ScriptedSocket(
        frames=[
            json.dumps({"type": "session_started"}),
            json.dumps({"type": "turn_complete", "turn": 1, "interrupted": True}),
        ]
    )
    check(not asyncio.run(prepare_session(interrupted_socket, 1.0)), "false greeting changed", failures)
    interrupted_recorder = TrialRecorder(save_pcm=False)
    _, was_interrupted, _ = asyncio.run(
        measure_prompt(interrupted_socket, "exact", interrupted_recorder, 1.0, 1.0)
    )
    check(was_interrupted, "interrupted protocol completion accepted", failures)

    private_prompt = "private prompt words must not survive"
    privacy_report = build_report(
        "pass",
        "start",
        "finish",
        private_prompt,
        1,
        thresholds,
        1.0,
        1.0,
        1.0,
        False,
        [passing],
    )
    serialized = json.dumps(privacy_report)
    check(private_prompt not in serialized, "report retained prompt text", failures)
    check("read the long message exactly" not in serialized, "report retained transcript text", failures)
    check("sha256" not in serialized.casefold(), "report retained a stable prompt fingerprint", failures)

    if failures:
        print("voice read acceptance self-test FAILED:", file=sys.stderr)
        for failure in failures:
            print(f"  - {failure}", file=sys.stderr)
        return EXIT_SELF_TEST
    print("voice read acceptance self-test: offline controls passed")
    return EXIT_OK


def parser() -> argparse.ArgumentParser:
    """The complete operator interface, including every live default."""
    result = argparse.ArgumentParser(
        description=(
            "Run fresh-session vibe-talk-v1 spoken-read trials and assert transcript/audio timing "
            "without saving prompt or transcript text. Live runs may consume provider time."
        )
    )
    result.add_argument(
        "--url-file",
        type=Path,
        help=(
            f"UTF-8 file containing the ws/wss endpoint (default: ${URL_ENV}); URL values are "
            "never accepted directly on the command line or saved in results"
        ),
    )
    result.add_argument(
        "--prompt-file",
        type=Path,
        help=(
            "UTF-8 file sent exactly, including its final newline when present; required for a "
            "live run so prompt text never enters the process list"
        ),
    )
    result.add_argument(
        "--trials",
        type=int,
        default=DEFAULT_TRIALS,
        help=f"fresh WebSocket sessions to run (default: {DEFAULT_TRIALS})",
    )
    result.add_argument(
        "--max-gap-seconds",
        type=float,
        default=DEFAULT_MAX_GAP_SECONDS,
        help=(
            "maximum PCM audio seconds without an assistant transcript change "
            f"(default: {DEFAULT_MAX_GAP_SECONDS:g})"
        ),
    )
    result.add_argument(
        "--max-audio-seconds-per-character",
        type=float,
        default=DEFAULT_MAX_AUDIO_SECONDS_PER_CHAR,
        help=(
            "maximum PCM seconds divided by final assistant transcript characters "
            f"(default: {DEFAULT_MAX_AUDIO_SECONDS_PER_CHAR:g})"
        ),
    )
    result.add_argument(
        "--session-timeout-seconds",
        type=float,
        default=DEFAULT_SESSION_TIMEOUT_SECONDS,
        help=(
            "connection, session_started, and promised-greeting timeout "
            f"(default: {DEFAULT_SESSION_TIMEOUT_SECONDS:g})"
        ),
    )
    result.add_argument(
        "--first-audio-timeout-seconds",
        type=float,
        default=DEFAULT_FIRST_AUDIO_TIMEOUT_SECONDS,
        help=(
            "maximum wait for a prompt's first binary PCM frame "
            f"(default: {DEFAULT_FIRST_AUDIO_TIMEOUT_SECONDS:g})"
        ),
    )
    result.add_argument(
        "--turn-timeout-seconds",
        type=float,
        default=DEFAULT_TURN_TIMEOUT_SECONDS,
        help=f"maximum wait for one prompt's turn_complete (default: {DEFAULT_TURN_TIMEOUT_SECONDS:g})",
    )
    result.add_argument(
        "--out",
        type=Path,
        help=(
            "new artifact directory (default: debug/voice-read-acceptance/<UTC timestamp>); "
            "results.json is always written"
        ),
    )
    result.add_argument(
        "--wav",
        action="store_true",
        help="also retain each trial's potentially sensitive speech as a WAV for ASR replay",
    )
    result.add_argument(
        "--self-test",
        action="store_true",
        help="run deterministic offline metric and WAV controls; opens no socket",
    )
    return result


def default_output() -> Path:
    """A unique ignored artifact directory inside vibe-talk."""
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    return Path(__file__).resolve().parent.parent / "debug" / "voice-read-acceptance" / stamp


def main(argv: Sequence[str] | None = None) -> int:
    """Validate configuration, run offline controls or drive the live endpoint."""
    args = parser().parse_args(argv)
    if args.self_test:
        return self_test()
    if args.url_file is not None:
        try:
            url = args.url_file.read_text(encoding="utf-8").strip()
        except OSError as error:
            raise AcceptanceFailure(
                "usage",
                f"the URL file could not be read ({type(error).__name__})",
            ) from error
    else:
        url = os.environ.get(URL_ENV, "")
    if not url:
        raise AcceptanceFailure("usage", f"--url-file or {URL_ENV} is required")
    parsed = urlsplit(url)
    if parsed.scheme not in {"ws", "wss"} or not parsed.netloc:
        raise AcceptanceFailure("usage", "the WebSocket URL must be absolute ws:// or wss://")
    if args.prompt_file is None:
        raise AcceptanceFailure("usage", "--prompt-file is required")
    try:
        prompt = args.prompt_file.read_text(encoding="utf-8")
    except OSError as error:
        raise AcceptanceFailure(
            "usage",
            f"the prompt file could not be read ({type(error).__name__})",
        ) from error
    if not prompt.strip():
        raise AcceptanceFailure("usage", "the exact prompt must contain non-whitespace text")
    if args.trials < 1:
        raise AcceptanceFailure("usage", "--trials must be at least 1")
    for name, value in (
        ("--max-gap-seconds", args.max_gap_seconds),
        ("--max-audio-seconds-per-character", args.max_audio_seconds_per_character),
        ("--session-timeout-seconds", args.session_timeout_seconds),
        ("--first-audio-timeout-seconds", args.first_audio_timeout_seconds),
        ("--turn-timeout-seconds", args.turn_timeout_seconds),
    ):
        if not 0 < value < float("inf"):
            raise AcceptanceFailure("usage", f"{name} must be a positive finite number")
    output_dir = args.out or default_output()
    if output_dir.exists():
        raise AcceptanceFailure("usage", f"the artifact directory already exists: {output_dir}")
    thresholds = Thresholds(
        max_gap_seconds=args.max_gap_seconds,
        max_audio_seconds_per_character=args.max_audio_seconds_per_character,
    )
    outcomes, report_path = asyncio.run(
        run_acceptance(
            url=url,
            prompt=prompt,
            trial_count=args.trials,
            thresholds=thresholds,
            session_timeout_seconds=args.session_timeout_seconds,
            first_audio_timeout_seconds=args.first_audio_timeout_seconds,
            turn_timeout_seconds=args.turn_timeout_seconds,
            save_wav=args.wav,
            output_dir=output_dir,
        )
    )
    print(f"results: {report_path.resolve()}")
    if all(outcome.passed for outcome in outcomes):
        print(f"PASS: {len(outcomes)} fresh-session reads met every threshold")
        return EXIT_OK
    print(
        f"FAIL: {sum(not outcome.passed for outcome in outcomes)} of {len(outcomes)} trials failed",
        file=sys.stderr,
    )
    return EXIT_ACCEPTANCE


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except AcceptanceFailure as error:
        print(f"voice-read-acceptance: {error.detail}", file=sys.stderr)
        raise SystemExit(EXIT_DEPENDENCY if error.code == "missing_dependency" else EXIT_USAGE)
