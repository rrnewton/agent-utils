"""Tests for YAML interchange: YAML is ISOMORPHIC to the JSON schema.

The YAML loader funnels through the same strict narrowing as JSON, so a YAML document and the
equivalent JSON document must build byte-identical canonical JSON. These tests also pin the
adversarial cases (block scalars and the quoted Norway-problem tokens) so a parser change that
diverged from the schema would fail loudly.
"""

from __future__ import annotations

from safe_ci_dag_runner.io import dag_from_json, dag_from_yaml, dag_to_json, dag_to_yaml

# A YAML document exercising: a literal (|-) block-scalar description with quotes + unicode, a
# folded (>-) block scalar, a QUOTED Norway-problem token ("no") that must stay the STRING "no"
# (not the bool false), a quoted number-like string, and YAML comments.
_YAML = """
# top-level description as a folded block scalar (newlines fold to spaces)
description: >-
  the whole
  pipeline
resource_caps:
  browser: 1
steps:
  - group: build
    job: app
    desc: compile               # short label
    description: |-             # literal block scalar: newlines preserved, no trailing newline
      line 1
      line 2 with "quotes" and unicode é☃
    cmd: make build
    hint:
      classification: cpu-bound
      preferred_inner_jobs: 8
  - group: e2e
    job: smoke
    desc: browser
    description: "no"            # quoted Norway token -> string, not bool false
    cmd: make e2e
    deps: [build.app]
    hint:
      resources:
        browser: 1
"""

# The JSON document that MUST load to the same DagConfig. Note ">- the whole\\n  pipeline" folds
# to "the whole pipeline" (single space, no trailing newline).
_JSON = (
    '{"description": "the whole pipeline", "resource_caps": {"browser": 1}, "steps": ['
    '{"group": "build", "job": "app", "desc": "compile",'
    ' "description": "line 1\\nline 2 with \\"quotes\\" and unicode é☃",'
    ' "cmd": "make build",'
    ' "hint": {"classification": "cpu-bound", "preferred_inner_jobs": 8}},'
    '{"group": "e2e", "job": "smoke", "desc": "browser", "description": "no",'
    ' "cmd": "make e2e", "deps": ["build.app"], "hint": {"resources": {"browser": 1}}}]}'
)


def test_yaml_isomorphic_to_json() -> None:
    from_yaml = dag_from_yaml(_YAML)
    from_json = dag_from_json(_JSON)
    # Isomorphism: identical canonical JSON regardless of input syntax.
    assert dag_to_json(from_yaml) == dag_to_json(from_json)
    # The quoted Norway token stayed a string; the literal block scalar chomped correctly.
    assert from_yaml.steps[1].description == "no"
    assert from_yaml.steps[0].description == 'line 1\nline 2 with "quotes" and unicode é☃'
    assert from_yaml.description == "the whole pipeline"


def test_yaml_emit_round_trips() -> None:
    cfg = dag_from_json(
        '{"description": "d", "steps": [{"group": "g", "job": "j", "desc": "x",'
        ' "description": "multi\\nline", "cmd": "true"}]}'
    )
    # dag_to_yaml output need not match Rust, but must round-trip back to the same DagConfig.
    back = dag_from_yaml(dag_to_yaml(cfg))
    assert dag_to_json(back) == dag_to_json(cfg)
