"""A constant-sized, MERGEABLE profile SUMMARY that closes the profiling feedback loop on
EPHEMERAL CI.

The problem this solves
-----------------------
The profile store (:mod:`safe_ci_dag_runner.perflog`) auto-logs per-step resource-usage CSVs, and
the planner (:mod:`safe_ci_dag_runner.estimates`) reads them back to refine each step's duration /
memory / speedup estimates. That feedback loop is INERT on ephemeral CI: a fresh GitHub runner
starts with an EMPTY store every run, writes its own CSV, uploads it, and nothing ever downloads
the accumulated history back — so every run re-learns from zero.

This module is the piece that closes the loop: a bounded, mergeable SUMMARY that a pluggable
backend (:mod:`safe_ci_dag_runner.sync`) can UPLOAD at end-of-run and DOWNLOAD at start-of-run, so
the planner is seeded with the whole fleet's history instead of a blank store.

The design
----------
* **Bounded, not unbounded raw CSVs.** For each ``(step, inner_jobs)`` bucket the summary keeps a
  RESERVOIR of up to :data:`DEFAULT_RESERVOIR_K` :class:`~safe_ci_dag_runner.estimates.Sample`
  records — exactly the fields the estimator + speedup model consume (raw wall + contention, total
  CPU-seconds, effective cores, throttled seconds, peak bytes). The number of buckets is bounded by
  the workload structure (distinct steps × widths), and is additionally hard-capped at
  :data:`DEFAULT_MAX_BUCKETS`, so a summary is genuinely CONSTANT-SIZED: merging N runs never grows
  it past ``MAX_BUCKETS × RESERVOIR_K`` samples, independent of N.
* **Deterministic, order-independent MERGE.** ``merge(a, b)`` unions the two summaries' reservoirs
  per bucket and subsamples back to K by a CONTENT-derived stable order (sort by an FNV-1a hash of
  each sample's canonical serialization, then take the first K) — NOT an RNG. That makes merge
  **commutative and associative** and byte-identical across the Python and Rust builds, so two
  runners merging the same contributions in any order reach the same summary.
* **Recompute estimates FROM the reservoirs on read.** The planner's estimates are recomputed from
  the summary's samples via the SAME estimator core the CSV reader uses
  (:func:`~safe_ci_dag_runner.estimates.step_samples_from_buckets` /
  :func:`~safe_ci_dag_runner.estimates.step_speedups_from_buckets`), so a summary that has not yet
  subsampled a bucket yields byte-identical estimates to reading the raw rows.

Cross-language parity (the correctness core): the canonical JSON serialization and the merge are
byte-identical between the Python and Rust builds — every float is emitted as a fixed 3-decimal
STRING (like the plan JSON) so parity never depends on float ``repr``, and the subsample hash is a
hand-rolled FNV-1a over that canonical form. This is asserted by ``cross/differential.py``.
"""

from __future__ import annotations

import json
import math
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from typing import NoReturn

from safe_ci_dag_runner.estimates import (
    BucketKey,
    Sample,
    StepSamples,
    StepSpeedup,
    _affinity_width,
    _fmt_secs,
    _json_str,
    bucketize_rows,
    step_samples_from_buckets,
    step_speedups_from_buckets,
)

__all__ = [
    "SUMMARY_VERSION",
    "DEFAULT_RESERVOIR_K",
    "DEFAULT_MAX_BUCKETS",
    "SummaryError",
    "Summary",
    "empty",
    "summary_from_rows",
    "merge",
    "merge_all",
    "to_json",
    "from_json",
    "step_samples_from_summary",
    "step_speedups_from_summary",
    "summary_stats",
]

#: On-disk schema version. Bumped only on an incompatible sample/bucket shape change; a reader
#: refuses a version it does not understand (No Silent Failure) rather than mis-parsing it.
SUMMARY_VERSION = 1

#: Default reservoir size K: the maximum number of samples kept per ``(step, inner_jobs)`` bucket.
#: A merge unions two buckets then subsamples back to K, so a bucket never exceeds K regardless of
#: how many runs contributed. 64 keeps the robust median / p90 / speedup curve statistically stable
#: while bounding the summary at a few KB per bucket.
DEFAULT_RESERVOIR_K = 64

