"""Receipt verdict: what is proven about one exact commit."""
from __future__ import annotations


import json
from pathlib import Path

from .config import Config
from .ledger import Ledger


def _canonical_json(value: object) -> str:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False)


def _release_identity_ok(cfg: Config, r: dict[str, object]) -> bool:
    """Whether a row's release_builder/e2e_payload claim may qualify.

    The two fields are read together: a lone field names no builder. A row
    carrying neither field makes no identity claim and is judged by the other
    clauses. When both fields are present, the claim is honoured only when a
    current payload is configured and the row payload is exactly that payload;
    with no configured payload an identity claim cannot be honoured.
    """
    has_builder = "release_builder" in r
    has_payload = "e2e_payload" in r
    if not has_builder and not has_payload:
        return True
    if not (has_builder and has_payload):
        return False
    if cfg.release_payload is None:
        return False
    try:
        return _canonical_json(r.get("e2e_payload")) == _canonical_json(cfg.release_payload)
    except (TypeError, ValueError):
        return False


def remote_for_commit(cfg: Config, commit: str) -> dict[str, object]:
    """Return cached remote evidence for one exact commit, or unknown."""
    """Remote evidence: an optional JSON file written by `remote` ingest, else unknown."""
    p = cfg.state_dir / "remote.json"
    if cfg.provider == "none" or not p.exists():
        return {"source": "remote", "status": "unknown"}
    try:
        data = json.loads(p.read_text())
    except Exception:
        return {"source": "remote", "status": "unknown"}
    for row in data.get("runs", []):
        if str(row.get("commit", "")) == commit:
            return {"source": "remote", "status": str(row.get("status", "unknown")).lower(), "row": row}
    return {"source": "remote", "status": "unknown"}


