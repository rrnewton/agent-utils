"""Differential: Rust `wrkviz audit-glossary` vs the oracle.

One hand-built archive of retired schema-3 glossary caches exercises
every mechanical rejection reason, the semantic-review path, a
missing cache, explicit and discovered team selection, and refusal
cases. Audit is read-only, so both editions run against the same
fixture and stdout/stderr/exit must agree byte-for-byte.
"""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

import pytest

from wrkviz.terminology import glossary_term_id

AU_ROOT = Path(__file__).resolve().parents[2]
PYTHON_WRKVIZ = AU_ROOT / "py" / "bin" / "wrkviz"
RUST_WRKVIZ = AU_ROOT / "rs" / "target" / "release" / "wrkvizd"


@pytest.fixture(scope="module")
def rust_wrkviz() -> Path:
    subprocess.run(
        ["cargo", "build", "--release", "--manifest-path", "rs/Cargo.toml",
         "-p", "wrkviz"],
        cwd=AU_ROOT, check=True, capture_output=True, text=True, timeout=900,
    )
    return RUST_WRKVIZ


def _term(term: str, status: str = "supported", definition: str = "",
          context: str = "") -> dict[str, str]:
    return {
        "term": term,
        "term_id": glossary_term_id(term),
        "definition_status": status,
        "definition": definition,
        "context": context,
    }


# One entry per rejection reason, plus review candidates.
TERMS = [
    _term("unsupported-widget", status="unsupported",
          definition="The widget engine runtime system"),
    _term("command-stdout", definition="output of the command",
          context="saw <command-stdout> here"),
    _term("mystery-field", definition="a field value",
          context="the <mystery-field> element"),
    _term("fail-closed", definition="The system runtime behaviour"),
    _term("API", definition="The system interface"),
    _term("force-push", definition="The system runtime behaviour"),
    _term("5m", definition="The system runtime behaviour"),
    _term("every 5 minutes", definition="The system runtime behaviour"),
    _term("deadbeefcafe", definition="The system runtime behaviour"),
    _term("issuecomment-123", definition="The system runtime behaviour"),
    _term("EACCES", definition="The system runtime behaviour"),
    _term("agent-42", definition="The system runtime behaviour"),
    _term("hello, world", definition="The system runtime behaviour"),
    _term(".leading-dot", definition="The system runtime behaviour"),
    _term("one two three four five six seven eight nine",
          definition="The system runtime behaviour"),
    _term("some_thing", definition="The system runtime behaviour"),
    _term("FOO_BAR=1", definition="The system runtime behaviour"),
    _term("run()", definition="The system runtime behaviour"),
    _term("sudo widget-restart", definition="The system runtime behaviour"),
    _term("widget-flag", definition="The widget-flag is a flag that toggles"),
    _term("zebra-widget", definition="A striped pattern on the archive wall"),
    _term("widget-engine",
          definition="The widget engine is a runtime system for parsing"),
    _term("widget-repo-history",
          definition="The repository workspace milestone that shipped"),
]


def _archive(base: Path) -> Path:
    archive = base / "archive"
    cache = archive / "teams" / "alpha" / "summary_data" / "glossary.json"
    cache.parent.mkdir(parents=True)
    cache.write_text(json.dumps(
        {"schema_version": 3, "terms": TERMS}, indent=1) + "\n")
    (archive / "teams" / "beta" / "summary_data").mkdir(parents=True)
    gamma = archive / "teams" / "gamma" / "summary_data" / "glossary.json"
    gamma.parent.mkdir(parents=True)
    gamma.write_text(json.dumps(
        {"schema_version": 2, "terms": []}, indent=1) + "\n")
    return archive


def _run(command: list[str], args: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        command + args, cwd=AU_ROOT, capture_output=True, text=True,
        check=False, timeout=300,
    )


def _both(archive: Path, args: list[str], expect: int = 0,
          ) -> dict[str, subprocess.CompletedProcess[str]]:
    outcomes = {}
    for label, command in (("py", [sys.executable, str(PYTHON_WRKVIZ)]),
                           ("rs", [str(RUST_WRKVIZ)])):
        outcomes[label] = _run(command, ["audit-glossary", *args])
    oracle, result = outcomes["py"], outcomes["rs"]
    assert oracle.returncode == expect, oracle.stderr
    assert result.returncode == oracle.returncode
    assert result.stdout == oracle.stdout
    assert result.stderr == oracle.stderr
    return outcomes


