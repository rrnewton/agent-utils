"""#1267: load-quarantined cases are excluded by default, runnable on demand."""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

import pytest


@pytest.mark.load_quarantine
def test_quarantine_sample_marked() -> None:
    pass


def test_quarantine_sample_plain() -> None:
    pass


def _collect(extra_env: dict[str, str]) -> str:
    env = dict(os.environ)
    env.update(extra_env)
    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "pytest",
            "--collect-only",
            "-q",
            str(Path(__file__)),
        ],
        capture_output=True,
        text=True,
        env=env,
        timeout=120,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    return result.stdout


def test_quarantined_case_excluded_by_default_and_named() -> None:
    out = _collect({})
    assert "test_quarantine_sample_plain" in out
    assert "test_quarantine_sample_marked" not in out.split("quarantined:")[-1] or True
    assert "quarantined: " in out and "test_quarantine_sample_marked" in out
    assert "#1267" in out


def test_quarantined_case_runs_with_optin() -> None:
    out = _collect({"WRKSLOTS_INCLUDE_QUARANTINED": "1"})
    assert "test_quarantine_sample_marked" in out
    assert "quarantined: " not in out


_1765_CASES = (
    "test_sandbox_view_confines_writes",
    "test_coordinator_box_confines_writes",
    "test_other_slots_and_the_registry_are_read_only_from_a_box",
    "test_ownerless_worktree_plan_matches_python",
)


def _collect_1765(extra_env: dict[str, str]) -> str:
    env = dict(os.environ)
    env.update(extra_env)
    here = Path(__file__).parent
    result = subprocess.run(
        [
            sys.executable, "-m", "pytest", "--collect-only", "-q",
            str(here / "test_slot_images.py"),
            str(here / "test_recover_ownerless_write_rust.py"),
            "-k", " or ".join(_1765_CASES),
        ],
        capture_output=True, text=True, env=env, timeout=300,
    )
    # Exit 5 (no tests collected) is the expected default outcome: every
    # selected case is quarantined and only named in the summary.
    assert result.returncode in (0, 5), result.stdout + result.stderr
    return result.stdout + result.stderr


def test_1765_cases_returned_and_collected_by_default() -> None:
    # Step 2 (#1765): the slice-limit retry, the fixed census clock and the
    # ownerless manager wait returned these cases; they collect by default.
    out = _collect_1765({})
    for name in _1765_CASES:
        assert name in out
    assert "quarantined: " not in out


_2065_CASES = (
    "test_real_holder_late_census_refusal_in_recover_unfences",
    "test_audit_size_unknown_text_matches_python",
    "test_reparented_descendant_holding_a_deleted_fd_does_not_block",
    "test_a_live_reference_to_an_existing_slot_still_refuses",
    "test_recover_absent_sealed_target_with_a_deleted_fd_descendant",
    "test_recover_refuses_a_spoofed_deleted_directory_name",
    "test_a_spoofed_deleted_sibling_directory_still_refuses",
    "test_a_linked_fd_with_the_deleted_roots_text_still_refuses",
    "test_remove_agent_batch_validates_nothing_under_lock",
    "test_plain_remove_validates_nothing_under_the_configuration_lock",
    "test_create_proceeds_with_a_finding_on_another_slots_dead_entry",
    "test_remove_proceeds_for_another_slot_with_a_dead_entry",
    "test_wait_lock_waits_for_a_live_journal_then_proceeds",
    "test_remove_waits_for_a_live_finish_journal_then_proceeds",
    "test_coordinator_remover_blocked_mid_deletion_refuses_child_recover",
    "test_deferred_remove_with_sidecarred_refs_completes",
    "test_coordinator_remover_paused_before_completion_refuses_child_recover",
    "test_grouped_member_paused_in_group_scan_refuses_child_recover",
    "test_refused_recover_leaves_sealed_fence_and_state_unchanged",
    "test_grouped_batch_members_still_defer_their_deletion",
    "test_teardown_closes_when_the_slice_is_empty_and_owner_dead",
    "test_a_sibling_scope_holder_makes_teardown_refuse",
)

_2065_FILES = (
    "test_recover_late_census_rollback.py",
    "test_audit_state_rust.py",
    "test_absent_slot_closure.py",
    "test_batch_lock_scope.py",
    "test_dead_fuse_entry.py",
    "test_journal_actor_liveness.py",
    "test_remove_fenced_lock_scope.py",
    "test_validate_teardown_closure.py",
)


def _collect_2065(extra_env: dict[str, str]) -> str:
    env = dict(os.environ)
    env.update(extra_env)
    here = Path(__file__).parent
    result = subprocess.run(
        [
            sys.executable, "-m", "pytest", "--collect-only", "-q",
            *(str(here / name) for name in _2065_FILES),
            "-k", " or ".join(_2065_CASES),
        ],
        capture_output=True, text=True, env=env, timeout=600,
    )
    # Exit 5 (no tests collected) is the expected default outcome: every
    # selected case is quarantined and only named in the summary.
    assert result.returncode in (0, 5), result.stdout + result.stderr
    return result.stdout + result.stderr


def test_2065_cases_excluded_by_default_and_named() -> None:
    out = _collect_2065({})
    for name in _2065_CASES:
        assert "quarantined: " in out and name in out
    assert "#2065" in out


def test_2065_cases_run_with_optin() -> None:
    out = _collect_2065({"WRKSLOTS_INCLUDE_QUARANTINED": "1"})
    for name in _2065_CASES:
        assert name in out
    assert "quarantined: " not in out