#: Hard cap on the number of buckets, so a pathological caller inventing unbounded step names cannot
#: make the summary grow without bound. Real workloads have far fewer buckets (steps × widths); this
#: only ever bites synthetic abuse. Buckets over the cap are dropped by the same content-derived
#: stable order used for sample subsampling, so the surviving set is deterministic across builds.
DEFAULT_MAX_BUCKETS = 4096

#: FNV-1a (64-bit) constants — a tiny, dependency-free hash implemented identically in both builds,
#: so the content-derived subsample order is byte-identical py<->rs.
_FNV_OFFSET_BASIS = 0xCBF29CE484222325
_FNV_PRIME = 0x100000001B3
_U64_MASK = (1 << 64) - 1


class SummaryError(Exception):
    """A summary document was malformed, carried an unknown version, or had a mismatched identity."""


@dataclass(frozen=True)
class Summary:
    """A bounded, mergeable profile summary for ONE ``(machine_id, container_class)`` identity.

    Mirrors the per-machine + per-container scoping of the CSV store (one ``step_profiles_<machine>_
    <container>.csv`` file per identity): a summary carries the identity so merges are only ever
    applied within a homogeneous identity, and the speedup core budget is recovered from the
    ``container_class`` exactly as :func:`~safe_ci_dag_runner.estimates.load_step_speedups` does.

    ``buckets`` maps each ``(step, inner_jobs)`` bucket to its reservoir (a tuple of up to K
    :class:`~safe_ci_dag_runner.estimates.Sample`). The tuple order is not significant — every
    consumer that cares (serialization, merge) re-derives the canonical content order — but a
    freshly built / merged summary already stores each reservoir in that canonical order.
    """

    version: int
    machine_id: str
    container_class: str
    buckets: Mapping[BucketKey, tuple[Sample, ...]]


# --------------------------------------------------------------------------- content hashing


def _fnv1a_64(data: bytes) -> int:
    """FNV-1a 64-bit hash of ``data`` (wrapping 64-bit arithmetic), identical to the Rust build."""
    h = _FNV_OFFSET_BASIS
    for byte in data:
        h = ((h ^ byte) * _FNV_PRIME) & _U64_MASK
    return h


def _opt_secs_json(value: float | None) -> str:
    """A JSON value for an optional seconds-like float: ``null`` or a quoted fixed-3-decimal string,
    matching the plan JSON's float discipline (parity never depends on float ``repr``)."""
    return "null" if value is None else f'"{_fmt_secs(value)}"'


def _sample_canonical(sample: Sample) -> str:
    """The canonical one-line JSON object for a sample — used BOTH as the serialized form and as the
    input to the subsample hash, so the two can never drift. Every float is a fixed 3-decimal
    string; ``contention`` is always present; the rest are ``null`` when absent."""
    return (
        "{"
        f'"elapsed_s": {_opt_secs_json(sample.elapsed_s)}, '
        f'"contention": "{_fmt_secs(sample.contention)}", '
        f'"cpu_s": {_opt_secs_json(sample.cpu_s)}, '
        f'"effective_cores": {_opt_secs_json(sample.effective_cores)}, '
        f'"throttled_s": {_opt_secs_json(sample.throttled_s)}, '
        f'"peak_bytes": {"null" if sample.peak_bytes is None else str(sample.peak_bytes)}'
        "}"
    )


def _sample_sort_key(sample: Sample) -> tuple[int, str]:
    """The stable, content-derived sort key ``(fnv_hash, canonical_json)`` a reservoir is ordered /
    subsampled by. The canonical JSON is pure ASCII (digits, ``.``, ``-``, structural chars, and
    ``null``), so Python's code-point string order equals Rust's UTF-8 byte order — the tie-break is
    identical across builds."""
    canon = _sample_canonical(sample)
    return (_fnv1a_64(canon.encode("utf-8")), canon)