def test_audit_glossary_text_and_json(tmp_path: Path, rust_wrkviz: Path) -> None:
    archive = _archive(tmp_path)
    outcomes = _both(archive, ["--output", str(archive), "--team", "alpha"])
    assert "widget-engine" not in outcomes["py"].stdout  # no details yet
    assert "semantic-review-required" not in outcomes["py"].stdout
    _both(archive, ["--output", str(archive), "--team", "alpha", "--details"])
    _both(archive, ["--output", str(archive), "--team", "alpha",
                    "--format", "json"])
    outcomes = _both(archive, ["--output", str(archive), "--team", "alpha",
                               "--format", "json", "--details"])
    report = json.loads(outcomes["py"].stdout)
    assert report["totals"] == {
        "teams": 1,
        "legacy_candidates": len(TERMS),
        "rejected_mechanical": len(TERMS) - 2,
        "semantic_review_required": 2,
        "published_from_legacy_cache": 0,
    }
    reasons = {item["term"]: item["reason"]
               for item in report["teams"][0]["terms"]}
    assert reasons["unsupported-widget"] == "definition-not-supported"
    assert reasons["command-stdout"] == "transcript-markup-field"
    assert reasons["mystery-field"] == "transcript-markup-field"
    assert reasons["fail-closed"] == "generic-language-or-workflow"
    assert reasons["API"] == "generic-language-or-workflow"
    assert reasons["force-push"] == "process-action"
    assert reasons["5m"] == "duration-or-command-example"
    assert reasons["every 5 minutes"] == "duration-or-command-example"
    assert reasons["deadbeefcafe"] == "opaque-record-identifier"
    assert reasons["EACCES"] == "opaque-record-identifier"
    assert reasons["hello, world"] == "opaque-or-prose-fragment"
    assert reasons["some_thing"] == "code-or-configuration-literal"
    assert reasons["FOO_BAR=1"] == "code-or-configuration-literal"
    assert reasons["sudo widget-restart"] == "operational-command"
    assert reasons["widget-flag"] == "operational-command-or-label"
    assert reasons["zebra-widget"] == "no-semantic-kind-evidence"
    assert reasons["widget-engine"] == "legacy-schema-lacks-semantic-classification"


def test_audit_glossary_missing_cache_and_selection(
        tmp_path: Path, rust_wrkviz: Path) -> None:
    archive = _archive(tmp_path)
    outcomes = _both(archive, ["--output", str(archive), "--team", "beta"])
    assert "beta: no legacy glossary cache" in outcomes["py"].stdout
    # Discovery skips gamma's refusal only when teams are named; the
    # full discovery run hits the schema-2 cache and refuses.
    _both(archive, ["--output", str(archive)], expect=2)
    _both(archive, ["--output", str(archive),
                    "--team", "alpha", "--team", "beta"])


def test_audit_glossary_refusals(tmp_path: Path, rust_wrkviz: Path) -> None:
    archive = _archive(tmp_path)
    _both(archive, ["--output", str(archive), "--team", "gamma"], expect=2)
    _both(archive, ["--output", str(archive),
                    "--team", "alpha", "--team", "alpha"], expect=2)
    _both(archive, ["--output", str(archive), "--team", "Bad Slug"], expect=2)
    bad = archive / "teams" / "delta" / "summary_data" / "glossary.json"
    bad.parent.mkdir(parents=True)
    bad.write_text(json.dumps({
        "schema_version": 3,
        "terms": [_term("widget-engine") | {"term_id": "term-wrong-000000000000"}],
    }) + "\n")
    _both(archive, ["--output", str(archive), "--team", "delta"], expect=2)
    _both(archive, ["--output", str(tmp_path / "missing")], expect=2)


def test_audit_glossary_external_build_state(
        tmp_path: Path, rust_wrkviz: Path) -> None:
    # Build state outside the archive, no `teams/` yet: discovery
    # must find the ingested team and report its missing cache
    # instead of refusing with "no team directory found".
    archive = tmp_path / "archive"
    archive.mkdir()
    raw = tmp_path / "archive.build" / "alpha" / "raw" / "team.json"
    raw.parent.mkdir(parents=True)
    raw.write_text("{}\n")
    outcomes = _both(archive, ["--output", str(archive)])
    assert "alpha: no legacy glossary cache" in outcomes["py"].stdout
    _both(archive, ["--output", str(archive), "--team", "alpha",
                    "--format", "json"])


def test_audit_glossary_help(rust_wrkviz: Path) -> None:
    oracle = _run([sys.executable, str(PYTHON_WRKVIZ)],
                  ["audit-glossary", "--help"])
    result = _run([str(RUST_WRKVIZ)], ["audit-glossary", "--help"])
    assert result.returncode == oracle.returncode == 0
    assert result.stdout == oracle.stdout
    assert result.stderr == oracle.stderr
