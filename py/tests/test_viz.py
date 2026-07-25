"""Tests for safe_ci_dag_runner.viz (synthetic DAG)."""

from __future__ import annotations

from safe_ci_dag_runner.model import DagConfig, ResourceHint, Step
from safe_ci_dag_runner.viz import to_ascii, to_dot


def _cfg() -> DagConfig:
    return DagConfig(
        steps=(
            Step("build", "app", "", "true"),
            Step("test", "unit", "", "true", deps=["build.app"]),
            Step("e2e", "a", "", "true", deps=["build.app"], hint=ResourceHint(resources={"browser": 1})),
            Step("e2e", "b", "", "true", deps=["build.app"], hint=ResourceHint(resources={"browser": 1})),
        ),
        resource_caps={"browser": 1},
    )


def test_dot_has_clusters_nodes_and_edges() -> None:
    dot = to_dot(_cfg())
    assert dot.startswith("digraph dag {")
    assert '"build.app" -> "test.unit";' in dot
    assert '"build.app" -> "e2e.a";' in dot
    # cap-1 browser resource -> a dashed serialization edge between the two browser steps
    assert '"e2e.a" -> "e2e.b" [style=dashed' in dot
    assert dot.rstrip().endswith("}")


def test_ascii_shows_layers_deps_and_resources() -> None:
    art = to_ascii(_cfg())
    assert "layer 0:" in art and "layer 1:" in art
    assert "build.app" in art
    assert "<- build.app" in art  # dependent lists its dep
    assert "{browser:1}" in art  # resource demand shown