def _ordered(samples: Sequence[Sample], cap: int | None) -> tuple[Sample, ...]:
    """Return ``samples`` in canonical content order, optionally truncated to the first ``cap``.

    Sorting by :func:`_sample_sort_key` is the deterministic subsample: the smallest-``cap`` samples
    by the content hash. Because the order is a fixed total order on sample CONTENT, taking the
    first ``cap`` of a union is commutative and associative — the mergeable-summary property."""
    ordered = sorted(samples, key=_sample_sort_key)
    if cap is not None and len(ordered) > cap:
        ordered = ordered[:cap]
    return tuple(ordered)


def _bucket_sort_key(key: BucketKey) -> tuple[int, str]:
    """The content-derived stable order buckets are dropped by when over :data:`DEFAULT_MAX_BUCKETS`
    (same FNV-1a scheme as samples, over a canonical ``step:inner`` rendering)."""
    canon = f"{_json_str(key[0])}:{key[1]}"
    return (_fnv1a_64(canon.encode("utf-8")), canon)


def _cap_buckets(
    buckets: Mapping[BucketKey, tuple[Sample, ...]], max_buckets: int
) -> dict[BucketKey, tuple[Sample, ...]]:
    """Drop buckets beyond ``max_buckets`` by the content-derived stable order, so the surviving set
    is deterministic across builds. A no-op for a normal workload (far fewer buckets than the cap)."""
    if len(buckets) <= max_buckets:
        return dict(buckets)
    kept = sorted(buckets, key=_bucket_sort_key)[:max_buckets]
    return {key: buckets[key] for key in kept}


# --------------------------------------------------------------------------- construction / merge


def empty(machine_id: str, container_class: str) -> Summary:
    """An empty summary for ``(machine_id, container_class)`` — what a backend returns when no
    summary has been published yet (the planner then falls back to DAG hints)."""
    return Summary(
        version=SUMMARY_VERSION,
        machine_id=machine_id,
        container_class=container_class,
        buckets={},
    )


def summary_from_rows(
    rows: Sequence[Mapping[str, str]],
    machine_id: str,
    container_class: str,
    affinity_width: int | None,
    *,
    reservoir_cap: int = DEFAULT_RESERVOIR_K,
    max_buckets: int = DEFAULT_MAX_BUCKETS,
) -> Summary:
    """Build a bounded summary from raw profile ``rows`` (a CSV store's rows, or this run's per-step
    profile rows stringified). Each ``(step, inner_jobs)`` reservoir is subsampled to
    ``reservoir_cap``; the whole summary is capped at ``max_buckets`` buckets."""
    raw = bucketize_rows(rows, affinity_width)
    capped = {key: _ordered(samples, reservoir_cap) for key, samples in raw.items()}
    return Summary(
        version=SUMMARY_VERSION,
        machine_id=machine_id,
        container_class=container_class,
        buckets=_cap_buckets(capped, max_buckets),
    )


def merge(
    a: Summary,
    b: Summary,
    *,
    reservoir_cap: int = DEFAULT_RESERVOIR_K,
    max_buckets: int = DEFAULT_MAX_BUCKETS,
) -> Summary:
    """Merge two summaries of the SAME identity: union each bucket's reservoirs and subsample back to
    ``reservoir_cap`` by the content-derived stable order.

    Deterministic, COMMUTATIVE, and ASSOCIATIVE: because the subsample keeps the smallest-K samples
    by a fixed total order on content, ``merge(merge(a, b), c) == merge(a, merge(b, c)) ==
    merge`` of any permutation. A summary merged with an empty summary of the same identity is
    unchanged (up to reservoir order). Raises :class:`SummaryError` on an identity mismatch — the
    caller must never merge a different machine's / container's history into this one."""
    if a.machine_id != b.machine_id or a.container_class != b.container_class:
        raise SummaryError(
            f"cannot merge summaries of different identities: "
            f"{a.machine_id}/{a.container_class} vs {b.machine_id}/{b.container_class}"
        )
    merged: dict[BucketKey, tuple[Sample, ...]] = {}
    for key in set(a.buckets) | set(b.buckets):
        combined = list(a.buckets.get(key, ())) + list(b.buckets.get(key, ()))
        merged[key] = _ordered(combined, reservoir_cap)
    return Summary(
        version=SUMMARY_VERSION,
        machine_id=a.machine_id,
        container_class=a.container_class,
        buckets=_cap_buckets(merged, max_buckets),
    )


