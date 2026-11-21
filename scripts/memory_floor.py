#!/usr/bin/env python3
"""Shared host memory floor: the single source both teams read (#2062).

File contract (the whole contract; reimplementing it needs only this):

  Path: ~/.local/etc/host-limits/memory-floor.toml
        (override $MEMORY_FLOOR_CONFIG; per-host, never synced)
  Keys (TOML, all optional; an absent key takes the default):
    version = 1
    heavy_min_gib = 64            # heavy floor is at least this
    heavy_percent_of_total = 10   # ... and at least this % of MemTotal
    light_floor_gib = 16
    poll_s = 5
  The file holds ONLY host floors. It never holds a job's need: each
  caller passes its own need, and admission is

      MemAvailable - need >= floor(class)

  with floor(heavy) = max(heavy_min_gib, heavy_percent_of_total% of
  MemTotal) and floor(light) = light_floor_gib, all from the live
  /proc/meminfo (override $MEMORY_FLOOR_MEMINFO, or --meminfo).

A missing file means the defaults above (the agreed contract: both
teams get the same floors out of the box, and an operator places the
file only to change them). A file that exists but is malformed, and a
meminfo that cannot be parsed, refuse with exit 2 (fail closed),
never a silent admit.

Usage (the tiny shared reader either team can call):
  memory_floor.py check --class heavy --need-gib 32 [--meminfo PATH]
      prints one JSON line {mem_available_gib, mem_total_gib,
      floor_gib, need_gib, class, admit, margin_gib, source};
      exit 0 admit, 78 not yet, 2 malformed config or meminfo.
  memory_floor.py wait (same flags) polls poll_s until admitted,
      printing each changed verdict, then exits 0.
"""
from __future__ import annotations

import argparse
import json
import math
import os
import sys
import time
from dataclasses import dataclass
from pathlib import Path

if sys.version_info >= (3, 11):
    import tomllib
else:  # Python 3.10: the tomli package provides the same API
    import tomli as tomllib

DEFAULT_CONFIG = Path("~/.local/etc/host-limits/memory-floor.toml")
DEFAULT_MEMINFO = Path("/proc/meminfo")

HEAVY_MIN_GIB = 64.0
HEAVY_PERCENT_OF_TOTAL = 10.0
LIGHT_FLOOR_GIB = 16.0
POLL_S = 5.0

_ALLOWED_KEYS = {"version", "heavy_min_gib", "heavy_percent_of_total",
                 "light_floor_gib", "poll_s"}
_CLASSES = ("heavy", "light")


class MemoryFloorError(Exception):
    """The floor config or meminfo exists but cannot be used as written."""


@dataclass(frozen=True)
class FloorConfig:
    heavy_min_gib: float = HEAVY_MIN_GIB
    heavy_percent_of_total: float = HEAVY_PERCENT_OF_TOTAL
    light_floor_gib: float = LIGHT_FLOOR_GIB
    poll_s: float = POLL_S
    source: str = "defaults"

    def floor_gib(self, cls: str, mem_total_gib: float) -> float:
        if cls == "heavy":
            return max(self.heavy_min_gib,
                       self.heavy_percent_of_total / 100.0 * mem_total_gib)
        if cls == "light":
            return self.light_floor_gib
        raise MemoryFloorError(f"unknown memory class {cls!r} (heavy or light)")


def config_path(explicit: str | None = None) -> Path:
    raw = explicit or os.environ.get("MEMORY_FLOOR_CONFIG") or DEFAULT_CONFIG
    return Path(raw).expanduser()


def meminfo_path(explicit: str | None = None) -> Path:
    raw = explicit or os.environ.get("MEMORY_FLOOR_MEMINFO") or DEFAULT_MEMINFO
    return Path(raw).expanduser()


def load_config(explicit: str | None = None) -> FloorConfig:
    """The host floor: the file's values, or the defaults when absent.

    A present-but-malformed file raises MemoryFloorError: a floor an
    operator tried to set must never silently vanish.
    """
    path = config_path(explicit)
    try:
        text = path.read_text(encoding="utf-8")
    except FileNotFoundError:
        return FloorConfig(source=f"defaults (no floor file at {path})")
    except OSError as exc:
        raise MemoryFloorError(f"memory floor config {path} is unreadable: {exc}") from exc
    try:
        raw = tomllib.loads(text)
    except tomllib.TOMLDecodeError as exc:
        raise MemoryFloorError(f"memory floor config {path} is not TOML: {exc}") from exc
    unknown = set(raw) - _ALLOWED_KEYS
    if unknown:
        raise MemoryFloorError(
            f"memory floor config {path} may only set {sorted(_ALLOWED_KEYS)}; "
            f"got {sorted(unknown)}")
    version = raw.get("version", 1)
    if version != 1:
        raise MemoryFloorError(f"memory floor config {path}: version must be 1, not {version!r}")

    def _num(key: str, default: float) -> float:
        value = raw.get(key, default)
        # Non-finite values refuse: nan compares false against every
        # floor and would silently weaken it, inf would block forever.
        if (isinstance(value, bool) or not isinstance(value, (int, float))
                or not math.isfinite(value) or value < 0):
            raise MemoryFloorError(
                f"memory floor config {path}: {key} must be a finite, "
                f"non-negative number, not {value!r}")
        return float(value)

    return FloorConfig(
        heavy_min_gib=_num("heavy_min_gib", HEAVY_MIN_GIB),
        heavy_percent_of_total=_num("heavy_percent_of_total", HEAVY_PERCENT_OF_TOTAL),
        light_floor_gib=_num("light_floor_gib", LIGHT_FLOOR_GIB),
        poll_s=_num("poll_s", POLL_S) or POLL_S,
        source=str(path),
    )


