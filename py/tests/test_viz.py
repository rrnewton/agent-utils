"""Tests for dagrun.viz (synthetic DAG)."""

from __future__ import annotations

from dagrun.model import DagConfig, ResourceHint, Step
from dagrun.viz import to_ascii, to_dot


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


def test_dot_omits_profiling_when_no_estimates() -> None:
    # An undecorated DAG (no est/rss) renders exactly as before: no "Xs, YMB" and no scaling.
    dot = to_dot(_cfg())
    assert '"build.app" [label="build.app\\n[light]"];' in dot
    assert "max par-spdup" not in dot
    assert "MB" not in dot


def _profiled_cfg() -> DagConfig:
    # a: 30s -> b: 60s = 90s critical path; off-path c: 30s. Serial 120s, ideal speedup 1.3X.
    return DagConfig(
        steps=(
            Step("build", "a", "", "true", hint=ResourceHint(est_duration_s=30.0, rss_baseline_bytes=268_435_456)),
            Step(
                "test",
                "b",
                "",
                "true",
                deps=["build.a"],
                hint=ResourceHint(est_duration_s=60.0, rss_baseline_bytes=3_221_225_472),
            ),
            Step(
                "test",
                "c",
                "",
                "true",
                deps=["build.a"],
                hint=ResourceHint(est_duration_s=30.0, rss_baseline_bytes=1_073_741_824),
            ),
        ),
    )


def test_dot_annotates_profiling_and_scaling() -> None:
    dot = to_dot(_profiled_cfg())
    # Per-node "est-s, RSS-MB" (RSS floored to decimal MB).
    assert '"build.a" [label="build.a\\n[light]\\n30.0s, 268MB"];' in dot
    assert '"test.b" [label="test.b\\n[light]\\n60.0s, 3221MB"];' in dot
    # Graph-title scaling: serial 120 / critpath 90 = 1.3X.
    assert "|  1.3X max par-spdup" in dot
