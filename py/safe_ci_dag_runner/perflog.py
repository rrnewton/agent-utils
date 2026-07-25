"""Always-on perf logging for a DAG run: whole-run window + per-step profile CSVs.

Ported from DeepScry's ``scripts/validate_perflog.py`` (ds-1190), stripped of every
DeepScry/MTG specific. The generic contract:

* :class:`PerfWindow` brackets the whole run and appends ONE row recording how long the
  run took and *who* was using the CPU during the window. The contention split
  (``pct_we`` / ``pct_other`` / ``total_busy_pct``) is derived from ``/proc/stat`` sampled
  at window start and end vs. ``RUSAGE_CHILDREN`` CPU-seconds, so an uncontended run (we
  had the box) is distinguishable from a contended one (other work ate the cores).
* :func:`append_step_profiles` appends per-step cgroup measurement rows to a
  machine/container-specific CSV using the self-migrating :data:`STEP_PROFILE_COLUMNS`
  schema.
* :class:`CsvMetricsSink` is a concrete file-backed implementation of the
  :class:`~safe_ci_dag_runner.protocols.MetricsSink` protocol wiring the two together.

Key generalization over the DeepScry original: the DeepScry code GUESSES the output
directory by walking two levels up from the project checkout (``<parent>/validate_perf/``)
and only writes when that opt-in directory already exists. That heuristic is a
DeepScry-harness-layout specific and is removed here. The output directory is an EXPLICIT
constructor / function argument; the caller decides where the data lives. The directory is
created on demand (``mkdir(parents=True)``); a failure to create or write it is surfaced as
a visible warning and returns ``None`` (No Silent Failure) rather than being swallowed.

Two further DeepScry specifics dropped: the whole-run ``machine_id().csv`` layout and the
row context read from DeepScry-named environment variables (``VALIDATE_PROFILE_BASE_SHA``,
``VALIDATE_CPU_ENFORCEMENT_KIND``, ``RUNNER_NAME``) are replaced by plain constructor
arguments with neutral defaults.

Linux cgroup-v2 only for :func:`container_class` (matches the DeepScry target).
"""

from __future__ import annotations

import csv
import fcntl
import os
import re
import resource
import sys
import time
from collections.abc import Mapping, Sequence
from pathlib import Path

from safe_ci_dag_runner.protocols import MetricsSink, RunWindow

__all__ = [
    "CSV_COLUMNS",
    "STEP_PROFILE_COLUMNS",
    "machine_id",
    "nproc",
    "container_class",
    "append_step_profiles",
    "PerfWindow",
    "CsvMetricsSink",
]

# ---------------------------------------------------------------------------
# On-disk schemas. Append-only: never reorder or remove a column (old rows would
# misalign); only append new columns at the end.
# ---------------------------------------------------------------------------

#: Whole-run summary columns, one row per run.
CSV_COLUMNS = [
    "timestamp",       # ISO-8601 local time the run finished
    "machine_id",      # CPU model, run-benchmark-style (spaces->_, non-alnum stripped)
    "git_sha",         # source revision at run time
    "nproc",           # cores the run could use (len(sched_getaffinity))
    "wall_s",          # wall-clock seconds of the run window
    "user_s",          # delta RUSAGE_CHILDREN user CPU-seconds
    "sys_s",           # delta RUSAGE_CHILDREN system CPU-seconds
    "result",          # run outcome verbatim (e.g. "pass" | "fail")
    "n_steps",         # number of steps run
    "pct_we",          # our (user+sys) CPU / (nproc*wall)
    "pct_other",       # other-activity CPU / (nproc*wall)
    "total_busy_pct",  # total system busy CPU / (nproc*wall)
    "ipc",             # optional instructions-per-cycle (blank when unavailable)
    "cache_miss_pct",  # optional cache-miss rate % (blank when unavailable)
    "jobs",            # effective scheduler fan-out (-j)
]

