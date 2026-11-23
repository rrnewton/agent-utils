"""Unit tests for common/bin/wrkslots-shadow (reader shadow compare).

#562 rust-primary-wrkslots: the helper decides agree/diverge/skipped for a
served-Python vs shadow-Rust reader pair and appends one JSONL line. A skip
is never an agreement.
"""

from __future__ import annotations

import importlib.machinery
import importlib.util
import json
from pathlib import Path
from types import ModuleType


AU_ROOT = Path(__file__).resolve().parents[2]


def _load_helper() -> ModuleType:
    path = AU_ROOT / "common" / "bin" / "wrkslots-shadow"
    loader = importlib.machinery.SourceFileLoader("wrkslots_shadow", str(path))
    spec = importlib.util.spec_from_loader(loader.name, loader)
    assert spec is not None
    module = importlib.util.module_from_spec(spec)
    loader.exec_module(module)
    return module


HELPER = _load_helper()


def _compare(**overrides):
    base = dict(
        py_rc=0,
        rs_rc=0,
        rs_timed_out=False,
        py_out='{"active": [], "heartbeat_age_seconds": 3}',
        rs_out='{"heartbeat_age_seconds": 9, "active": []}',
        rs_err="",
        fp_before="abc",
        fp_after="abc",
    )
    base.update(overrides)
    return HELPER.compare(**base)


def test_agree_after_scrub_and_key_order() -> None:
    assert _compare() == {"result": "agree"}


def test_first_divergent_field_is_named() -> None:
    result = _compare(rs_out='{"active": [{"owner_state": "dead"}]}')
    assert result["result"] == "diverge"
    assert result["field"] == "active"


def test_nested_field_path() -> None:
    result = _compare(
        py_out='{"active": [{"a": 1, "b": 2}]}',
        rs_out='{"active": [{"a": 1, "b": 3}]}',
    )
    assert result["field"] == "active[0].b"
    assert result["py"] == "2"
    assert result["rs"] == "3"


def test_registry_change_is_a_skip_not_agreement() -> None:
    assert _compare(fp_after="xyz") == {
        "result": "skipped",
        "reason": "registry-changed",
    }
    assert _compare(fp_before="none", fp_after="none")["reason"] == "no-fingerprint"


def test_timeout_and_rs_refusal_classes() -> None:
    assert _compare(rs_rc=124, rs_timed_out=True)["reason"] == "timeout"
    # A tool's own 124 without the timeout flag is an ordinary rc divergence.
    assert _compare(rs_rc=124)["result"] == "diverge"
    assert _compare(py_rc=130)["reason"] == "served-signal"
    refused = _compare(rs_err="'status' stays with the Python edition in this slice")
    assert refused["result"] == "diverge"
    assert refused["reason"] == "rs-refused"


def test_rc_mismatch_diverges() -> None:
    result = _compare(rs_rc=3)
    assert result["result"] == "diverge"
    assert result["field"] == "<rc>"


def test_text_compare_scrubs_ages() -> None:
    result = _compare(py_out="slot-a heartbeat 3s ago\n", rs_out="slot-a heartbeat 8s ago\n")
    assert result == {"result": "agree"}
    # The real human spellings readers emit.
    assert _compare(
        py_out="slot-a heartbeat_age=5s\n", rs_out="slot-a heartbeat_age=6s\n"
    ) == {"result": "agree"}
    assert _compare(
        py_out="journal age=10s\n", rs_out="journal age=11s\n"
    ) == {"result": "agree"}


def test_compare_scrubs_doctor_age_in_both_formats() -> None:
    # The verbatim doctor finding both editions emit for a quarantined journal.
    finding = (
        "quarantined journal {journal} retained for inspection (age {age}); "
        "report-only in v1, hand restore is in the user guide"
    )
    py_finding = finding.format(journal="j1", age="10s")
    rs_finding = finding.format(journal="j1", age="11s")
    assert _compare(py_out=py_finding + "\n", rs_out=rs_finding + "\n") == {
        "result": "agree"
    }
    assert _compare(
        py_out=json.dumps({"findings": [{"detail": py_finding}]}),
        rs_out=json.dumps({"findings": [{"detail": rs_finding}]}),
    ) == {"result": "agree"}


