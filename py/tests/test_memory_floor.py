"""Shared host memory floor (#2062): floors only in the file, need per caller."""
from __future__ import annotations

import importlib.util
import json
import os
import subprocess
import sys
from pathlib import Path
from types import ModuleType

import pytest

SCRIPT = Path(__file__).resolve().parents[2] / "scripts" / "memory_floor.py"


def _load() -> ModuleType:
    spec = importlib.util.spec_from_file_location("memory_floor", SCRIPT)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module  # dataclasses look their module up there
    spec.loader.exec_module(module)
    return module


mf = _load()


def _meminfo(tmp_path: Path, total_gib: float, avail_gib: float) -> Path:
    path = tmp_path / "meminfo"
    staging = tmp_path / "meminfo.staging"
    staging.write_text(
        f"MemTotal:       {int(total_gib * 1024 * 1024)} kB\n"
        f"MemAvailable:   {int(avail_gib * 1024 * 1024)} kB\n")
    os.replace(staging, path)
    return path


def _config(tmp_path: Path, text: str) -> Path:
    path = tmp_path / "memory-floor.toml"
    path.write_text(text)
    return path


def test_heavy_floor_is_max_of_gib_and_percent(tmp_path: Path) -> None:
    cfg = mf.load_config(str(_config(tmp_path, "")))
    assert cfg is not None
    # 100 GiB total: 10% = 10 GiB, so the 64 GiB floor wins.
    assert cfg.floor_gib("heavy", 100.0) == 64.0
    # 1000 GiB total: 10% = 100 GiB wins.
    assert cfg.floor_gib("heavy", 1000.0) == 100.0
    assert cfg.floor_gib("light", 1000.0) == 16.0


def test_admit_is_available_minus_need_at_or_above_floor(tmp_path: Path) -> None:
    cfg = mf.load_config(str(_config(tmp_path, "")))
    info = _meminfo(tmp_path, 200.0, 100.0)
    # 100 - 36 = 64, exactly the heavy floor: admit.
    verdict = mf.check("heavy", 36.0, cfg, str(info))
    assert verdict["admit"] is True and verdict["margin_gib"] == 0.0
    # One GiB more need leaves the host below the floor: not yet.
    verdict = mf.check("heavy", 37.0, cfg, str(info))
    assert verdict["admit"] is False and verdict["margin_gib"] == -1.0
    # Light floor is 16 GiB flat.
    assert mf.check("light", 84.0, cfg, str(info))["admit"] is True
    assert mf.check("light", 85.0, cfg, str(info))["admit"] is False


def test_custom_floors_from_file(tmp_path: Path) -> None:
    cfg = mf.load_config(str(_config(
        tmp_path, "heavy_min_gib = 10\nheavy_percent_of_total = 0\nlight_floor_gib = 2\n")))
    assert cfg is not None
    assert cfg.floor_gib("heavy", 500.0) == 10.0
    assert cfg.floor_gib("light", 500.0) == 2.0


def test_absent_file_means_defaults_and_malformed_refuses(tmp_path: Path) -> None:
    cfg = mf.load_config(str(tmp_path / "absent.toml"))
    assert cfg.floor_gib("heavy", 100.0) == 64.0 and cfg.floor_gib("light", 100.0) == 16.0
    verdict = mf.check("heavy", 9999.0, cfg, str(_meminfo(tmp_path, 10.0, 1.0)))
    assert verdict["admit"] is False and verdict["floor_gib"] == 64.0
    with pytest.raises(mf.MemoryFloorError):
        mf.load_config(str(_config(tmp_path, "heavy_min_gib = 'lots'\n")))
    with pytest.raises(mf.MemoryFloorError):
        mf.load_config(str(_config(tmp_path, "bogus_key = 1\n")))
    bad = tmp_path / "bad-meminfo"
    bad.write_text("MemTotal: 100 kB\n")
    with pytest.raises(mf.MemoryFloorError):
        mf.check("heavy", 1.0, mf.load_config(str(_config(tmp_path, ""))), str(bad))


def _cli(tmp_path: Path, *args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run([sys.executable, str(SCRIPT), *args],
                          capture_output=True, text=True)


def test_cli_check_exit_codes_and_json(tmp_path: Path) -> None:
    cfg = _config(tmp_path, "")
    info = _meminfo(tmp_path, 200.0, 100.0)
    ok = _cli(tmp_path, "check", "--class", "heavy", "--need-gib", "10",
              "--config", str(cfg), "--meminfo", str(info))
    assert ok.returncode == 0, ok.stdout + ok.stderr
    assert json.loads(ok.stdout)["floor_gib"] == 64.0
    no = _cli(tmp_path, "check", "--class", "heavy", "--need-gib", "90",
              "--config", str(cfg), "--meminfo", str(info))
    assert no.returncode == 78, no.stdout + no.stderr
    bad = _cli(tmp_path, "check", "--class", "heavy", "--need-gib", "1",
               "--config", str(_config(tmp_path, "nope = [\n")), "--meminfo", str(info))
    assert bad.returncode == 2


@pytest.mark.parametrize("value", ["nan", "inf", "-inf", "-1"])
def test_config_non_finite_or_negative_refuses(tmp_path: Path, value: str) -> None:
    """A consumer's review: nan compares false against every floor and
    would silently weaken it; inf would block forever. Both refuse."""
    with pytest.raises(mf.MemoryFloorError):
        mf.load_config(str(_config(tmp_path, f"heavy_min_gib = {value}\n")))
    with pytest.raises(mf.MemoryFloorError):
        mf.load_config(str(_config(tmp_path, f"light_floor_gib = {value}\n")))


@pytest.mark.parametrize("value", ["nan", "inf", "-inf", "-5"])
def test_meminfo_and_need_non_finite_refuse(tmp_path: Path, value: str) -> None:
    cfg = mf.load_config(str(_config(tmp_path, "")))
    bad = tmp_path / "meminfo-bad"
    bad.write_text(f"MemTotal:       {value} kB\nMemAvailable:   1024 kB\n")
    with pytest.raises(mf.MemoryFloorError):
        mf.check("heavy", 1.0, cfg, str(bad))
    with pytest.raises(mf.MemoryFloorError):
        mf.check("heavy", float(value), cfg, str(_meminfo(tmp_path, 200.0, 100.0)))


def test_cli_nan_need_exits_2(tmp_path: Path) -> None:
    result = _cli(tmp_path, "check", "--class", "heavy", "--need-gib", "nan",
                  "--config", str(_config(tmp_path, "")),
                  "--meminfo", str(_meminfo(tmp_path, 200.0, 100.0)))
    assert result.returncode == 2, result.stdout + result.stderr


def test_never_admissible_flag(tmp_path: Path) -> None:
    cfg = mf.load_config(str(_config(tmp_path, "")))
    info = _meminfo(tmp_path, 200.0, 190.0)
    # 200 total, heavy floor 64: a 137 GiB need can never fit.
    assert mf.check("heavy", 137.0, cfg, str(info))["never_admissible"] is True
    assert mf.check("heavy", 136.0, cfg, str(info))["never_admissible"] is False
