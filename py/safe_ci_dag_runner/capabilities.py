"""The enforcement manifest, derived from the guards that implement it.

The ``capabilities`` subcommand advertises which containment guards this engine really
applies. That manifest used to be a hand-typed JSON string literal, which meant it could
claim enforcement that no longer happened: nothing tied the literal to a guard site, so
deleting or short-circuiting a guard left the advertisement intact. The py-vs-rs
differential compares the two manifests byte-for-byte, which stops the two ENGINES from
drifting apart but cannot notice both being wrong together, nor a manifest drifting from
the guard sites inside its own engine.

So the manifest is generated here instead. :data:`ENFORCEMENT_REGISTRY` is the single
declaration; :func:`enforcement_manifest` serializes it key-sorted, compact and with
lowercase booleans, so the two editions agree BY CONSTRUCTION rather than because two
people kept two literals in step; and :func:`is_enforced` is consulted at the guard site
itself, so flipping one ``enforced`` flag moves the advertisement AND the behaviour
together.

:func:`is_enforced` raises on an unknown key. A guard site that misspells its capability
therefore fails loudly at the moment it runs, rather than reading as "not enforced" and
silently switching the guard off — which is the exact failure this module exists to
prevent.

WHICH KEYS ACTUALLY GATE SOMETHING. Five of the nine are consulted at the code that
enforces them, and their flags are load-bearing:

    ==============  ================================================================
    key             guard site that consults ``is_enforced``
    ==============  ================================================================
    cpu_timeout     the scheduler's 1 Hz ``cpu.stat`` monitor, which reaps over budget
    wall_timeout    the scheduler's per-step wait deadline
    oom_detection   the post-step ``memory.events`` ``oom_kill`` read
    memory_max      the per-step inner ``memory.max`` write in the cgroup manager
    pids_guard      the per-step inner ``pids.max`` write in the cgroup manager
    ==============  ================================================================

The remaining four -- ``cpu_affinity``, ``cpu_bandwidth``, ``run_timeout`` and
``write_domains`` -- are declarations only: the registry records them and the manifest
publishes them, but no code consults their flag, so flipping one changes the
advertisement WITHOUT changing behaviour. Wiring those is a further step, and saying so
here is better than implying a coverage that does not exist.
"""

from __future__ import annotations

import json
from collections.abc import Iterator
from contextlib import contextmanager
from dataclasses import dataclass

__all__ = [
    "Capability",
    "ENFORCEMENT_REGISTRY",
    "enforcement_manifest",
    "is_enforced",
    "registry_override",
]


@dataclass(frozen=True)
class Capability:
    """One advertised containment guard.

    ``key`` is the manifest key (and what a guard site passes to :func:`is_enforced`);
    ``enforced`` is whether this engine really applies it; ``summary`` is the human
    sentence that used to live in the comment above the string literal.
    """

    key: str
    enforced: bool
    summary: str