#: Per-step profile columns, keyed by ``machine_id`` + ``container_class``.
STEP_PROFILE_COLUMNS = [
    "timestamp", "machine_id", "container_class", "git_sha", "outer_jobs",
    "step", "classification", "inner_jobs", "duration_s", "effective_cores",
    "quota_utilization_pct", "throttled_s", "memory_peak_bytes", "thread_peak",
    "load1_start", "load1_end", "load5_start", "load5_end",
    "external_cpu_s", "external_cores", "co_tenants_start", "co_tenants_end",
    "ambient_bucket",
    "host_cpu_psi_avg10_start", "host_cpu_psi_avg10_end",
    "host_cpu_psi_avg60_start", "host_cpu_psi_avg60_end",
    "host_memory_psi_avg10_start", "host_memory_psi_avg10_end",
    "host_memory_psi_avg60_start", "host_memory_psi_avg60_end",
    "host_io_psi_avg10_start", "host_io_psi_avg10_end",
    "host_io_psi_avg60_start", "host_io_psi_avg60_end",
    "step_cpu_psi_avg10_start", "step_cpu_psi_avg10_end",
    "step_cpu_psi_avg60_start", "step_cpu_psi_avg60_end",
    "profile_base_sha",
    "enforcement_kind",
    "runner_name",
]


# ---------------------------------------------------------------------------
# Machine / container identity + system-wide CPU sampling.
# ---------------------------------------------------------------------------


def machine_id() -> str:
    """Per-machine identifier from ``/proc/cpuinfo`` 'model name' (spaces -> underscores,
    then non ``[A-Za-z0-9_-]`` stripped). Falls back to the hostname, then ``"unknown"``.
    Mirrors run_benchmark-style CPU naming so rows group cleanly per machine."""
    try:
        with open("/proc/cpuinfo") as f:
            for line in f:
                if line.startswith("model name"):
                    name = line.split(":", 1)[1].strip().replace(" ", "_")
                    return re.sub(r"[^A-Za-z0-9_-]", "", name)
    except OSError:
        pass
    return re.sub(r"[^A-Za-z0-9_-]", "", os.uname().nodename) or "unknown"


def nproc() -> int:
    """Cores this run could actually use (respects CPU affinity / cgroup pin)."""
    try:
        return len(os.sched_getaffinity(0))
    except (AttributeError, OSError):
        return os.cpu_count() or 1


def container_class() -> str:
    """Stable CPU-container key: affinity width plus the cgroup-v2 CPU quota
    (``/sys/fs/cgroup/cpu.max``). cgroup-v2 only, matching the target host."""
    quota = "unknown"
    try:
        quota = Path("/sys/fs/cgroup/cpu.max").read_text().strip().replace(" ", "_")
    except OSError:
        pass
    return f"affinity{nproc()}_cpu-max-{quota}"


def _proc_stat_busy_jiffies() -> int | None:
    """Sum of all non-idle jiffies from the aggregate 'cpu ' line of ``/proc/stat``.
    Busy = everything except idle + iowait (iowait is not CPU-burning work). ``None`` when
    ``/proc/stat`` is unreadable or malformed."""
    try:
        with open("/proc/stat") as f:
            first = f.readline()
    except OSError:
        return None
    parts = first.split()
    if not parts or parts[0] != "cpu":
        return None
    vals = [int(x) for x in parts[1:]]
    idle = vals[3] if len(vals) > 3 else 0      # idle jiffies
    iowait = vals[4] if len(vals) > 4 else 0    # iowait jiffies (when present)
    return sum(vals) - idle - iowait


# ---------------------------------------------------------------------------
# CSV append helpers (self-migrating schemas).
# ---------------------------------------------------------------------------


def _ensure_dir(output_dir: str | Path) -> Path | None:
    """Create ``output_dir`` on demand and return it, or warn + return ``None`` on failure.

    Unlike the DeepScry original (which required a pre-existing opt-in directory two levels
    up from the checkout), the directory is explicit and created eagerly; an inability to
    create it is a visible warning, never a silent skip."""
    path = Path(output_dir)
    try:
        path.mkdir(parents=True, exist_ok=True)
    except OSError as exc:
        sys.stderr.write(f"[perflog] skipped: cannot create output dir {path} ({exc})\n")
        return None
    return path


