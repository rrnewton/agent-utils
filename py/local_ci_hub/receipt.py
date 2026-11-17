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
    _LEGACY_RESULTS = {"PASS": "pass", "FAIL": "fail", "FAILED": "fail",
                       "TIMEOUT": "timeout", "INCOMPLETE": "incomplete",
                       "KILLED": "killed", "NO_RESULT": "no_result"}
    _FLOOR_KINDS = {"current-main", "frozen-target", "main-ancestor",
                    "disjoint-main", "queue-base"}
    _FINISHED = __import__("re").compile(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z")
    _HEX40 = __import__("re").compile(r"[0-9a-f]{40}")

    def _result_of(r: dict[str, object]) -> str:
        # Canonical lowercase, plus exact legacy uppercase spellings.
        # No stripping or case folding: " Pass " is not a result.
        value = r.get("result")
        if not isinstance(value, str):
            return ""
        if value in _RESULTS:
            return value
        return _LEGACY_RESULTS.get(value, "")

    def _version_of(r: dict[str, object]) -> int | None:
        # None means refused: present-but-malformed or above 6.
        if "schema_version" not in r:
            return 0  # unversioned legacy record: era rules
        value = r.get("schema_version")
        if isinstance(value, bool) or not isinstance(value, int) or value < 0 or value > 6:
            return None
        return value

    def _count_of(value: object) -> int | None:
        if isinstance(value, bool):
            return None
        if isinstance(value, int):
            return value
        if isinstance(value, list):
            return len(value)
        return None

    def _is_int(value: object) -> bool:
        return isinstance(value, int) and not isinstance(value, bool)

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

    def _era_hash(r: dict[str, object]) -> str:
        from .ledger import canonical_bytes as _canon
        import hashlib as _hl
        body = {k: v for k, v in r.items()
                if not str(k).startswith("_") and k != "digest"}
        return _hl.sha256(_canon(body)).hexdigest()

    def _floor_ok(floor: object) -> bool:
        # Complete floor evidence for every kind: hex floor and
        # tree, an authority binding, 64-hex plan digests, and pin
        # bindings; current-main also needs commit equality.
        if not isinstance(floor, dict) or floor.get("floor_kind") not in _FLOOR_KINDS:
            return False
        for key in ("floor_sha", "floor_tree"):
            value = floor.get(key)
            if not isinstance(value, str) or _HEX40.fullmatch(value) is None:
                return False
        if not isinstance(floor.get("authority"), str) or not floor.get("authority"):
            return False
        plans = floor.get("plan_sha256s")
        if not isinstance(plans, list) or not plans:
            return False
        _hex64 = __import__("re").compile(r"[0-9a-f]{64}")
        if any(not isinstance(v, str) or _hex64.fullmatch(v) is None for v in plans):
            return False
        if not isinstance(floor.get("pin_bindings"), (dict, list)):
            return False
        if floor.get("floor_kind") == "current-main":
            # floor_sha is already hex-checked above, so equality
            # with it also proves the observed SHA is hex.
            if floor.get("floor_sha") != floor.get("observed_main_sha"):
                return False
        return True

    def _era_listed(r: dict[str, object]) -> bool:
        # Frozen era set: an entry is the pair "id:sha256" — the
        # record's id (record_id, else run_id) AND the sha256 of
        # that exact canonical row, matched together. Copying a
        # listed id onto a different row never matches, and any
        # later event that alters the row changes its hash, so
        # corrections and enrichments never inherit era status.
        listed = set(cfg.era_records)
        ident = r.get("record_id") or r.get("run_id")
        return isinstance(ident, str) and bool(ident) and f"{ident}:{_era_hash(r)}" in listed

    def _repo_allowed(r: dict[str, object], version: int) -> bool:
        # A foreign repository never qualifies at any version. A
        # missing repository refuses schema 5+ records; below that
        # only frozen-era records may omit it.
        allowed = set(cfg.counted_repos) | set(cfg.repo_aliases)
        if cfg.repo:
            allowed.add(cfg.repo)
        repo = r.get("repo")
        if repo is not None:
            return isinstance(repo, str) and repo in allowed
        if version >= 5:
            return False  # schema 5+ must name its repository
        return _era_listed(r)  # below schema 5: frozen era set only

    def _strict_pass(r: dict[str, object]) -> bool:
        r = _with_totals(r)
        if _result_of(r) != "pass" or r.get("_digest_ok") is False or r.get("_evidence_ok") is False:
            return False
        version = _version_of(r)
        if version is None:
            return False  # malformed or future schema version: refused
        if version < 6 and not _era_listed(r):
            return False  # unlisted: schema 6 with every clause, or nothing
        if not _repo_allowed(r, version):
            return False
        if not _release_identity_ok(cfg, r):
            return False  # superseded/mismatched release identity never qualifies
        if version >= 6:
            # Every schema-6 clause, unconditional on evidence mode.
            if r.get("commit_anchored") is not True:
                return False
            if cfg.repo and r.get("repo") != cfg.repo and r.get("repo") not in cfg.repo_aliases:
                return False
            producers = cfg.producers or ["local-ci-hub"]
            if r.get("producer") not in producers:
                return False  # unregistered producer
            if not isinstance(r.get("record_id"), str) or not r.get("record_id"):
                return False
            tree = r.get("tree")  # run_id itself is envelope-guaranteed by the fold
            if not isinstance(tree, str) or _HEX40.fullmatch(tree) is None:
                return False
            if r.get("tree_dirty") is not False:
                return False
            # int totals are enforced by the passed/executed block below
            kinds = cfg.admission_kinds or ["host-limits-live"]
            if r.get("admission") not in kinds:
                return False  # missing or unknown admission kind
            if not _floor_ok(r.get("admission_floor_evidence")):
                return False
            tooling = r.get("tooling_sha")
            if not isinstance(tooling, str) or _HEX40.fullmatch(tooling) is None:
                return False  # unknown tooling never qualifies
            cells = r.get("cell_results")
            if not isinstance(cells, dict):
                return False
            if cells.get("run_id") != r.get("run_id") or cells.get("commit") != r.get("commit"):
                return False  # cell results bound to another run or commit
            if not isinstance(r.get("log_identity"), str) or not r.get("log_identity"):
                return False
            stamp = r.get("finished_at")
            if not isinstance(stamp, str) or _FINISHED.fullmatch(stamp) is None:
                return False
            if r.get("profile") != "full" or r.get("selection_mode") != "full":
                return False
        if version == 5:
            # Schema 5 identity clauses (schema 6 adds its own below).
            if r.get("commit_anchored") is not True or r.get("tree_dirty") is not False:
                return False
            if r.get("profile") != "full" or r.get("selection_mode") != "full":
                return False
            producers5 = cfg.producers or ["local-ci-hub"]
            if r.get("producer") not in producers5:
                return False
            kinds5 = cfg.admission_kinds or ["host-limits-live"]
            if r.get("admission") not in kinds5:
                return False
            tooling5 = r.get("tooling_sha")
            if not isinstance(tooling5, str) or _HEX40.fullmatch(tooling5) is None:
                return False
            if not _floor_ok(r.get("admission_floor_evidence")):
                return False
        if version >= 5:
            if not _is_int(r.get("failures")) or r.get("failures") != 0:
                return False  # any failure refuses, at schema 5 and 6
            # Gate accounting closes; named skip lists are lists.
            ran = _count_of(r.get("gates_run"))
            expected = _count_of(r.get("gates_expected"))
            skipped = _count_of(r.get("intentional_skipped_nodes", 0))
            if ran is None or expected is None or skipped is None or ran < 0 or expected <= 0 or skipped < 0 or ran + skipped != expected:
                return False
            for key in ("skipped_nodes", "dependency_skipped_nodes", "unaccounted_nodes"):
                value = r.get(key)
                if value is not None and not isinstance(value, list):
                    return False
            cov = r.get("coverage")
            if not isinstance(cov, dict):
                return False
            planned = cov.get("planned_test_nodes")
            planned_n = planned if _is_int(planned) else (len(planned) if isinstance(planned, list) else 0)
            if not isinstance(planned_n, int) or planned_n <= 0:
                return False
            if cov.get("zero_executed_nodes") not in ([],) or cov.get("absent_nodes") not in ([],):
                return False
        if version < 5:
            # Era rules for older records (schema 5+ clauses above
            # already ran unconditionally for their versions).
            tests_val = r.get("executed_tests")
            if not (r.get("commit_anchored") is True and isinstance(tests_val, (int, float)) and not isinstance(tests_val, bool) and tests_val > 0):
                return False
            if r.get("profile") not in (None, "full") or r.get("selection_mode") not in (None, "full"):
                return False
            if r.get("tree_dirty") not in (None, False) or (r.get("failures") or 0) != 0:
                return False
            cov = r.get("coverage")
            if isinstance(cov, dict):
                planned = cov.get("planned_test_nodes")
                planned_n = planned if _is_int(planned) else (len(planned) if isinstance(planned, list) else 0)
                if not isinstance(planned_n, int) or planned_n <= 0 or cov.get("zero_executed_nodes") not in ([],) or cov.get("absent_nodes") not in ([],):
                    return False  # per-node coverage must be reported, planned, and clean
            if r.get("gates_run") is not None and r.get("gates_expected") is not None and r["gates_run"] != r["gates_expected"]:
                return False
        # Passed == executed, or the ledgered known-gap exception:
        # named test ids, each in the configured gap ledger.
        passed_raw, executed_raw = r.get("passed_tests"), r.get("executed_tests")
        if passed_raw is not None or version >= 6:
            if not _is_int(passed_raw) or not _is_int(executed_raw):
                return False
            passed_i, executed_i = int(str(passed_raw)), int(str(executed_raw))
            if passed_i > executed_i:
                return False
            if passed_i != executed_i:
                gaps = r.get("known_gaps")
                ledger = set(cfg.known_gaps)
                if not isinstance(gaps, list) or not gaps:
                    return False
                if any(not isinstance(g, str) or g not in ledger for g in gaps):
                    return False
                if len(set(gaps)) < executed_i - passed_i:
                    return False
        tests = r.get("executed_tests")
        if tests is not None and not (_is_int(tests) and int(str(tests)) > 0):
            return False
        return True
    def _is_genuine_fail(r: dict[str, object]) -> bool:
        # Only a clean full-coverage failure (anchored, full
        # profile/selection, clean tree, verified evidence) is a
        # durable failure. A partial-profile failure is a withheld
        # non-verdict and must not veto a qualifying pass.
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
        def _when(r: dict[str, object], *, fail: bool) -> str:
            import datetime as _dt
            raw = str(r.get("finished_at") or "")
            try:
                parsed = _dt.datetime.fromisoformat(raw.replace("Z", "+00:00"))
            except ValueError:
                return "9999" if fail else ""  # fail closed both ways
            if parsed.tzinfo is None:
                return "9999" if fail else ""
            return parsed.astimezone(_dt.timezone.utc).isoformat()
        newest_pass = max(_when(r, fail=False) for r in strict_passes)
        newest_fail = max(_when(r, fail=True) for r in genuine_fails)
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
