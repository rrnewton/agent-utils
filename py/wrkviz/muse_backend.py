"""Strict, non-interactive Muse CLI adapter for structured summary work.

wrkviz's default summary backend (#839 wrkviz-muse-backend) runs the headless ``muse exec``
once per backend batch, with the model shaped by ``--output-schema`` and every tool, rule,
reminder and session log turned off. The prompt and the schema are the ones the Claude and Codex
backends receive, so the answers fill the same summary cache entries.
"""

from __future__ import annotations

import json
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path

from wrkviz.archive import JsonValue, as_object, canonical_json, narrow_json
from wrkviz.backend_process import TIMED_OUT_EXIT, BackendProcesses
from wrkviz.token_usage import TokenUsage

#: The model the muse backend uses for phase summaries, agent names and the project overview.
#: Spark 1.4 since #1275 muse-ember-summarizer (it matched or beat 1.3 on every summary kind);
#: roll back by naming "muse-spark-1.3-internal" here, or per run with --model muse-spark-1.3-internal.
DEFAULT_MUSE_MODEL = "muse-spark-1.4-internal"

#: The model the muse backend uses for calendar rollups, which gain most from a larger model.
DEFAULT_MUSE_ROLLUP_MODEL = "watermelon-full-preview"

#: The muse reasoning effort when the run names none.
DEFAULT_MUSE_REASONING_EFFORT = "medium"

#: ``muse exec`` flags that keep one call a pure prompt-to-JSON transformation: no tools, no
#: workspace writes, no web, no reminders, no questions back, no session log on disk, and none
#: of the operator's personal rules or skills.
_ISOLATION_FLAGS: tuple[str, ...] = (
    "--preset",
    "empty",
    "--disable-shell",
    "--disable-write",
    "--disable-web-tools",
    "--disable-reminders",
    "--disable-request-user-input",
    "--no-session-log",
    "--no-foreign-personal-context",
    "--disable-muse-llm-rules",
)


class MuseBackendError(RuntimeError):
    """A Muse invocation failed, retaining any exact usage and the final text."""

    def __init__(
        self,
        message: str,
        *,
        usage: TokenUsage | None = None,
        raw_output: str | None = None,
    ) -> None:
        super().__init__(message)
        self.usage = usage
        self.raw_output = raw_output


@dataclass(frozen=True)
class MuseBackendResult:
    """Validated structured output and accounting from one Muse invocation."""

    output: str
    usage: TokenUsage
    raw_output: str


@dataclass(frozen=True)
class MuseRunEvents:
    """What one ``muse exec --json`` event stream reported."""

    terminal: str | None
    reason: str | None
    text: str | None
    usage: TokenUsage | None


def parse_muse_events(stdout: str) -> MuseRunEvents:
    """Read the terminal state, final text and token usage from ``muse exec --json`` output.

    Each line is one JSON event; its ``payload.kind`` is ``run_terminal`` (with ``terminal``,
    ``reason`` and the final ``text``) or ``run_usage`` (with per-model ``input_tokens``,
    ``cached_tokens``, ``output_tokens`` and ``reasoning_tokens``). Other events and lines that
    are not JSON are ignored. Usage sums every model the run used; ``input_tokens`` already
    counts the cached part.
    """

    terminal: str | None = None
    reason: str | None = None
    text: str | None = None
    usage: TokenUsage | None = None
    for line in stdout.splitlines():
        try:
            event: object = json.loads(line)
        except json.JSONDecodeError:
            continue
        if not isinstance(event, dict):
            continue
        payload = event.get("payload")
        if not isinstance(payload, dict):
            continue
        kind = payload.get("kind")
        if kind == "run_terminal":
            terminal = payload.get("terminal") if isinstance(payload.get("terminal"), str) else None
            reason = payload.get("reason") if isinstance(payload.get("reason"), str) else None
            text = payload.get("text") if isinstance(payload.get("text"), str) else None
        elif kind == "run_usage":
            models = payload.get("models")
            if not isinstance(models, list):
                raise ValueError("muse run_usage event has no models list")
            totals = {"input_tokens": 0, "cached_tokens": 0, "output_tokens": 0, "reasoning_tokens": 0}
            for model in models:
                if not isinstance(model, dict):
                    raise ValueError("muse run_usage model entry is not an object")
                for key in totals:
                    value = model.get(key, 0)
                    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
                        raise ValueError(f"muse run_usage {key} is not a non-negative integer")
                    totals[key] += value
            usage = TokenUsage(
                input_tokens=max(totals["input_tokens"], totals["cached_tokens"]),
                cached_input_tokens=totals["cached_tokens"],
                output_tokens=max(totals["output_tokens"], totals["reasoning_tokens"]),
                reasoning_output_tokens=totals["reasoning_tokens"],
            )
    return MuseRunEvents(terminal=terminal, reason=reason, text=text, usage=usage)