def show(cfg: Config, commit: str, remote: dict[str, object] | None = None) -> dict[str, object]:
    """Return the receipt verdict for one exact commit."""
    if commit in ("HEAD", "@"):
        import subprocess
        try:
            commit = subprocess.run(["git", "rev-parse", "HEAD"], cwd=cfg.root, capture_output=True, text=True, timeout=10).stdout.strip() or commit
        except Exception:
            pass
    rows = Ledger(cfg).for_commit(commit)
    _RESULTS = {"pass", "fail", "timeout", "incomplete", "killed", "no_result"}

    def _result_of(r: dict[str, object]) -> str:
        value = str(r.get("result", "")).strip().lower()
        return "fail" if value == "failed" else value  # reader leniency only

    def _version_of(r: dict[str, object]) -> int:
        value = r.get("schema_version")
        return value if isinstance(value, int) and not isinstance(value, bool) else 0

    def _count_of(value: object) -> int | None:
        if isinstance(value, bool):
            return None
        if isinstance(value, int):
            return value
        if isinstance(value, list):
            return len(value)
        return None

    def _with_totals(r: dict[str, object]) -> dict[str, object]:
        # Test totals may arrive nested under test_results.totals.
        out = dict(r)
        holder = r.get("test_results")
        totals = holder.get("totals") if isinstance(holder, dict) else None
        if isinstance(totals, dict):
            for key in ("executed_tests", "passed_tests", "filtered_tests",
                        "failures", "known_gaps"):
                if out.get(key) is None and totals.get(key) is not None:
                    out[key] = totals[key]
        return out

    def _strict_pass(r: dict[str, object]) -> bool:
        r = _with_totals(r)
        if _result_of(r) != "pass" or r.get("_digest_ok") is False or r.get("_evidence_ok") is False:
            return False
        version = _version_of(r)
        if version >= 6 and cfg.require_full_evidence:
            # Schema-6 records: repository required, full selection required.
            repo = r.get("repo")
            if not isinstance(repo, str) or not repo:
                return False
            if cfg.repo and repo != cfg.repo:
                return False  # the same SHA never qualifies across repositories
            if r.get("profile") != "full" or r.get("selection_mode") != "full":
                return False
        if version >= 5 and cfg.require_full_evidence:
            # Schema-5+: gate accounting must close and coverage is required.
            ran = _count_of(r.get("gates_run"))
            expected = _count_of(r.get("gates_expected"))
            skipped = _count_of(r.get("intentional_skipped_nodes")) or 0
            if ran is None or expected is None or ran + skipped != expected:
                return False
            cov = r.get("coverage")
            if not isinstance(cov, dict):
                return False
            planned = cov.get("planned_test_nodes")
            planned_n = len(planned) if isinstance(planned, list) else planned
            if not planned_n or cov.get("zero_executed_nodes") not in ([],) or cov.get("absent_nodes") not in ([],):
                return False
        if r.get("commit_anchored") is False:
            return False
        if not _release_identity_ok(cfg, r):
            return False  # superseded/mismatched release identity never qualifies (see _release_identity_ok)
        if cfg.require_full_evidence:
            # Ported from the canonical qualifying predicate: full profile and
            # selection, clean tree, no failures, no filtered tests, passed ==
            # executed, and complete gate accounting (when fields are present).
            tests_val = r.get("executed_tests")
            if not (r.get("commit_anchored") is True and isinstance(tests_val, (int, float)) and not isinstance(tests_val, bool) and tests_val > 0):
                return False
            if r.get("profile") not in (None, "full") or r.get("selection_mode") not in (None, "full"):
                return False
            if r.get("tree_dirty") not in (None, False) or (r.get("failures") or 0) != 0:
                return False
            if r.get("passed_tests") is not None and r.get("passed_tests") != r.get("executed_tests"):
                # Owner-approved known-gap exception: a shortfall qualifies
                # only when every missing test is ledgered as a known gap.
                executed = r.get("executed_tests")
                passed = r.get("passed_tests")
                gaps = _count_of(r.get("known_gaps"))
                if not (isinstance(executed, int) and isinstance(passed, int)
                        and gaps is not None and executed - passed <= gaps):
                    return False
            if cfg.release_payload is not None and r.get("release_builder") is not None and r.get("e2e_payload") != cfg.release_payload:
                return False  # only the configured current release identity qualifies; superseded payloads are supplemental
            cov = r.get("coverage")
            if isinstance(cov, dict):
                planned = cov.get("planned_test_nodes")
                planned_n = len(planned) if isinstance(planned, list) else planned
                if not planned_n or cov.get("zero_executed_nodes") not in ([],) or cov.get("absent_nodes") not in ([],):
                    return False  # per-node coverage must be reported, planned, and clean
            if r.get("gates_run") is not None and r.get("gates_expected") is not None and r["gates_run"] != r["gates_expected"]:
                return False
        tests = r.get("executed_tests")
        if tests is not None and not (isinstance(tests, (int, float)) and not isinstance(tests, bool) and tests > 0):
            return False
        return True
    def _is_genuine_fail(r: dict[str, object]) -> bool:
        # Only a clean full-coverage FAIL (anchored, full profile/selection,
        # clean tree, verified evidence) is a durable failure. A partial-profile
        # FAIL (e.g. only-portable) is a withheld non-verdict and must not veto
        # a qualifying PASS.
        return (
            _result_of(r) == "fail"
            and r.get("_evidence_ok") is not False
            and r.get("_digest_ok") is not False
            and r.get("commit_anchored") is True
            and r.get("profile") == "full"
            and r.get("selection_mode") == "full"
            and r.get("tree_dirty") is False
        )
    strict_passes = [r for r in rows if _strict_pass(r)]
    local_pass = bool(strict_passes)
    genuine_fails = [r for r in rows if _is_genuine_fail(r)]
    # Newest terminal record decides: a genuine FAIL vetoes only when it is
    # at least as new as the newest qualifying PASS.
    fail_veto = False
    if genuine_fails and strict_passes:
        newest_pass = max(str(r.get("finished_at") or "") for r in strict_passes)
        newest_fail = max(str(r.get("finished_at") or "") for r in genuine_fails)
        fail_veto = newest_fail >= newest_pass
    elif genuine_fails:
        fail_veto = True
    remote = remote if remote is not None else remote_for_commit(cfg, commit)
    checks = {
        "local-ledger-pass": local_pass and not fail_veto,
        "remote-pass": remote.get("status") in ("pass", "success", "passed"),
    }
    qualified = all(checks.get(rule, False) for rule in cfg.qualifying) if cfg.qualifying else False
    if not rows and remote.get("status") == "unknown":
        verdict = "no-result"
    elif qualified:
        verdict = "qualified"
    else:
        verdict = "not-qualified"
    return {
        "schema": "local-ci-hub/receipt/v1", "commit": commit, "verdict": verdict,
        "qualified": qualified, "qualifying": cfg.qualifying, "checks": checks,
        "local_rows": rows, "remote": remote,
        "note": "no-result means no readable ledger row names this commit; it cannot distinguish never-run from failed-before-write",
    }
