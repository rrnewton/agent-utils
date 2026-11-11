#!/usr/bin/env python3
"""Daily gate success-rate report (#1246 gate-success-rate).

Reads two read-only sources and summarises one America/New_York day:

* the admission history (default ``/tmp/dev-agent-utils-gate-admission/history.jsonl``,
  override with ``--history`` or ``$GATE_STATS_HISTORY``): one line per finished
  ticket with ``class`` (shared/exclusive), ``priority``, ``queued_at``,
  ``wait_s`` and ``run_s``;
* the durable gate ledger (default ``~/.local/state/agent-utils/gate-ledger.jsonl``,
  override with ``--ledger`` or ``$GATE_LEDGER``): one line per finished run
  with ``head_sha``, ``kind`` (gate / exclusive-rerun / preflight-refused),
  ``preflight``, ``verdict`` and ``failed_steps``.

Definitions (kept stable so days are comparable):

* Day assignment is by queued time in America/New_York (history ``queued_at``;
  ledger ``queued_utc``, falling back to ``started_utc``).
* Tier is ``owner`` when ``priority == "owner"``, else ``normal``.
* First-try pass: for each ``head_sha``, its first full gate (``kind=gate``,
  aborted gates excluded) in the whole ledger; green means that first gate's
  verdict is ``pass``. A head is counted on the day of that first gate.
* Flake: a step in a full gate's ``failed_steps`` where the same head later
  has a passing solo rerun (``kind=exclusive-rerun``, ``rerun_passed`` true).
  Rerun rows do not name their steps, so this attributes the rerun's pass to
  the failed steps of that head's gate; it is a proxy and is labelled as such.
* Red gate: a full gate whose verdict is ``fail`` (``partial`` is counted
  separately and is not green). Red-batch rate is null until batching writes
  batch records; the column is kept ready.
* Percentiles: p50 is the median; p90 is nearest-rank.

Usage:
    python3 scripts/gate_stats.py --day 2026-11-09
    python3 scripts/gate_stats.py --day 2026-11-09 --json
    python3 scripts/gate_stats.py --day 2026-11-09 --markdown --output /tmp/report.md
"""

from __future__ import annotations

import argparse
import json
import math
import os
import statistics
import sys
from collections import Counter, defaultdict
from datetime import datetime, time, timedelta
from pathlib import Path
from typing import Callable, Optional, TypedDict
from zoneinfo import ZoneInfo

ET = ZoneInfo("America/New_York")
DEFAULT_HISTORY = "/tmp/dev-agent-utils-gate-admission/history.jsonl"
DEFAULT_LEDGER = "~/.local/state/agent-utils/gate-ledger.jsonl"
Row = dict[str, object]
InDay = Callable[[Optional[float]], bool]


class Dist(TypedDict):
    n: int
    p50_s: float | None
    p90_s: float | None
    sum_s: float


Split = TypedDict("Split", {"tier": str, "class": str, "tickets": int, "wait": Dist,
                            "run": Dist, "red_tickets": int, "red_rate": Optional[float]})


class Overall(TypedDict):
    tickets: int
    shared_tickets: int
    exclusive_tickets: int
    wait: Dist
    run: Dist
    gates_per_hour: float
    exclusive_host_hours: float


GatesBlock = TypedDict("GatesBlock", {"full_gates": int, "pass": int, "fail": int, "partial": int,
                                      "red_gate_rate": Optional[float], "batch_gates": int,
                                      "red_batch_rate": Optional[float]})


class TierHeads(TypedDict):
    tier: str
    heads: int
    first_try_pass: int
    first_try_pass_rate: float | None


class FirstTry(TypedDict):
    heads: int
    first_try_pass: int
    first_try_pass_rate: float | None
    by_tier: list[TierHeads]


class FlakeStep(TypedDict):
    step: str
    failed: int
    flaky: int
    flake_rate: float | None
    heads: int


class Flakes(TypedDict):
    failed_step_instances: int
    flaky_step_instances: int
    flake_rate: float | None
    per_step: list[FlakeStep]
    top_flaky: list[FlakeStep]


class Sources(TypedDict):
    history_records: int
    history_records_day: int
    ledger_records: int
    ledger_records_day: int


class Report(TypedDict):
    day: str
    timezone: str
    sources: Sources
    overall: Overall
    splits: list[Split]
    gates: GatesBlock
    first_try: FirstTry
    flakes: Flakes
    notes: list[str]


class FlakeAcc(TypedDict):
    step: str
    failed: int
    flaky: int
    heads: set[str]