def parse_meminfo(text: str) -> tuple[float, float]:
    """(MemAvailable GiB, MemTotal GiB) from /proc/meminfo text."""
    found: dict[str, float] = {}
    for line in text.splitlines():
        parts = line.split()
        if len(parts) >= 2 and parts[0] in ("MemTotal:", "MemAvailable:"):
            try:
                kib = float(parts[1])
            except ValueError:
                continue
            if not math.isfinite(kib) or kib < 0:
                raise MemoryFloorError(f"meminfo {parts[0]} is not finite: {parts[1]!r}")
            found[parts[0]] = kib / (1024.0 * 1024.0)  # kB -> GiB
    if "MemTotal:" not in found or "MemAvailable:" not in found:
        raise MemoryFloorError("meminfo lacks MemTotal or MemAvailable")
    return found["MemAvailable:"], found["MemTotal:"]


def read_meminfo(explicit: str | None = None) -> tuple[float, float]:
    path = meminfo_path(explicit)
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as exc:
        raise MemoryFloorError(f"meminfo {path} is unreadable: {exc}") from exc
    try:
        return parse_meminfo(text)
    except MemoryFloorError as exc:
        raise MemoryFloorError(f"meminfo {path}: {exc}") from exc


def check(cls: str, need_gib: float, config: FloorConfig | None = None,
          meminfo: str | None = None) -> dict[str, object]:
    """One admission verdict; `config=None` loads the host file."""
    if cls not in _CLASSES:
        raise MemoryFloorError(f"unknown memory class {cls!r} (heavy or light)")
    if not math.isfinite(need_gib) or need_gib < 0:
        raise MemoryFloorError(f"need must be a finite, non-negative number, not {need_gib}")
    cfg = load_config() if config is None else config
    available, total = read_meminfo(meminfo)
    floor = cfg.floor_gib(cls, total)
    margin = (available - need_gib) - floor
    return {"class": cls, "need_gib": need_gib, "floor_gib": round(floor, 3),
            "mem_available_gib": round(available, 3),
            "mem_total_gib": round(total, 3), "admit": margin >= 0,
            # A need larger than MemTotal - floor can never be admitted;
            # callers must not wait forever on it (they admit with a
            # logged warning instead).
            "never_admissible": need_gib > total - floor,
            "margin_gib": round(margin, 3), "source": cfg.source}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="memory_floor",
        description="Shared host memory-floor admission (#2062); see the file header "
                    "for the file contract both teams read.")
    sub = parser.add_subparsers(dest="action", required=True)
    for name, help_text in (("check", "print one JSON verdict (exit 0 admit, 78 not yet)"),
                            ("wait", "poll until admitted, printing changed verdicts")):
        cmd = sub.add_parser(name, help=help_text)
        cmd.add_argument("--class", dest="cls", choices=_CLASSES, required=True,
                         help="heavy: a validation, full build or test run; "
                              "light: a single check or rerun step")
        cmd.add_argument("--need-gib", type=float, required=True,
                         help="this job's own need in GiB (its MemoryMax, its measured "
                              "peak, or a caller's estimate); admission is "
                              "MemAvailable - need >= floor")
        cmd.add_argument("--config", default=None,
                         help="floor file (default $MEMORY_FLOOR_CONFIG or "
                              "~/.local/etc/host-limits/memory-floor.toml)")
        cmd.add_argument("--meminfo", default=None,
                         help="meminfo file (default $MEMORY_FLOOR_MEMINFO or /proc/meminfo); "
                              "the test hook for a fake /proc/meminfo")
    args = parser.parse_args(argv)
    try:
        cfg = load_config(args.config)
        if args.action == "check":
            verdict = check(args.cls, args.need_gib, cfg, args.meminfo)
            print(json.dumps(verdict, sort_keys=True), flush=True)
            return 0 if verdict["admit"] else 78
        last: object = None
        while True:
            verdict = check(args.cls, args.need_gib, cfg, args.meminfo)
            # Reload the file each poll: an operator raising a floor
            # takes effect on waiters without anyone re-queueing.
            cfg = load_config(args.config)
            key = (verdict["admit"], verdict["margin_gib"])
            if key != last:
                print(json.dumps(verdict, sort_keys=True), flush=True)
                last = key
            if verdict["admit"]:
                return 0
            time.sleep(cfg.poll_s)
    except MemoryFloorError as exc:
        print(f"memory-floor: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
