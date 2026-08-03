"""Tests for safe_ci_dag_runner.io (JSON round-trip + strict parse errors)."""

from __future__ import annotations

from safe_ci_dag_runner.io import DagJsonError, dag_from_json, dag_to_json
from safe_ci_dag_runner.model import (
    DagConfig,
    ResourceHint,
    Step,
    StepClass,
    resolved_wall_timeout,
)


#: Multi-line description with quotes, backslashes, a tab, and unicode — proves the JSON string
#: escaping survives a round-trip (and, in the cross harness, is byte-identical to Rust).
_HAIRY_DESC = 'line 1\nline 2 with "quotes" and \\backslash\\ and\ttab and unicode é☃'


def _cfg() -> DagConfig:
    return DagConfig(
        steps=(
            Step(
                "build",
                "app",
                "compile",
                "make build",
                description=_HAIRY_DESC,
                hint=ResourceHint(
                    est_duration_s=90.0,
                    rss_baseline_bytes=5 * 1024**3,
                    classification=StepClass.CPU_BOUND,
                    preferred_inner_jobs=8,
                ),
            ),
            Step(
                "e2e",
                "smoke",
                "browser",
                "make e2e",
                deps=["build.app"],
                env={"HEADLESS": "1"},
                hint=ResourceHint(resources={"browser": 1}, classification=StepClass.LATENCY_BOUND),
            ),
        ),
        description="the whole pipeline",
        resource_caps={"browser": 2},
        mem_cap_factor=1.25,
        outer_mem_safety_factor=1.1,
    )


def test_roundtrip_is_stable() -> None:
    cfg = _cfg()
    once = dag_to_json(cfg)
    twice = dag_to_json(dag_from_json(once))
    assert once == twice  # canonical JSON is a fixed point
    back = dag_from_json(once)
    assert [s.tag for s in back.steps] == ["build.app", "e2e.smoke"]
    assert back.description == "the whole pipeline"
    assert back.steps[0].description == _HAIRY_DESC
    assert back.steps[1].description == ""  # default empty
    assert back.resource_caps == {"browser": 2}
    assert back.steps[0].hint.classification is StepClass.CPU_BOUND
    assert back.steps[0].hint.rss_baseline_bytes == 5 * 1024**3
    assert back.steps[1].hint.resources == {"browser": 1}
    assert back.steps[1].env == {"HEADLESS": "1"}


def test_minimal_document_defaults() -> None:
    cfg = dag_from_json('{"steps": [{"group": "g", "job": "j", "cmd": "true"}]}')
    step = cfg.steps[0]
    assert step.tag == "g.j"
    assert step.desc == "" and step.deps == [] and step.env == {}
    assert step.description == "" and cfg.description == ""  # default empty
    # An omitted wall timeout loads as the 0 "unset" sentinel; the effective backstop is
    # resolved lazily and falls back to the doc default (1800 here).
    assert step.timeout == 0
    assert resolved_wall_timeout(step, cfg.default_step_timeout) == 1800
    assert step.cpu_timeout == 0  # CPU-time guard disabled by default
    assert step.hint.classification is StepClass.LIGHT
    assert cfg.resource_caps == {} and cfg.mem_cap_factor == 1.25


def test_cpu_timeout_roundtrip_and_conditional_emit() -> None:
    # A step with a CPU-time budget parses it and round-trips; a step without one omits the
    # key entirely, so existing DAGs stay byte-for-byte unchanged (absence parses back to 0).
    doc = (
        '{"steps": ['
        '{"group": "g", "job": "cpu", "cmd": "true", "cpu_timeout": 30},'
        '{"group": "g", "job": "nocpu", "cmd": "true"}]}'
    )
    cfg = dag_from_json(doc)
    by_tag = cfg.by_tag()
    assert by_tag["g.cpu"].cpu_timeout == 30
    assert by_tag["g.nocpu"].cpu_timeout == 0  # absent -> default 0

    out = dag_to_json(cfg)
    assert '"cpu_timeout": 30' in out  # present only for the step that set it
    # Exactly one step emits the key; the default-0 step must not.
    assert out.count('"cpu_timeout"') == 1
    # Stable across a second serialization pass.
    assert dag_to_json(dag_from_json(out)) == out


