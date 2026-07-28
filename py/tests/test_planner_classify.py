"""Tests for the pure CI classifier: base state + the five red classifications."""

from __future__ import annotations

from pr_landing_planner.classify import (
    ClassifyConfig,
    FlakySignature,
    classify_pr,
    classify_state,
    flaky_signatures_from_objs,
    parse_rollup,
)
from pr_landing_planner.model import CheckRun, CiState, RedClass


def _check(name: str, conclusion: str = "", status: str = "COMPLETED", text: str = "",
           duration: int | None = None) -> CheckRun:
    return CheckRun(name=name, status=status, conclusion=conclusion, text=text, duration_secs=duration)


CFG = ClassifyConfig(gate_check="merge-gate")


def test_classify_state_green_red_pending_none() -> None:
    assert classify_state([]) is CiState.NONE
    assert classify_state([_check("CI", "SUCCESS")]) is CiState.GREEN
    assert classify_state([_check("CI", "SUCCESS"), _check("x", "FAILURE")]) is CiState.RED
    assert classify_state([_check("CI", "", "IN_PROGRESS")]) is CiState.PENDING
    # A single red anywhere makes the PR red even when others pass.
    assert classify_state([_check("a", "SUCCESS"), _check("b", "TIMED_OUT")]) is CiState.RED


def test_green_fresh_gate_ok() -> None:
    v = classify_pr([_check("CI", "SUCCESS"), _check("merge-gate", "SUCCESS")], CFG)
    assert v.raw_state is CiState.GREEN
    assert v.red_class is None
    assert v.gate_present and v.gate_ok


def test_stale_required_check() -> None:
    # Real CI green on the head, but the required gate froze red (ds-4171).
    v = classify_pr(
        [_check("CI", "SUCCESS"), _check("merge-gate", "FAILURE", text="stale")], CFG
    )
    assert v.raw_state is CiState.RED
    assert v.red_class is RedClass.STALE_REQUIRED_CHECK


def test_evaluate_once_race_is_benign() -> None:
    v = classify_pr(
        [
            _check("CI", "", "IN_PROGRESS"),
            _check("merge-gate", "FAILURE", text="Full CI still queued; rerun after CI completes"),
        ],
        CFG,
    )
    assert v.red_class is RedClass.EVALUATE_ONCE_RACE


def test_runner_outage_by_marker_and_duration() -> None:
    v1 = classify_pr(
        [_check("CI", "SUCCESS"), _check("merge-gate", "FAILURE", text="BlobNotFound", duration=3)],
        CFG,
    )
    assert v1.red_class is RedClass.RUNNER_OUTAGE
    assert v1.gate_missing_run
    v2 = classify_pr([_check("merge-gate", "STARTUP_FAILURE", text="no runner")], CFG)
    assert v2.red_class is RedClass.RUNNER_OUTAGE


def test_flaky_requires_a_signature() -> None:
    checks = [_check("wasm-core", "FAILURE", text="browser flake"), _check("merge-gate", "SUCCESS")]
    # Without a signature the wasm-core red is a REAL regression.
    assert classify_pr(checks, CFG).red_class is RedClass.REAL
    # With a matching signature it is reclassified flaky.
    cfg = ClassifyConfig(
        gate_check="merge-gate", flaky_signatures=(FlakySignature(name_regex="wasm-core"),)
    )
    assert classify_pr(checks, cfg).red_class is RedClass.FLAKY


def test_flaky_only_when_all_reds_match() -> None:
    checks = [
        _check("wasm-core", "FAILURE", text="flake"),
        _check("unit", "FAILURE", text="assertion failed"),
        _check("merge-gate", "SUCCESS"),
    ]
    cfg = ClassifyConfig(
        gate_check="merge-gate", flaky_signatures=(FlakySignature(name_regex="wasm-core"),)
    )
    # unit's red is not covered by any signature => the PR is a REAL red, not flaky.
    assert classify_pr(checks, cfg).red_class is RedClass.REAL


def test_real_red() -> None:
    v = classify_pr(
        [_check("unit", "FAILURE", text="assertion"), _check("merge-gate", "SUCCESS")], CFG
    )
    assert v.red_class is RedClass.REAL
    assert "unit" in v.detail


def test_outage_precedes_stale_and_flaky() -> None:
    # Even with a flaky signature present, a never-ran gate is an OUTAGE, not flaky.
    cfg = ClassifyConfig(
        gate_check="merge-gate", flaky_signatures=(FlakySignature(name_regex="merge-gate"),)
    )
    v = classify_pr([_check("merge-gate", "FAILURE", text="BlobNotFound", duration=1)], cfg)
    assert v.red_class is RedClass.RUNNER_OUTAGE


def test_parse_rollup_narrows_gh_json() -> None:
    raw: object = [
        {"name": "CI", "status": "COMPLETED", "conclusion": "SUCCESS"},
        {"context": "legacy-status", "state": "SUCCESS"},
        "garbage-entry-ignored",
    ]
    checks = parse_rollup(raw)
    assert len(checks) == 2
    assert checks[0].name == "CI" and checks[0].conclusion == "SUCCESS"
    assert checks[1].name == "legacy-status"


def test_flaky_signatures_from_objs() -> None:
    doc: object = {"signatures": [{"name_regex": "wasm"}, {"text_regex": "flake"}, {"note": "no regex"}]}
    sigs = flaky_signatures_from_objs(doc)
    assert len(sigs) == 2  # the note-only entry (no regex) is dropped
    doc_list: object = [{"name_regex": "x"}]
    assert len(flaky_signatures_from_objs(doc_list)) == 1
