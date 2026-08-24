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


def _step(**kw) -> Step:
    return Step("g", "j", "", "cargo build --workspace && cargo build --bin x", **kw)


def test_host_declares_the_channel_and_the_graph_does_not_have_to():
    """The motivating case: a compound command that no appended flag can reach."""
    step = _step(jobs_flag="")
    assert env_with_inner_jobs(step, "CARGO_BUILD_JOBS", 4) == {"CARGO_BUILD_JOBS": "4"}


def test_no_channel_configured_is_a_no_op():
    """A host that sets nothing behaves exactly as before this feature existed."""
    assert env_with_inner_jobs(_step(jobs_flag=""), "", 4) == {}


def test_no_declared_width_sets_nothing():
    assert env_with_inner_jobs(_step(jobs_flag=""), "CARGO_BUILD_JOBS", None) == {}


def test_a_step_may_opt_out_of_the_channel_specifically():
    assert env_with_inner_jobs(_step(jobs_env=""), "CARGO_BUILD_JOBS", 4) == {}


def test_step_override_beats_the_machine_default():
    assert env_with_inner_jobs(_step(jobs_env="MAKEFLAGS_J"), "CARGO_BUILD_JOBS", 2) == {
        "MAKEFLAGS_J": "2"
    }


# --- the predicate the refusal keys on -------------------------------------------------

def test_neither_channel_is_still_unresizable():
    """THE LOAD-BEARING CASE. Nothing here may make the refusal stop firing."""
    assert step_width_is_resizable(_step(jobs_flag=""), "-j", "") is False


def test_either_channel_alone_makes_it_resizable():
    assert step_width_is_resizable(_step(jobs_flag=""), "-j", "CARGO_BUILD_JOBS") is True
    assert step_width_is_resizable(_step(), "-j", "") is True


def test_whitespace_is_not_a_channel():
    """A blank-looking value must not be mistaken for a usable channel."""
    assert step_width_is_resizable(_step(jobs_flag="  "), "-j", "   ") is False


# --- resolution from the host ----------------------------------------------------------

def test_resolves_from_the_host_environment():
    assert resolve_jobs_env(env={JOBS_ENV_ENV: "CARGO_BUILD_JOBS"}) == "CARGO_BUILD_JOBS"


def test_unset_means_no_channel():
    assert resolve_jobs_env(env={}) == ""


def test_a_malformed_name_is_REFUSED_not_ignored():
    """Silently ignoring a typo would turn every self-managed step back into a hard capacity
    refusal, and the operator would debug the wrong thing."""
    with pytest.raises(ValueError, match="not a valid environment variable name"):
        resolve_jobs_env(env={JOBS_ENV_ENV: "not a name"})