def load_jsonl(path: Path) -> list[Row]:
    """JSON-lines records; missing file is empty, malformed lines are skipped."""
    if not path.exists():
        return []
    rows: list[Row] = []
    for line in path.read_text(errors="replace").splitlines():
        if not line.strip():
            continue
        try:
            row = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(row, dict):
            rows.append(row)
    return rows


def tier_of(row: Row) -> str:
    return "owner" if row.get("priority") == "owner" else "normal"


def ledger_class(row: Row) -> str | None:
    kind = row.get("kind")
    if kind == "gate":
        return "shared"
    if kind == "exclusive-rerun":
        return "exclusive"
    return None


def parse_iso(value: object) -> float | None:
    if not isinstance(value, str) or not value:
        return None
    try:
        return datetime.fromisoformat(value.replace("Z", "+00:00")).timestamp()
    except ValueError:
        return None


def ledger_queued_ts(row: Row) -> float | None:
    return parse_iso(row.get("queued_utc")) or parse_iso(row.get("started_utc"))


def day_window(day: str) -> tuple[float, float]:
    start = datetime.combine(datetime.strptime(day, "%Y-%m-%d").date(), time.min, tzinfo=ET)
    return start.timestamp(), (start + timedelta(days=1)).timestamp()


def percentile(values: list[float], q: float) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    if q <= 0.5:
        return float(statistics.median(ordered))
    rank = max(1, math.ceil(q * len(ordered)))
    return float(ordered[rank - 1])


def dist(values: list[float]) -> Dist:
    clean = [float(v) for v in values]
    return {
        "n": len(clean),
        "p50_s": percentile(clean, 0.5),
        "p90_s": percentile(clean, 0.9),
        "sum_s": round(sum(clean), 1) if clean else 0.0,
    }


def rate(part: int, whole: int) -> float | None:
    return round(part / whole, 4) if whole else None


def compute(history: list[Row], ledger: list[Row], day: str) -> Report:
    start, end = day_window(day)

    def in_day(ts: float | None) -> bool:
        return ts is not None and start <= ts < end

    hist_day = [r for r in history if in_day(_as_float(r.get("queued_at")))]
    # Splits by tier x class, from history (the only source with wait/run for all).
    splits: list[Split] = []
    for tier in ("owner", "normal"):
        for cls in ("shared", "exclusive"):
            rows = [r for r in hist_day if tier_of(r) == tier and r.get("class") == cls]
            red = sum(1 for r in rows if _as_float(r.get("exit_code")) not in (None, 0.0))
            splits.append({
                "tier": tier, "class": cls, "tickets": len(rows),
                "wait": dist(_nums(rows, "wait_s")), "run": dist(_nums(rows, "run_s")),
                "red_tickets": red, "red_rate": rate(red, len(rows)),
            })

    exclusive_run_s = sum(_nums([r for r in hist_day if r.get("class") == "exclusive"], "run_s"))
    overall: Overall = {
        "tickets": len(hist_day),
        "shared_tickets": sum(1 for r in hist_day if r.get("class") == "shared"),
        "exclusive_tickets": sum(1 for r in hist_day if r.get("class") == "exclusive"),
        "wait": dist(_nums(hist_day, "wait_s")), "run": dist(_nums(hist_day, "run_s")),
        "gates_per_hour": round(len(hist_day) / 24.0, 3),
        "exclusive_host_hours": round(exclusive_run_s / 3600.0, 3),
    }

    # Ledger full gates for the day (red-gate rate) and global first-try heads.
    gates_day = [r for r in ledger if ledger_class(r) == "shared"
                 and in_day(ledger_queued_ts(r)) and r.get("verdict") != "aborted"]
    verdicts = Counter(str(r.get("verdict")) for r in gates_day)
    gates_block: GatesBlock = {
        "full_gates": len(gates_day),
        "pass": verdicts.get("pass", 0), "fail": verdicts.get("fail", 0),
        "partial": verdicts.get("partial", 0),
        "red_gate_rate": rate(verdicts.get("fail", 0), len(gates_day)),
        "batch_gates": 0, "red_batch_rate": None,
    }

    first_try = _first_try(ledger, in_day)
    flakes = _flakes(ledger, in_day)

    notes = [
        "Day is by queued time in America/New_York.",
        "Flake attribution is a proxy: rerun rows do not name their steps, so a same-head passing solo rerun credits that head's earlier failed steps.",
        "Red-batch rate is null until batching writes batch records; the column is kept ready.",
        "Aborted full gates are excluded from first-try and red-gate rates.",
    ]
    if day == datetime.now(ET).strftime("%Y-%m-%d"):
        notes.insert(0, "This ET day is still in progress; counts are partial until the day ends.")
    return {
        "day": day, "timezone": "America/New_York",
        "sources": {"history_records": len(history), "history_records_day": len(hist_day),
                    "ledger_records": len(ledger),
                    "ledger_records_day": sum(1 for r in ledger if in_day(ledger_queued_ts(r)))},
        "overall": overall, "splits": splits, "gates": gates_block,
        "first_try": first_try, "flakes": flakes,
        "notes": notes,
    }