def run_muse_json(
    command: Sequence[str],
    *,
    prompt: str,
    schema: dict[str, JsonValue],
    model: str,
    reasoning_effort: str | None,
    cwd: Path,
    processes: BackendProcesses | None = None,
) -> MuseBackendResult:
    """Run ``muse exec`` once with every tool disabled and require schema-shaped JSON.

    The prompt and the schema go in files under *cwd* (a private temporary directory), so
    neither appears in a process listing. There is deliberately no model or backend fallback:
    a process, terminal-state, schema or accounting failure aborts the batch before any cache
    artifact is published.
    """

    if not command:
        raise MuseBackendError("muse command must not be empty")
    # Keep the complete schema in the summarization layer for validation, as the Claude
    # backend does, and hand the CLI the schema without the dialect declaration.
    cli_schema = {key: value for key, value in schema.items() if key != "$schema"}
    schema_path = cwd / "output-schema.json"
    prompt_path = cwd / "prompt.txt"
    schema_path.write_text(canonical_json(cli_schema), encoding="utf-8")
    prompt_path.write_text(prompt, encoding="utf-8")
    args = [
        *command,
        "exec",
        "--json",
        "--prompt-file",
        str(prompt_path),
        "--output-schema",
        str(schema_path),
        "--model",
        model,
        "--reasoning-effort",
        reasoning_effort or DEFAULT_MUSE_REASONING_EFFORT,
        *_ISOLATION_FLAGS,
    ]
    owned_processes = processes or BackendProcesses()
    try:
        completed = owned_processes.run(args, input_text="", cwd=cwd)
    except OSError as error:
        raise MuseBackendError(f"could not start muse backend: {error}") from error
    try:
        events = parse_muse_events(completed.stdout)
    except ValueError as error:
        raise MuseBackendError(f"muse backend reported invalid usage: {error}") from error
    if completed.returncode != 0:
        detail = " ".join(completed.stderr.split())[-500:]
        what = (
            "was stopped at its time limit"
            if completed.returncode == TIMED_OUT_EXIT
            else f"failed with exit {completed.returncode}"
        )
        raise MuseBackendError(
            f"muse backend {what}" + (f": {detail}" if detail else ""),
            usage=events.usage,
            raw_output=events.text,
        )
    if events.usage is None:
        raise MuseBackendError("muse backend reported no usage", raw_output=events.text)
    if events.terminal != "completed":
        reason = events.reason or events.terminal or "no terminal event"
        raise MuseBackendError(
            f"muse backend run did not complete: {reason}",
            usage=events.usage,
            raw_output=events.text,
        )
    if events.text is None:
        raise MuseBackendError("muse backend produced no final text", usage=events.usage)
    try:
        structured = as_object(
            narrow_json(json.loads(events.text), "Muse final text"), "Muse final text"
        )
    except (json.JSONDecodeError, ValueError) as error:
        raise MuseBackendError(
            f"muse backend returned malformed structured output: {error}",
            usage=events.usage,
            raw_output=events.text,
        ) from error
    # One canonical JSON representation for the strict schema parsers and the audit trail.
    output = canonical_json(structured)
    return MuseBackendResult(output=output, usage=events.usage, raw_output=events.text)


__all__ = [
    "DEFAULT_MUSE_MODEL",
    "DEFAULT_MUSE_REASONING_EFFORT",
    "DEFAULT_MUSE_ROLLUP_MODEL",
    "MuseBackendError",
    "MuseBackendResult",
    "MuseRunEvents",
    "parse_muse_events",
    "run_muse_json",
]