def append_step_profiles(
    output_dir: str | Path,
    rows: Sequence[Mapping[str, object]],
    *,
    git_sha: str,
    outer_jobs: int,
    profile_base_sha: str | None = None,
    enforcement_kind: str = "unverified",
    runner_name: str = "local",
) -> Path | None:
    """Append per-step cgroup measurement ``rows`` to a machine/container-specific CSV.

    The file is ``<output_dir>/step_profiles_<machine_id>_<container_class>.csv``. Each row
    is a heterogeneous column->value mapping owned by the caller; the shared run context
    (timestamp, machine id, container class, git SHA, outer job count, and the
    provenance columns) is stamped onto every row here. Writes are serialized with an
    ``flock`` sidecar so concurrent runs on one machine do not interleave.

    The schema self-migrates: if an existing file's header differs from
    :data:`STEP_PROFILE_COLUMNS`, it is rewritten to the current column set (dropping
    unknown columns, filling absent ones blank) before appending. Returns the CSV path, or
    ``None`` if the output directory could not be created (a visible warning is emitted).

    ``profile_base_sha`` defaults to ``git_sha``. ``enforcement_kind`` / ``runner_name``
    are recorded verbatim (neutral defaults replace the DeepScry env-var reads)."""
    logs_dir = _ensure_dir(output_dir)
    if logs_dir is None:
        return None
    path = logs_dir / f"step_profiles_{machine_id()}_{container_class()}.csv"
    lock_path = path.with_suffix(path.suffix + ".lock")
    with open(lock_path, "w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        _migrate_step_profile_schema(path)
        new = not path.exists() or path.stat().st_size == 0
        with open(path, "a", newline="") as f:
            writer = csv.DictWriter(f, fieldnames=STEP_PROFILE_COLUMNS)
            if new:
                writer.writeheader()
            common: dict[str, object] = {
                "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S"),
                "machine_id": machine_id(),
                "container_class": container_class(),
                "git_sha": git_sha,
                "outer_jobs": outer_jobs,
                "profile_base_sha": git_sha if profile_base_sha is None else profile_base_sha,
                "enforcement_kind": enforcement_kind,
                "runner_name": runner_name,
            }
            for row in rows:
                writer.writerow({**common, **row})
    return path


def _migrate_step_profile_schema(path: Path) -> None:
    """Rewrite ``path`` in place to the exact :data:`STEP_PROFILE_COLUMNS` header when its
    current header differs (append-column or column-drop drift). No-op when the file is
    absent, empty, or already current. Caller holds the ``flock``."""
    if not (path.exists() and path.stat().st_size):
        return
    with open(path, newline="") as existing:
        reader = csv.DictReader(existing)
        if (reader.fieldnames or []) == STEP_PROFILE_COLUMNS:
            return
        old_rows = list(reader)
    tmp = path.with_suffix(path.suffix + ".tmp")
    with open(tmp, "w", newline="") as migrated:
        writer = csv.DictWriter(migrated, fieldnames=STEP_PROFILE_COLUMNS)
        writer.writeheader()
        for old_row in old_rows:
            old_row.pop(None, None)  # drop csv restkey overflow
            writer.writerow({name: old_row.get(name, "") for name in STEP_PROFILE_COLUMNS})
    os.replace(tmp, path)