def _first_try(ledger: list[Row], in_day: InDay) -> FirstTry:
    gates = [r for r in ledger if ledger_class(r) == "shared"
             and r.get("verdict") != "aborted" and r.get("head_sha")]
    gates.sort(key=lambda r: parse_iso(r.get("started_utc")) or parse_iso(r.get("finished_utc")) or 0.0)
    first_by_head: dict[str, Row] = {}
    for row in gates:
        first_by_head.setdefault(str(row["head_sha"]), row)
    todays = [r for r in first_by_head.values() if in_day(ledger_queued_ts(r))]
    by_tier: list[TierHeads] = []
    for tier in ("owner", "normal"):
        rows = [r for r in todays if tier_of(r) == tier]
        passed = sum(1 for r in rows if r.get("verdict") == "pass")
        by_tier.append({"tier": tier, "heads": len(rows), "first_try_pass": passed,
                        "first_try_pass_rate": rate(passed, len(rows))})
    passed = sum(1 for r in todays if r.get("verdict") == "pass")
    return {"heads": len(todays), "first_try_pass": passed,
            "first_try_pass_rate": rate(passed, len(todays)), "by_tier": by_tier}


def _flakes(ledger: list[Row], in_day: InDay) -> Flakes:
    reruns: dict[str, list[float]] = defaultdict(list)
    for row in ledger:
        if ledger_class(row) == "exclusive" and row.get("head_sha") and row.get("rerun_passed") is True:
            ts = parse_iso(row.get("finished_utc")) or parse_iso(row.get("started_utc"))
            if ts is not None:
                reruns[str(row["head_sha"])].append(ts)
    per_step: dict[str, FlakeAcc] = {}
    failed_total = flaky_total = 0
    for row in ledger:
        if ledger_class(row) != "shared" or not in_day(ledger_queued_ts(row)):
            continue
        steps = row.get("failed_steps")
        if not isinstance(steps, list) or not steps:
            continue
        gate_ts = parse_iso(row.get("finished_utc")) or parse_iso(row.get("started_utc")) or 0.0
        head = str(row.get("head_sha") or "")
        later_pass = any(ts >= gate_ts for ts in reruns.get(head, []))
        for step in steps:
            entry = per_step.setdefault(str(step), {"step": str(step), "failed": 0, "flaky": 0, "heads": set()})
            entry["failed"] += 1
            failed_total += 1
            if later_pass:
                entry["flaky"] += 1
                entry["heads"].add(head)
                flaky_total += 1
    rows: list[FlakeStep] = []
    for entry in per_step.values():
        rows.append({"step": entry["step"], "failed": entry["failed"], "flaky": entry["flaky"],
                     "flake_rate": rate(entry["flaky"], entry["failed"]),
                     "heads": len(entry["heads"])})
    rows.sort(key=lambda r: (-r["flaky"], -r["failed"], r["step"]))
    return {"failed_step_instances": failed_total, "flaky_step_instances": flaky_total,
            "flake_rate": rate(flaky_total, failed_total),
            "per_step": sorted(rows, key=lambda r: r["step"]),
            "top_flaky": rows[:10]}


def _as_float(value: object) -> float | None:
    try:
        return float(str(value)) if value is not None else None
    except (TypeError, ValueError):
        return None


def _is_num(value: object) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool)


def _nums(rows: list[Row], key: str) -> list[float]:
    out: list[float] = []
    for row in rows:
        value = row.get(key)
        if isinstance(value, (int, float)) and not isinstance(value, bool):
            out.append(float(value))
    return out


def _fmt(value: object, suffix: str = "") -> str:
    if value is None:
        return "—"
    if isinstance(value, float):
        return f"{value:.1f}{suffix}" if abs(value) < 100 else f"{value:.0f}{suffix}"
    return f"{value}{suffix}"


def _pct(value: float | None) -> str:
    return "—" if value is None else f"{100 * value:.1f}%"