def merge_all(
    summaries: Sequence[Summary],
    machine_id: str,
    container_class: str,
    *,
    reservoir_cap: int = DEFAULT_RESERVOIR_K,
    max_buckets: int = DEFAULT_MAX_BUCKETS,
) -> Summary:
    """Fold-merge a sequence of same-identity summaries, starting from an empty summary of
    ``(machine_id, container_class)``. Order-independent (merge is associative + commutative)."""
    acc = empty(machine_id, container_class)
    for summary in summaries:
        acc = merge(acc, summary, reservoir_cap=reservoir_cap, max_buckets=max_buckets)
    return acc


# --------------------------------------------------------------------------- serialization


def to_json(summary: Summary) -> str:
    """Canonical, byte-identical (py<->rs) JSON for a summary (2-space indent).

    Buckets are sorted by ``(step, inner_jobs)``; each reservoir's samples are emitted in the
    canonical content order (:func:`_ordered`). Floats are fixed 3-decimal strings, so the output
    depends only on the shared fixed-precision formatting — never on float ``repr``."""
    lines: list[str] = [
        "{",
        f'  "version": {summary.version},',
        f'  "machine_id": {_json_str(summary.machine_id)},',
        f'  "container_class": {_json_str(summary.container_class)},',
    ]
    keys = sorted(summary.buckets)
    if not keys:
        lines.append('  "buckets": []')
        lines.append("}")
        return "\n".join(lines)
    lines.append('  "buckets": [')
    bucket_blocks: list[str] = []
    for step, inner in keys:
        samples = _ordered(summary.buckets[(step, inner)], None)
        block = [
            "    {",
            f'      "step": {_json_str(step)},',
            f'      "inner_jobs": {inner},',
        ]
        if not samples:
            block.append('      "samples": []')
        else:
            block.append('      "samples": [')
            sample_lines = [f"        {_sample_canonical(s)}" for s in samples]
            block.append(",\n".join(sample_lines))
            block.append("      ]")
        block.append("    }")
        bucket_blocks.append("\n".join(block))
    lines.append(",\n".join(bucket_blocks))
    lines.append("  ]")
    lines.append("}")
    return "\n".join(lines)


def _reject_json_constant(token: str) -> NoReturn:
    """Reject the JSON non-finite literals (``NaN`` / ``Infinity`` / ``-Infinity``) Python's json
    accepts by default — the Rust build's serde_json rejects them, so both must (parity)."""
    raise SummaryError(f"invalid summary: non-finite JSON constant {token!r}")


def _as_obj(value: object, where: str) -> dict[str, object]:
    if not isinstance(value, dict):
        raise SummaryError(f"invalid summary: {where} must be an object")
    return {str(k): v for k, v in value.items()}


def _req_str(m: Mapping[str, object], key: str, where: str) -> str:
    value = m.get(key)
    if not isinstance(value, str):
        raise SummaryError(f"invalid summary: {where}.{key} must be a string")
    return value


def _opt_secs(m: Mapping[str, object], key: str, where: str) -> float | None:
    """Parse an optional fixed-decimal seconds STRING (or ``null``) back to a float. The serialized
    form is always a quoted string or ``null``; anything else is a malformed document."""
    value = m.get(key, None)
    if value is None:
        return None
    if not isinstance(value, str):
        raise SummaryError(f"invalid summary: {where}.{key} must be a decimal string or null")
    try:
        parsed = float(value)
    except ValueError as exc:
        raise SummaryError(f"invalid summary: {where}.{key} is not a number ({value!r})") from exc
    # Reject a non-finite string ("nan"/"inf") too — our serializer never emits one, and both builds
    # must reject the same set so a hand-crafted document parses (or fails) identically.
    if not math.isfinite(parsed):
        raise SummaryError(f"invalid summary: {where}.{key} is not finite ({value!r})")
    return parsed


def _opt_int(m: Mapping[str, object], key: str, where: str) -> int | None:
    value = m.get(key, None)
    if value is None:
        return None
    # A JSON integer parses to ``int``; reject bool (a subclass of int) and floats.
    if isinstance(value, bool) or not isinstance(value, int):
        raise SummaryError(f"invalid summary: {where}.{key} must be an integer or null")
    return value


