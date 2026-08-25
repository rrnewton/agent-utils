"""The MACHINE-level env channel for a step's inner width.

These tests exist because the capacity refusal must keep its teeth. A step that bakes its
width into its own command cannot be narrowed by clamping its cgroup quota -- that would leave
the original worker count running inside a smaller box, a slowdown disguised as a limit. The
env channel gives such a step a way to be narrowed honestly; it must NOT give it a way to be
waved through.
"""

from __future__ import annotations

import pytest

from dagrun.model import (
    JOBS_ENV_ENV,
    Step,
    env_with_inner_jobs,
    resolve_jobs_env,
    step_width_is_resizable,
)


def _step(*, jobs_flag: str | None = None, jobs_env: str | None = None) -> Step:
    """A step whose command no appended flag can reach, with the two channels spelled out.

    The channels are named parameters rather than ``**kw``: ``Step`` has fields of a dozen
    different types, so a forwarding ``**kw`` cannot be annotated at all without widening every
    one of them, and this file was failing ``mypy --strict`` -- and therefore ``make validate``
    -- for exactly that reason.
    """

    return Step(
        "g",
        "j",
        "",
        "cargo build --workspace && cargo build --bin x",
        jobs_flag=jobs_flag,
        jobs_env=jobs_env,
    )


def test_host_declares_the_channel_and_the_graph_does_not_have_to() -> None:
    """The motivating case: a compound command that no appended flag can reach."""
    step = _step(jobs_flag="")
    assert env_with_inner_jobs(step, "CARGO_BUILD_JOBS", 4) == {"CARGO_BUILD_JOBS": "4"}


def test_no_channel_configured_is_a_no_op() -> None:
    """A host that sets nothing behaves exactly as before this feature existed."""
    assert env_with_inner_jobs(_step(jobs_flag=""), "", 4) == {}


def test_no_declared_width_sets_nothing() -> None:
    assert env_with_inner_jobs(_step(jobs_flag=""), "CARGO_BUILD_JOBS", None) == {}


def test_a_step_may_opt_out_of_the_channel_specifically() -> None:
    assert env_with_inner_jobs(_step(jobs_env=""), "CARGO_BUILD_JOBS", 4) == {}


def test_step_override_beats_the_machine_default() -> None:
    assert env_with_inner_jobs(_step(jobs_env="MAKEFLAGS_J"), "CARGO_BUILD_JOBS", 2) == {
        "MAKEFLAGS_J": "2"
    }


# --- the predicate the refusal keys on -------------------------------------------------

def test_neither_channel_is_still_unresizable() -> None:
    """THE LOAD-BEARING CASE. Nothing here may make the refusal stop firing."""
    assert step_width_is_resizable(_step(jobs_flag=""), "-j", "") is False


def test_either_channel_alone_makes_it_resizable() -> None:
    assert step_width_is_resizable(_step(jobs_flag=""), "-j", "CARGO_BUILD_JOBS") is True
    assert step_width_is_resizable(_step(), "-j", "") is True


def test_whitespace_is_not_a_channel() -> None:
    """A blank-looking value must not be mistaken for a usable channel."""
    assert step_width_is_resizable(_step(jobs_flag="  "), "-j", "   ") is False


# --- resolution from the host ----------------------------------------------------------

def test_resolves_from_the_host_environment() -> None:
    assert resolve_jobs_env(env={JOBS_ENV_ENV: "CARGO_BUILD_JOBS"}) == "CARGO_BUILD_JOBS"


def test_unset_means_no_channel() -> None:
    assert resolve_jobs_env(env={}) == ""


def test_a_malformed_name_is_REFUSED_not_ignored() -> None:
    """Silently ignoring a typo would turn every self-managed step back into a hard capacity
    refusal, and the operator would debug the wrong thing."""
    with pytest.raises(ValueError, match="not a valid environment variable name"):
        resolve_jobs_env(env={JOBS_ENV_ENV: "not a name"})


# --- the planner must ALLOCATE for an env-only step, not pass the raw declaration ------

def test_the_planner_allocates_width_for_an_env_only_step() -> None:
    """REGRESSION. The first version of this feature delivered the env channel at execution
    time but left `apply_plan_to_config` gating on `jobs_flag` alone, so a step resizable
    ONLY by env kept its raw `preferred_inner_jobs` instead of the planner's allocated
    width. It would have handed cargo the declared 32 while the runner believed it had
    clamped the step -- the exact "declared width that never reached the tool" defect this
    feature exists to fix, inverted.

    Five sites in estimates.py made that distinction; all now ask
    `step_width_is_resizable`, which is true for either channel.
    """
    from dagrun.estimates import apply_plan_to_config  # noqa: F401  (import must resolve)
    import inspect

    import dagrun.estimates as est

    src = inspect.getsource(est)
    # No site may gate width decisions on the flag alone any more.
    assert "effective_jobs_flag(step, cfg.default_jobs_flag).strip()" not in src, (
        "a width decision still gates on jobs_flag alone; an env-only step would be "
        "treated as unresizable and keep its raw declared width"
    )
    assert src.count("step_width_is_resizable(") >= 5
