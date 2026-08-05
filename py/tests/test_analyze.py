"""Compatibility coverage for the profile-analysis library helper."""

from __future__ import annotations

from pathlib import Path

import safe_ci_dag_runner
from safe_ci_dag_runner import analyze


def test_summarize_remains_library_only_and_filters_ambient_rows(tmp_path: Path) -> None:
    """Preserve the public helper without recreating an undeclared command surface."""
    profile = tmp_path / "profiles.csv"
    profile.write_text(
        "step,inner_jobs,ambient_bucket,duration_s,effective_cores,memory_peak_bytes\n"
        "build.unit,2,quiet,4.0,1.5,100\n"
        "build.unit,2,quiet,2.0,2.0,120\n"
        "build.unit,2,busy,20.0,1.0,500\n",
        encoding="utf-8",
    )

    assert safe_ci_dag_runner.summarize is analyze.summarize
    assert not hasattr(analyze, "main")
    assert analyze.summarize([profile]) == [
        {
            "step": "build.unit",
            "inner_jobs": "2",
            "samples": 2,
            "duration_median_s": 3.0,
            "effective_cores_median": 1.75,
            "memory_peak_max_bytes": 120,
        }
    ]