def _req_int(m: Mapping[str, object], key: str, where: str) -> int:
    value = m.get(key)
    if isinstance(value, bool) or not isinstance(value, int):
        raise SummaryError(f"invalid summary: {where}.{key} must be an integer")
    return value


def _sample_from_obj(obj: object, where: str) -> Sample:
    m = _as_obj(obj, where)
    return Sample(
        elapsed_s=_opt_secs(m, "elapsed_s", where),
        contention=_opt_secs(m, "contention", where) or 0.0,
        cpu_s=_opt_secs(m, "cpu_s", where),
        effective_cores=_opt_secs(m, "effective_cores", where),
        throttled_s=_opt_secs(m, "throttled_s", where),
        peak_bytes=_opt_int(m, "peak_bytes", where),
    )


def from_json(text: str) -> Summary:
    """Parse a canonical summary document, narrowing strictly (no ``Any`` leaks, matching the Rust
    reader) and rejecting an unknown ``version`` or a malformed shape (:class:`SummaryError`)."""
    try:
        raw: object = json.loads(text, parse_constant=_reject_json_constant)
    except json.JSONDecodeError as exc:
        raise SummaryError(f"invalid summary JSON: {exc}") from exc
    doc = _as_obj(raw, "summary")
    version = _req_int(doc, "version", "summary")
    if version != SUMMARY_VERSION:
        raise SummaryError(
            f"unsupported summary version {version} (this build understands {SUMMARY_VERSION})"
        )
    machine_id = _req_str(doc, "machine_id", "summary")
    container_class = _req_str(doc, "container_class", "summary")
    buckets_raw = doc.get("buckets", [])
    if not isinstance(buckets_raw, list):
        raise SummaryError("invalid summary: buckets must be a list")
    buckets: dict[BucketKey, tuple[Sample, ...]] = {}
    for i, bucket_obj in enumerate(buckets_raw):
        where = f"buckets[{i}]"
        bucket = _as_obj(bucket_obj, where)
        step = _req_str(bucket, "step", where)
        inner = _req_int(bucket, "inner_jobs", where)
        samples_raw = bucket.get("samples", [])
        if not isinstance(samples_raw, list):
            raise SummaryError(f"invalid summary: {where}.samples must be a list")
        samples = tuple(
            _sample_from_obj(s, f"{where}.samples[{j}]") for j, s in enumerate(samples_raw)
        )
        buckets[(step, inner)] = samples
    return Summary(
        version=version,
        machine_id=machine_id,
        container_class=container_class,
        buckets=buckets,
    )


# --------------------------------------------------------------------------- estimate recompute


def step_samples_from_summary(summary: Summary) -> dict[str, StepSamples]:
    """Recompute the per-step duration + memory estimates from a summary's reservoirs, via the SAME
    estimator core the CSV reader uses (:func:`~safe_ci_dag_runner.estimates.step_samples_from_buckets`)."""
    return step_samples_from_buckets(summary.buckets)


def step_speedups_from_summary(
    summary: Summary, core_budget: int | None = None
) -> dict[str, StepSpeedup]:
    """Recompute the per-step parallel-speedup curves from a summary's reservoirs. ``core_budget``
    defaults to the affinity width parsed from the summary's ``container_class`` — exactly what
    :func:`~safe_ci_dag_runner.estimates.load_step_speedups` uses — so a summary reproduces the CSV
    reader's recommendations."""
    budget = core_budget if core_budget is not None else _affinity_width(summary.container_class)
    return step_speedups_from_buckets(summary.buckets, budget)


def summary_stats(summary: Summary) -> tuple[int, int, int]:
    """``(bucket_count, total_samples, max_bucket_samples)`` — the bounded-size witness. ``merge``
    keeps ``max_bucket_samples <= reservoir_cap`` and ``bucket_count <= max_buckets`` no matter how
    many runs contributed."""
    bucket_count = len(summary.buckets)
    total = sum(len(s) for s in summary.buckets.values())
    largest = max((len(s) for s in summary.buckets.values()), default=0)
    return bucket_count, total, largest
