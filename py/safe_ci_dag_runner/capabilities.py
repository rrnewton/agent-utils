"""Enforcement-capability registry: the SINGLE SOURCE OF TRUTH for the manifest.

The ``capabilities`` subcommand does NOT print a hand-maintained JSON literal. It serializes this
registry (:func:`enforcement_manifest`), and every enforcement guard in the runner consults
:func:`is_enforced` at its guard site (``cpu_timeout`` reap, ``wall_timeout`` wait deadline,
``oom_detection`` event read, inner ``memory_max`` write, ``solo_validate`` admission), so the
ADVERTISED manifest and the CODE that enforces it read the SAME source and cannot silently diverge.
This is the recurrence guard for the historical gap where the Rust runner silently did NOT enforce
``cpu_timeout`` while the Python runner did.

The manifest is derived, not declared: flip a :class:`Capability`'s ``enforced`` flag and BOTH the
emitted manifest AND the guarded behavior change together (a guard wrapped in ``is_enforced(key)``
becomes inert when its capability is flagged off). The py-vs-rs differential asserts the two
engines' serialized manifests are byte-identical and reports N = :func:`capability_count`.

MUST stay behaviorally identical to ``rs/safe-ci-dag-runner/src/capabilities.rs``.
"""

from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True)
class Capability:
    """One enforcement guard the engine advertises. ``enforced`` is the truth the guard site reads."""

    key: str
    enforced: bool
    summary: str


#: The enumerated enforcement guards, in key order. This is the source of truth the manifest is
#: generated from and that every guard site consults; it is NOT a description of a literal kept
#: elsewhere.
#:   cpu_timeout    per-step user+system CPU budget (cgroup cpu.stat), reaped over budget
#:   memory_max     per-step inner memory.max cap (kernel OOM-kills the step at its cap)
#:   oom_detection  failure attributed to OOM via cgroup memory.events oom_kill count
#:   pids_guard     per-step PID/thread ceiling (plumbed in both, enforced in neither -> false)
#:   solo_validate  SOLO-VALIDATE box exclusivity: a validate node is refused admission while
#:                  another validate OR a benchmark harness holds the box (see admission.py)
#:   wall_timeout   per-step wall-clock ceiling (load-dependent; active with or without boxing)
ENFORCEMENT_REGISTRY: tuple[Capability, ...] = (
    Capability(
        "cpu_timeout", True,
        "per-step user+system CPU budget (cgroup cpu.stat), reaped over budget",
    ),
    Capability(
        "memory_max", True,
        "per-step inner memory.max cap (kernel OOM-kills the step at its cap)",
    ),
    Capability(
        "oom_detection", True,
        "failure attributed to OOM via cgroup memory.events oom_kill count",
    ),
    Capability(
        "pids_guard", False,
        "per-step PID/thread ceiling (plumbed in both, enforced in neither)",
    ),
    Capability(
        "solo_validate", True,
        "SOLO-VALIDATE box exclusivity: a validate node is refused admission while another "
        "validate or a benchmark harness holds the box",
    ),
    Capability(
        "wall_timeout", True,
        "per-step wall-clock ceiling (load-dependent; active with or without boxing)",
    ),
)

_BY_KEY: dict[str, Capability] = {c.key: c for c in ENFORCEMENT_REGISTRY}


def enforcement_manifest() -> str:
    """Serialize the registry to the compact, key-sorted JSON the ``capabilities`` subcommand emits.

    Byte-identical across engines by construction: keys sorted, no whitespace, lowercase booleans.
    """
    parts = [
        f'"{c.key}":{"true" if c.enforced else "false"}'
        for c in sorted(ENFORCEMENT_REGISTRY, key=lambda c: c.key)
    ]
    return "{" + ",".join(parts) + "}"


def is_enforced(key: str) -> bool:
    """Whether guard ``key`` is actively enforced.

    Raises :class:`KeyError` for an unknown key so a typo at a guard site fails LOUDLY rather than
    silently disabling enforcement.
    """
    return _BY_KEY[key].enforced


def capability_count() -> int:
    """N: the number of declared enforcement capabilities (reported by the differential)."""
    return len(ENFORCEMENT_REGISTRY)