def _append_whole_run_row(csv_path: Path, row: Mapping[str, object]) -> None:
    """Append one whole-run summary ``row`` to ``csv_path`` under the additive
    :data:`CSV_COLUMNS` schema. Unlike the per-step schema (which is rewritten to the exact
    column set), the whole-run schema MERGES any pre-existing columns with the current ones
    so older, wider files keep their extra columns intact."""
    new = not csv_path.exists()
    if not new:
        with open(csv_path, newline="") as f:
            old_fields = list(csv.DictReader(f).fieldnames or [])
        if old_fields != CSV_COLUMNS:
            merged_fields = list(old_fields)
            for col in CSV_COLUMNS:
                if col not in merged_fields:
                    merged_fields.append(col)
            with open(csv_path, newline="") as f:
                old_rows = list(csv.DictReader(f))
            tmp = csv_path.with_suffix(csv_path.suffix + ".tmp")
            with open(tmp, "w", newline="") as out:
                w = csv.DictWriter(out, fieldnames=merged_fields)
                w.writeheader()
                for old_row in old_rows:
                    old_row.pop(None, None)
                    w.writerow({col: old_row.get(col, "") for col in merged_fields})
            os.replace(tmp, csv_path)
    with open(csv_path, newline="") as existing:
        fieldnames = list(csv.DictReader(existing).fieldnames or []) if not new else CSV_COLUMNS
    with open(csv_path, "a", newline="") as f:
        w = csv.DictWriter(f, fieldnames=fieldnames)
        if new:
            w.writeheader()
        w.writerow({col: row.get(col, "") for col in fieldnames})


# ---------------------------------------------------------------------------
# Whole-run window.
# ---------------------------------------------------------------------------


class PerfWindow:
    """A started whole-run measurement bracket; :meth:`finish` appends one summary row.

    Concrete implementation of :class:`~safe_ci_dag_runner.protocols.RunWindow`. Construct
    via :meth:`start` (captures baseline wall clock, ``RUSAGE_CHILDREN`` CPU, and
    system-wide busy jiffies), then call :meth:`finish` once the DAG completes. The row is
    written to ``<output_dir>/<machine_id>.csv``. The summary row is appended regardless of
    ``result`` (both passing and failing runs are recorded)."""

    def __init__(
        self,
        *,
        output_dir: str | Path,
        git_sha: str,
        machine_id_value: str,
        wall_start: float,
        ru_start: resource.struct_rusage,
        busy_start: int | None,
        ipc: object = "",
        cache_miss_pct: object = "",
    ) -> None:
        self.output_dir = output_dir
        self.git_sha = git_sha
        self.machine_id_value = machine_id_value
        self.wall_start = wall_start
        self.ru_start = ru_start
        self.busy_start = busy_start
        self.ipc = ipc
        self.cache_miss_pct = cache_miss_pct

    @classmethod
    def start(
        cls,
        *,
        output_dir: str | Path,
        git_sha: str,
        machine_id_value: str | None = None,
        ipc: object = "",
        cache_miss_pct: object = "",
    ) -> "PerfWindow":
        """Open the window, capturing baseline counters at the current instant."""
        return cls(
            output_dir=output_dir,
            git_sha=git_sha,
            machine_id_value=machine_id() if machine_id_value is None else machine_id_value,
            wall_start=time.time(),
            ru_start=resource.getrusage(resource.RUSAGE_CHILDREN),
            busy_start=_proc_stat_busy_jiffies(),
            ipc=ipc,
            cache_miss_pct=cache_miss_pct,
        )

    def finish(self, *, result: str, n_steps: int, jobs: int) -> Mapping[str, object] | None:
        """Compute window metrics and append the summary row.

        Derives the contention split from the baseline captured at :meth:`start`:

        * ``pct_we``          = our (user+sys child CPU-seconds) / (nproc * wall)
        * ``pct_other``       = (system busy CPU-seconds - our CPU-seconds) / (nproc * wall)
        * ``total_busy_pct``  = system busy CPU-seconds / (nproc * wall)

        ``pct_other`` / ``total_busy_pct`` are blank when ``/proc/stat`` could not be
        sampled at both ends. Returns the recorded row for logging, or ``None`` when the
        output directory could not be created / written (a visible warning is emitted).
        Never raises into the run path — metrics recording must not fail a run."""
        try:
            wall = max(time.time() - self.wall_start, 0.0)
            ru_end = resource.getrusage(resource.RUSAGE_CHILDREN)
            user_s = max(ru_end.ru_utime - self.ru_start.ru_utime, 0.0)
            sys_s = max(ru_end.ru_stime - self.ru_start.ru_stime, 0.0)
            ncpu = nproc()
            capacity = ncpu * wall if wall > 0 else 0.0
            our_cpu = user_s + sys_s

            total_busy_s: float | None = None
            busy_end = _proc_stat_busy_jiffies()
            if self.busy_start is not None and busy_end is not None:
                clk = os.sysconf("SC_CLK_TCK") or 100
                total_busy_s = max(busy_end - self.busy_start, 0) / clk

            def pct(x: float) -> float | str:
                return round(100.0 * x / capacity, 2) if capacity > 0 else ""

            pct_we = pct(our_cpu)
            if total_busy_s is not None:
                pct_other: float | str = pct(max(total_busy_s - our_cpu, 0.0))
                total_busy_pct: float | str = pct(total_busy_s)
            else:
                pct_other = ""
                total_busy_pct = ""

            row: dict[str, object] = {
                "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S"),
                "machine_id": self.machine_id_value,
                "git_sha": self.git_sha,
                "nproc": ncpu,
                "wall_s": round(wall, 1),
                "user_s": round(user_s, 1),
                "sys_s": round(sys_s, 1),
                "result": result,
                "n_steps": n_steps,
                "pct_we": pct_we,
                "pct_other": pct_other,
                "total_busy_pct": total_busy_pct,
                "ipc": self.ipc,
                "cache_miss_pct": self.cache_miss_pct,
                "jobs": jobs,
            }

            logs_dir = _ensure_dir(self.output_dir)
            if logs_dir is None:
                return None
            _append_whole_run_row(logs_dir / f"{self.machine_id_value}.csv", row)
            return row
        except Exception as exc:  # never break the run over perf logging
            try:
                sys.stderr.write(f"[perflog] whole-run row skipped ({exc})\n")
            except Exception:
                pass
            return None