def test_scrub_is_an_explicit_set_not_a_suffix() -> None:
    # Bare age_seconds (interrupted_journals) is volatile and scrubbed...
    assert _compare(
        py_out='{"interrupted_journals": [{"age_seconds": 10}]}',
        rs_out='{"interrupted_journals": [{"age_seconds": 11}]}',
    ) == {"result": "agree"}
    # ...but a non-volatile *_age_seconds threshold is real signal.
    assert _compare(
        py_out='{"evidence_max_age_seconds": 100}',
        rs_out='{"evidence_max_age_seconds": 999}',
    )["result"] == "diverge"
    # bool is not a number for divergence purposes.
    assert _compare(py_out='{"a": true}', rs_out='{"a": 1}')["result"] == "diverge"


def _project(tmp_path: Path, config: str, control_rel: str) -> Path:
    project = tmp_path / "proj"
    control = project / control_rel
    control.mkdir(parents=True)
    (project / ".wrkslots.yml").write_text(config)
    (control / "ACTIVE.node-a.json").write_text("{}\n")
    return project


def test_fingerprint_layouts_and_config_shapes(tmp_path: Path) -> None:
    cases = [
        # nested + worktrees/slots: control is the PARENT (slots special case)
        ("worktrees_dir: worktrees/slots\n", "worktrees"),
        # flat + worktrees: control is the project root
        ("worktrees_dir: worktrees\nlayout: flat\n", "."),
        # JSON document, non-default values
        ('{"worktrees_dir": "wt", "layout": "nested"}', "wt"),
        # quoted YAML scalar with a trailing comment
        ('worktrees_dir: "worktrees" # control lives here\n', "worktrees"),
    ]
    for index, (config, control_rel) in enumerate(cases):
        root = tmp_path / f"case-{index}"
        root.mkdir()
        project = _project(root, config, control_rel)
        first = HELPER.fingerprint(str(project))
        assert first != "none", config
        active = next((project / control_rel).glob("ACTIVE.*.json"))
        active.write_text('{"revision": 2}\n')
        assert HELPER.fingerprint(str(project)) != first, config


def test_fingerprint_reads_registry(tmp_path: Path) -> None:
    project = tmp_path / "proj"
    control = project / "worktrees"
    control.mkdir(parents=True)
    (project / ".wrkslots.yml").write_text("schema: 2\nworktrees_dir: worktrees\n")
    assert HELPER.fingerprint(str(project)) != "none"
    active = control / "ACTIVE.node-a.json"
    active.write_text("{}\n")
    first = HELPER.fingerprint(str(project))
    active.write_text('{"revision": 2}\n')
    assert HELPER.fingerprint(str(project)) != first
    assert HELPER.fingerprint(str(tmp_path / "missing")) == "none"


def test_main_compare_writes_one_jsonl_line(tmp_path: Path) -> None:
    log = tmp_path / "divergence.jsonl"
    py_out = tmp_path / "py.out"
    rs_out = tmp_path / "rs.out"
    empty = tmp_path / "empty"
    py_out.write_text('{"a": 1}')
    rs_out.write_text('{"a": 2}')
    empty.write_text("")
    rc = HELPER.main(
        [
            "compare",
            "--cmd", "status",
            "--argv-sha", "deadbeef",
            "--py-rc", "0",
            "--rs-rc", "0",
            "--py-out", str(py_out),
            "--rs-out", str(rs_out),
            "--py-err", str(empty),
            "--rs-err", str(empty),
            "--fp-before", "abc",
            "--fp-after", "abc",
            "--py-ms", "10",
            "--rs-ms", "2",
            "--log", str(log),
        ]
    )
    assert rc == 0
    lines = log.read_text().splitlines()
    assert len(lines) == 1
    record = json.loads(lines[0])
    assert record["result"] == "diverge"
    assert record["field"] == "a"
    assert record["served"] == "python"
    assert record["cmd"] == "status"