def test_default_step_timeout_applied() -> None:
    # A step that omits `timeout` inherits the document-level default_step_timeout; a step
    # with its own timeout keeps it; and the config records the document default.
    doc = (
        '{"default_step_timeout": 42, "steps": ['
        '{"group": "g", "job": "a", "cmd": "true"},'
        '{"group": "g", "job": "b", "cmd": "true", "timeout": 7}]}'
    )
    cfg = dag_from_json(doc)
    by_tag = cfg.by_tag()
    # g.a omits `timeout` -> raw 0 sentinel, resolving to the doc default (42).
    assert by_tag["g.a"].timeout == 0
    assert resolved_wall_timeout(by_tag["g.a"], cfg.default_step_timeout) == 42
    # g.b declares an explicit wall timeout -> preserved verbatim through the resolver.
    assert by_tag["g.b"].timeout == 7
    assert resolved_wall_timeout(by_tag["g.b"], cfg.default_step_timeout) == 7
    assert cfg.default_step_timeout == 42


def test_default_step_timeout_falls_back_to_module_constant() -> None:
    # Without a document-level default, an omitted per-step `timeout` stays the 0 sentinel and
    # resolves to the module DEFAULT_STEP_TIMEOUT via resolved_wall_timeout.
    cfg = dag_from_json('{"steps": [{"group": "g", "job": "a", "cmd": "true"}]}')
    assert cfg.steps[0].timeout == 0
    assert resolved_wall_timeout(cfg.steps[0], cfg.default_step_timeout) == 1800
    assert cfg.default_step_timeout == 1800


def test_wall_timeout_omitted_when_unset_and_roundtrips() -> None:
    # The idiom: a step that leaves `timeout` unset must NOT emit the key (byte-stable), and an
    # explicit wall timeout must round-trip. Mirrors the cpu_timeout conditional-emit contract.
    doc = (
        '{"steps": ['
        '{"group": "g", "job": "wall", "cmd": "true", "timeout": 55},'
        '{"group": "g", "job": "nowall", "cmd": "true"}]}'
    )
    cfg = dag_from_json(doc)
    by_tag = cfg.by_tag()
    assert by_tag["g.wall"].timeout == 55
    assert by_tag["g.nowall"].timeout == 0  # absent -> 0 sentinel

    out = dag_to_json(cfg)
    assert '"timeout": 55' in out  # present only for the step that set it
    assert out.count('"timeout"') == 1  # the unset step omits the key
    assert dag_to_json(dag_from_json(out)) == out  # stable across a second pass


def test_resolved_wall_timeout_precedence() -> None:
    # 1. explicit wall timeout wins verbatim even with a cpu_timeout present;
    # 2. unset wall + cpu_timeout -> 3x hang-catcher backstop;
    # 3. both unset -> document default.
    s = Step("g", "j", "", "true", timeout=900, cpu_timeout=100)
    assert resolved_wall_timeout(s, 1800) == 900
    s = Step("g", "j", "", "true", timeout=0, cpu_timeout=100)
    assert resolved_wall_timeout(s, 1800) == 300
    s = Step("g", "j", "", "true", timeout=0, cpu_timeout=0)
    assert resolved_wall_timeout(s, 1800) == 1800
    assert resolved_wall_timeout(s, 42) == 42


def test_strict_parse_errors() -> None:
    bad_docs = [
        "not json at all",
        "[]",  # root not an object
        '{"steps": "not a list"}',
        '{"steps": [{"job": "j", "cmd": "c"}]}',  # missing group
        '{"steps": [{"group": "g", "job": "j"}]}',  # missing cmd
        '{"steps": [{"group": "g", "job": "j", "cmd": "c", "timeout": "x"}]}',
        '{"steps": [{"group": "g", "job": "j", "cmd": "c", "deps": [1]}]}',
        '{"steps": [{"group": "g", "job": "j", "cmd": "c", "hint": {"classification": "nope"}}]}',
        '{"steps": [{"group": "g", "job": "j", "cmd": "c", "hint": {"resources": {"x": "y"}}}]}',
    ]
    for doc in bad_docs:
        raised = False
        try:
            dag_from_json(doc)
        except DagJsonError:
            raised = True
        assert raised, f"expected DagJsonError for: {doc!r}"
