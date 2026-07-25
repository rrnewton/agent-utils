"""Quiet-bucket per-step scaling summarizer.

Ported from DeepScry's ``scripts/analyze_validate_step_profiles.py`` (and its test
``scripts/test_analyze_validate_step_profiles.py``), stripped of every DeepScry/MTG
specific: it reads the generic per-step profile CSV that a
:class:`safe_ci_dag_runner.protocols.MetricsSink` writes (one row per step per run,
tagged with the ambient load bucket the run observed) and collapses it into a small
scaling table.

The whole point is **contention isolation**: a step's wall-clock and effective-core
numbers are only trustworthy when the box was otherwise idle. Mixing a sample taken on a
loaded machine into the same median would make a fast step look slow purely because a
neighbour was eating the cores. So :func:`summarize` filters to a single ambient bucket
(``"quiet"`` by default) BEFORE aggregating, and only ``ambient="all"`` opts back into the
unfiltered, contention-blind view.

The column names below are the schema contract with the metrics sink; they are the subset
of the sink's per-step row that this summary consumes, named here so the summarizer carries
no dependency on the sink implementation.
"""

from __future__ import annotations

import argparse
import csv
import statistics
import sys
from collections import defaultdict
from collections.abc import Iterable, Mapping, Sequence
from pathlib import Path

__all__ = ["summarize", "main"]

# --- CSV schema the metrics sink writes (the columns this summary reads) ---------------
#: Step identity ("group.job").
COL_STEP = "step"
#: The step's inner (own-command) parallelism width, kept as a string so distinct widths
#: stay distinct scaling rows even when the raw value is non-numeric or blank.
COL_INNER_JOBS = "inner_jobs"
#: Ambient load bucket the run observed ("quiet" / "moderate" / "busy"). The filter key.
COL_AMBIENT = "ambient_bucket"
#: Measured wall-clock seconds for the step.
COL_DURATION_S = "duration_s"
#: Measured effective cores the step actually kept busy.
COL_EFFECTIVE_CORES = "effective_cores"
#: Measured peak resident memory (bytes) for the step.
COL_MEMORY_PEAK_BYTES = "memory_peak_bytes"

#: Sentinel ambient value that disables filtering (aggregate every sample).
AMBIENT_ALL = "all"
#: Default ambient bucket: only samples taken on an otherwise-idle box.
AMBIENT_QUIET = "quiet"

#: Ambient buckets a caller may request on the command line (plus ``AMBIENT_ALL``).
AMBIENT_CHOICES = (AMBIENT_QUIET, "moderate", "busy", AMBIENT_ALL)


def _numeric_column(rows: Sequence[Mapping[str, str]], field: str) -> list[float]:
    """Parse one numeric column across ``rows``, skipping blank/absent cells.

    A cell that is missing or empty (falsy) is dropped rather than parsed, mirroring the
    original tolerant behaviour; a cell that is present but non-numeric raises, so bad data
    is surfaced loudly instead of silently coerced.
    """
    return [float(row[field]) for row in rows if row.get(field)]


def summarize(
    paths: Iterable[Path], ambient: str = AMBIENT_QUIET
) -> list[dict[str, object]]:
    """Collapse per-step profile CSVs into one scaling row per ``(step, inner_jobs)``.

    Rows whose :data:`COL_AMBIENT` bucket does not match ``ambient`` are dropped before
    aggregation, so a loaded-box sample never contaminates the medians of a step that is
    actually fast — unless ``ambient == "all"``, which keeps every sample.

    Each returned row is heterogeneous (strings, ints, floats), hence
    ``dict[str, object]``:

    * ``step`` / ``inner_jobs`` — the group key, verbatim.
    * ``samples`` — how many matching rows fed the group.
    * ``duration_median_s`` / ``effective_cores_median`` — medians of the parseable cells.
    * ``memory_peak_max_bytes`` — worst-case peak RSS across the group (``""`` if none was
      recorded).

    A group with no parseable duration is skipped (it has no scaling signal), but the skip
    is reported on stderr rather than dropped silently (No Silent Failure).
    """
    groups: dict[tuple[str, str], list[Mapping[str, str]]] = defaultdict(list)
    for path in paths:
        with path.open(newline="") as source:
            for row in csv.DictReader(source):
                if ambient != AMBIENT_ALL and row.get(COL_AMBIENT) != ambient:
                    continue
                groups[(row[COL_STEP], row[COL_INNER_JOBS])].append(row)

    result: list[dict[str, object]] = []
    for (step, inner_jobs), rows in sorted(groups.items()):
        durations = _numeric_column(rows, COL_DURATION_S)
        if not durations:
            sys.stderr.write(
                f"[analyze] {step} J={inner_jobs}: {len(rows)} matching sample(s) but no "
                f"parseable {COL_DURATION_S}; dropping from the summary\n"
            )
            continue
        memory = _numeric_column(rows, COL_MEMORY_PEAK_BYTES)
        cores = _numeric_column(rows, COL_EFFECTIVE_CORES)
        result.append(
            {
                "step": step,
                "inner_jobs": inner_jobs,
                "samples": len(rows),
                "duration_median_s": round(statistics.median(durations), 3),
                "effective_cores_median": round(statistics.median(cores), 3),
                "memory_peak_max_bytes": int(max(memory)) if memory else "",
            }
        )
    return result


def main(argv: Sequence[str] | None = None) -> int:
    """CLI: print the quiet-bucket scaling table as a tab-separated grid."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("paths", nargs="+", type=Path, help="per-step profile CSV files")
    parser.add_argument(
        "--ambient",
        choices=AMBIENT_CHOICES,
        default=AMBIENT_QUIET,
        help="ambient load bucket to isolate ('all' disables filtering)",
    )
    args = parser.parse_args(argv)
    rows = summarize(args.paths, args.ambient)
    print("step\tJ\tsamples\tmedian_wall_s\tmedian_effective_cores\tmax_memory_bytes")
    for row in rows:
        print(
            f"{row['step']}\t{row['inner_jobs']}\t{row['samples']}\t"
            f"{row['duration_median_s']}\t{row['effective_cores_median']}\t"
            f"{row['memory_peak_max_bytes']}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