# ---------------------------------------------------------------------------
# Concrete MetricsSink.
# ---------------------------------------------------------------------------


class CsvMetricsSink(MetricsSink):
    """File-backed :class:`~safe_ci_dag_runner.protocols.MetricsSink` writing CSV rows.

    Carries the fixed run context (explicit output directory, git SHA, machine id, and the
    per-step provenance columns) as construction state; the scheduler passes only the
    varying per-call data. The output directory is an EXPLICIT argument — the generalization
    over DeepScry, which guessed it from the checkout layout — and is created on demand."""

    def __init__(
        self,
        output_dir: str | Path,
        *,
        git_sha: str,
        machine_id_value: str | None = None,
        profile_base_sha: str | None = None,
        enforcement_kind: str = "unverified",
        runner_name: str = "local",
        ipc: object = "",
        cache_miss_pct: object = "",
    ) -> None:
        self.output_dir = output_dir
        self.git_sha = git_sha
        self.machine_id_value = machine_id() if machine_id_value is None else machine_id_value
        self.profile_base_sha = profile_base_sha
        self.enforcement_kind = enforcement_kind
        self.runner_name = runner_name
        self.ipc = ipc
        self.cache_miss_pct = cache_miss_pct

    def start_run_window(self) -> RunWindow:
        """Open the whole-run window bound to this sink's context."""
        return PerfWindow.start(
            output_dir=self.output_dir,
            git_sha=self.git_sha,
            machine_id_value=self.machine_id_value,
            ipc=self.ipc,
            cache_miss_pct=self.cache_miss_pct,
        )

    def record_step_profiles(
        self, rows: Sequence[Mapping[str, object]], *, jobs: int
    ) -> str | None:
        """Append per-step profile ``rows`` (accumulated during the run) to durable storage.

        Delegates to :func:`append_step_profiles`; ``jobs`` is stamped as ``outer_jobs`` on
        every row. Rows are recorded regardless of run outcome. Returns the CSV path as a
        string, or ``None`` when recording was skipped (a visible warning is emitted)."""
        path = append_step_profiles(
            self.output_dir,
            rows,
            git_sha=self.git_sha,
            outer_jobs=jobs,
            profile_base_sha=self.profile_base_sha,
            enforcement_kind=self.enforcement_kind,
            runner_name=self.runner_name,
        )
        return None if path is None else str(path)