#: Every enforcement guard this engine advertises, and whether it is real. This is the
#: ONLY place the answer is written down: the manifest is generated from it and the guard
#: sites listed in the module docstring consult it. The cgroup-dependent guards take
#: effect only under boxing; the boxed smoke tests in each build anchor these declarations
#: to real behaviour wherever a cgroup-v2 + systemd --user scope exists.
ENFORCEMENT_REGISTRY: tuple[Capability, ...] = (
    Capability(
        key="cpu_affinity",
        enforced=True,
        summary=(
            "opt-in --cores K: constrain the WHOLE run tree to K least-busy free cores "
            "with an exact, verified cgroup cpuset; refuse when unavailable"
        ),
    ),
    Capability(
        key="cpu_bandwidth",
        enforced=True,
        summary=(
            "boxed run: exact outer cpu.max = --max-cpus x period, read back before execution"
        ),
    ),
    Capability(
        key="cpu_timeout",
        enforced=True,
        summary="per-step user+system CPU budget (cgroup cpu.stat), reaped over budget",
    ),
    Capability(
        key="memory_max",
        enforced=True,
        summary="per-step inner memory.max cap (kernel OOM-kills the step at its cap)",
    ),
    Capability(
        key="oom_detection",
        enforced=True,
        summary="failure attributed to OOM via cgroup memory.events oom_kill count",
    ),
    Capability(
        key="pids_guard",
        enforced=False,
        summary=(
            "per-step PID/thread ceiling: the cgroup manager can write pids.max, but no "
            "caller sets the limit and the companion native engine has no pids plumbing "
            "at all, so the write is gated off here and nothing applies a PID ceiling"
        ),
    ),
    Capability(
        key="run_timeout",
        enforced=True,
        summary=(
            "OUTER wall budget for the WHOLE run: the scheduler cuts in-flight steps and "
            "still reports (works boxed or unboxed); under boxing it is additionally backed "
            "by the scope's systemd RuntimeMaxSec, set strictly later so the reporting "
            "bound fires first"
        ),
    ),
    Capability(
        key="wall_timeout",
        enforced=True,
        summary="per-step wall-clock ceiling (load-dependent; active with or without boxing)",
    ),
    Capability(
        key="write_domains",
        enforced=True,
        summary=(
            "pre-execution closed-vocabulary declaration guard; omission/unknown/duplicate "
            "domains refuse before any node starts when the DAG opts in"
        ),
    ),
)

#: Bracket support only (see :func:`registry_override`); ``None`` means "use the real one".
_OVERRIDE: tuple[Capability, ...] | None = None


def _active() -> tuple[Capability, ...]:
    return ENFORCEMENT_REGISTRY if _OVERRIDE is None else _OVERRIDE


@contextmanager
def registry_override(key: str, enforced: bool) -> Iterator[None]:
    """Temporarily flip one capability's ``enforced`` flag. **Brackets only.**

    This exists so a test can prove the coupling the module claims: flip one flag and
    assert that BOTH the published manifest and the guarded behaviour move. A test that
    only re-derived the manifest from a registry it had just built itself would be
    tautological; a test that only checked behaviour would not notice the manifest lying.

    Mirrored by the companion native engine's ``with_registry_override``, and like it,
    raises for an unknown key so a stale bracket cannot silently flip nothing.
    """
    global _OVERRIDE
    base = _active()
    if not any(c.key == key for c in base):
        raise KeyError(f"unknown enforcement capability {key!r}")
    previous = _OVERRIDE
    _OVERRIDE = tuple(
        Capability(key=c.key, enforced=enforced, summary=c.summary) if c.key == key else c
        for c in base
    )
    try:
        yield
    finally:
        _OVERRIDE = previous


def enforcement_manifest() -> str:
    """The machine-readable manifest: key-sorted, compact, lowercase booleans.

    Byte-identical to the companion native engine's manifest by construction -- both
    serialize the same shape from their own registry, so a guard present in one build and
    missing from the other shows up as a differing manifest in the cross-engine check.
    """
    return json.dumps(
        {c.key: c.enforced for c in sorted(_active(), key=lambda c: c.key)},
        separators=(",", ":"),
        sort_keys=True,
    )


def is_enforced(key: str) -> bool:
    """Whether this engine really applies the guard named ``key``.

    Call this AT the guard site, not near it, so the flag and the behaviour cannot part
    company. Raises :class:`KeyError` for an unknown key: a misspelled capability at a
    guard site is a bug that must be loud, because the quiet reading of it -- "unknown, so
    not enforced" -- would silently disable the very guard the caller was writing.
    """
    for capability in _active():
        if capability.key == key:
            return capability.enforced
    known = ", ".join(sorted(c.key for c in _active()))
    raise KeyError(
        f"unknown enforcement capability {key!r}; a guard site must name a capability "
        f"declared in ENFORCEMENT_REGISTRY (known: {known})"
    )