def render_markdown(report: Report) -> str:
    day = report["day"]
    o, g, f, ft = report["overall"], report["gates"], report["flakes"], report["first_try"]
    lines = [
        f"# Gate success rates — {day} (ET)",
        "",
        f"Daily gate report for {day} in America/New_York, from the admission history and the durable gate ledger (read-only). Generated by `scripts/gate_stats.py --day {day}`.",
        "",
        "## Summary",
        "",
        "| Metric | Value |",
        "|---|---:|",
        f"| Tickets queued | {o['tickets']} ({o['shared_tickets']} shared, {o['exclusive_tickets']} exclusive) |",
        f"| Gates per hour | {o['gates_per_hour']} |",
        f"| Exclusive host-hours | {o['exclusive_host_hours']} |",
        f"| Wait p50 / p90 | {_fmt(o['wait']['p50_s'], 's')} / {_fmt(o['wait']['p90_s'], 's')} |",
        f"| Run p50 / p90 | {_fmt(o['run']['p50_s'], 's')} / {_fmt(o['run']['p90_s'], 's')} |",
        f"| Full gates (ledger) | {g['full_gates']} (pass {g['pass']}, fail {g['fail']}, partial {g['partial']}) |",
        f"| Red-gate rate | {_pct(g['red_gate_rate'])} |",
        f"| Red-batch rate | {_pct(g['red_batch_rate'])} |",
        f"| First-try pass rate per head | {_pct(ft['first_try_pass_rate'])} ({ft['first_try_pass']}/{ft['heads']} heads) |",
        f"| Flake rate (failed steps later rerun-passed) | {_pct(f['flake_rate'])} ({f['flaky_step_instances']}/{f['failed_step_instances']} step instances) |",
        "",
        "## By tier and class (admission history)",
        "",
        "| Tier | Class | Tickets | Wait p50 | Wait p90 | Run p50 | Run p90 | Red rate |",
        "|---|---|---:|---:|---:|---:|---:|---:|",
    ]
    for s in report["splits"]:
        lines.append(
            f"| {s['tier']} | {s['class']} | {s['tickets']} | {_fmt(s['wait']['p50_s'], 's')} | "
            f"{_fmt(s['wait']['p90_s'], 's')} | {_fmt(s['run']['p50_s'], 's')} | {_fmt(s['run']['p90_s'], 's')} | {_pct(s['red_rate'])} |")
    lines += [
        "",
        "## First-try pass per head (ledger)",
        "",
        "| Tier | Heads | First-try pass | Rate |",
        "|---|---:|---:|---:|",
    ]
    for ft_row in ft["by_tier"]:
        lines.append(f"| {ft_row['tier']} | {ft_row['heads']} | {ft_row['first_try_pass']} | {_pct(ft_row['first_try_pass_rate'])} |")
    lines += [
        "",
        "## Top flaky steps",
        "",
        "| Step | Failed | Later rerun-passed | Flake rate | Heads |",
        "|---|---:|---:|---:|---:|",
    ]
    if f["top_flaky"]:
        for flake_row in f["top_flaky"]:
            lines.append(f"| `{flake_row['step']}` | {flake_row['failed']} | {flake_row['flaky']} | {_pct(flake_row['flake_rate'])} | {flake_row['heads']} |")
    else:
        lines.append("| — | 0 | 0 | — | 0 |")
    lines += ["", "## Notes", ""]
    lines += [f"- {note}" for note in report["notes"]]
    lines.append("- Batching does not write batch records yet, so the red-batch column stays empty until it does.")
    lines.append("")
    return "\n".join(lines)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Daily gate success-rate report for one ET day.")
    parser.add_argument("--day", required=True, help="ET day to report, YYYY-MM-DD (by queued time)")
    fmt = parser.add_mutually_exclusive_group()
    fmt.add_argument("--json", action="store_true", help="emit JSON instead of Markdown")
    fmt.add_argument("--markdown", action="store_true", help="emit Markdown (the default)")
    parser.add_argument("--history", default=os.environ.get("GATE_STATS_HISTORY", DEFAULT_HISTORY),
                        help=f"admission history JSONL path (default {DEFAULT_HISTORY}, or $GATE_STATS_HISTORY)")
    parser.add_argument("--ledger", default=os.environ.get("GATE_LEDGER", DEFAULT_LEDGER),
                        help=f"gate ledger JSONL path (default {DEFAULT_LEDGER}, or $GATE_LEDGER)")
    parser.add_argument("--output", default=None, help="write to this file instead of stdout")
    args = parser.parse_args(argv)
    try:
        datetime.strptime(args.day, "%Y-%m-%d")
    except ValueError:
        parser.error("--day must be YYYY-MM-DD")
    report = compute(load_jsonl(Path(args.history).expanduser()),
                     load_jsonl(Path(args.ledger).expanduser()), args.day)
    text = json.dumps(report, indent=1, sort_keys=True) + "\n" if args.json else render_markdown(report)
    if args.output:
        Path(args.output).expanduser().write_text(text)
    else:
        sys.stdout.write(text)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
