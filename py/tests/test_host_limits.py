"""Host-limits tests: parser fallbacks, sampler, admission, sweep.

Deterministic throughout: injected /proc snapshots and fake clocks
only. No test sleeps and no assertion reads wall time.
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from local_ci_hub import host_limits as hl  # noqa: E402

GIB = 1024 ** 3
MEM = 800 * GIB


def _host_file(tmp_path: Path, body: str) -> Path:
    path = tmp_path / "host-limits.toml"
    path.write_text(body)
    path.chmod(0o644)
    return path


def _valid_body(root: Path) -> str:
    return (
        "version = 1\n"
        "[admission]\nmax_concurrent_validation_runs = 4\n"
        "cleanup_pending_max = 6\nslot_fs_min_free_bytes = 1000\n"
        "[run_scope]\ncpu_quota_cpus = 8\nmemory_max_bytes = 1000000\npids_max = 2048\n"
        f'[paths]\nstate_root = "{root}"\n'
    )


def _snap(idle: int, total: int = 100, cpus: int = 4) -> str:
    lines = ["cpu  0 0 0 9999 0 0 0 0 0 0"]
    for i in range(cpus):
        busy = total - idle
        lines.append(f"cpu{i} {busy} 0 0 {idle} 0 0 0 0 0 0")
    return "\n".join(lines) + "\n"


_ZERO = "\n".join(["cpu  0 0 0 0 0 0 0 0 0 0"] + [f"cpu{i} 0 0 0 0 0 0 0 0 0 0" for i in range(8)]) + "\n"


def test_missing_file_is_limp_never_refusal(tmp_path: Path) -> None:
    eff = hl.load_effective(tmp_path / "absent.toml", mem_total=MEM)
    assert not eff.file_present
    assert eff.max_concurrent_validation_runs == 1 and eff.cleanup_pending_max == 1
    assert eff.cpu_quota_cpus == 16 and eff.pids_max == 16384
    assert eff.memory_max_bytes == MEM // 8  # unmeasured built-in: MemTotal/8
    assert eff.slot_fs_min_free_bytes == 50 * GIB
    assert eff.warnings and eff.errors  # warns every run; doctor errors


def test_valid_file_values_and_sources(tmp_path: Path) -> None:
    root = tmp_path / "state"
    eff = hl.load_effective(_host_file(tmp_path, _valid_body(root)), mem_total=MEM)
    assert eff.max_concurrent_validation_runs == 4 and eff.cleanup_pending_max == 6
    assert eff.memory_max_bytes == 1000000 and eff.state_root == root
    assert eff.sources["run_scope.pids_max"] == "file" and not eff.errors


def test_malformed_keys_fall_back_alone(tmp_path: Path) -> None:
    body = _valid_body(tmp_path / "state").replace(
        "max_concurrent_validation_runs = 4", "max_concurrent_validation_runs = true"
    ).replace("memory_max_bytes = 1000000", "memory_max_bytes = 0"
              ).replace('state_root = "' + str(tmp_path / "state") + '"',
                        'state_root = "relative/path"')
    eff = hl.load_effective(_host_file(tmp_path, body), mem_total=MEM)
    assert eff.max_concurrent_validation_runs == 1  # bool is never an int; caps at 1
    assert eff.memory_max_bytes == MEM // 8  # 0 is malformed, never unlimited
    assert eff.state_root == hl.DEFAULT_STATE_ROOT.expanduser()
    assert eff.cleanup_pending_max == 6 and eff.pids_max == 2048  # valid keys still apply


def test_stricter_floor_fallback_both_sides(tmp_path: Path) -> None:
    body = _valid_body(tmp_path / "state").replace(
        "slot_fs_min_free_bytes = 1000", 'slot_fs_min_free_bytes = "lots"')
    path = _host_file(tmp_path, body)
    small = hl.load_effective(path, mem_total=MEM, slot_total_bytes=100 * GIB)
    assert small.slot_fs_min_free_bytes == 50 * GIB  # 10% of 100 GiB is less strict
    large = hl.load_effective(path, mem_total=MEM, slot_total_bytes=1024 * GIB)
    assert large.slot_fs_min_free_bytes == (1024 * GIB) // 10  # 10% wins when stricter


def test_bad_version_discards_file_and_unknown_keys_warn(tmp_path: Path) -> None:
    body = _valid_body(tmp_path / "state").replace("version = 1", "version = 2")
    eff = hl.load_effective(_host_file(tmp_path, body), mem_total=MEM)
    assert eff.max_concurrent_validation_runs == 1 and eff.errors
    body2 = _valid_body(tmp_path / "state") + "\n[mystery]\nkey = 1\n"
    eff2 = hl.load_effective(_host_file(tmp_path, body2), mem_total=MEM, reader="doctor")
    assert eff2.unknown_keys == ["mystery.key"] and eff2.warnings


def test_reader_validates_only_its_own_tables(tmp_path: Path) -> None:
    eff = hl.load_effective(_host_file(tmp_path, _valid_body(tmp_path / "state")),
                            mem_total=MEM, reader="sweep")
    assert eff.cleanup_pending_max == 6  # sweep reads K and the state root
    assert eff.memory_max_bytes == MEM // 8  # run_scope is not sweep's table


def test_symlink_state_root_falls_back(tmp_path: Path) -> None:
    real = tmp_path / "real"
    real.mkdir()
    real.chmod(0o700)
    link = tmp_path / "link"
    link.symlink_to(real)
    body = _valid_body(link)
    eff = hl.load_effective(_host_file(tmp_path, body), mem_total=MEM)
    assert eff.state_root == hl.DEFAULT_STATE_ROOT.expanduser() and eff.errors


def test_sampler_affinity_set_only() -> None:
    first = hl.parse_proc_stat(_ZERO)
    second_text = _snap(50) + "cpu9 100 0 0 0 0 0 0 0 0 0\n"
    second = hl.parse_proc_stat(second_text)
    frac = hl.idle_fraction_between(first, second, {0, 1})
    assert frac == 0.5  # cpu9 is outside the set and must not move it
    head = hl.sample_headroom(cpus={0, 1}, snapshots=(_ZERO, second_text),
                              meminfo_text="MemTotal: 819200 kB\nMemAvailable: 409600 kB\n")
    assert head.h == 1 and head.mem_available_bytes == 409600 * 1024


def test_sampler_loadavg_fallback_and_injected_never_sleeps() -> None:
    def boom(_seconds: float) -> None:
        raise AssertionError("injected snapshots must not sleep")

    head = hl.sample_headroom(cpus={0, 1, 2, 3}, snapshots=(_ZERO, _snap(100)),
                              meminfo_text="MemFree: 1024 kB\n", sleep_fn=boom)
    assert head.idle_fraction == 1.0 and head.signal == "proc-stat"
    fallen = hl.sample_headroom(cpus={0, 1, 2, 3}, stat_texts=(None, None),
                                loadavg_1min=1.2, meminfo_text="MemFree: 1024 kB\n")
    assert fallen.signal == "loadavg-fallback" and fallen.h == 2  # 4 - ceil(1.2)


def _limits(tmp_path: Path) -> hl.EffectiveLimits:
    return hl.load_effective(_host_file(tmp_path, _valid_body(tmp_path / "state")),
                             mem_total=MEM)


def _head(n: int, idle: float, mem: int) -> hl.Headroom:
    return hl.Headroom(n=n, idle_fraction=idle, h=int(n * idle),
                       signal="proc-stat", mem_available_bytes=mem)


def test_admission_boundaries_and_young_charge(tmp_path: Path) -> None:
    limits = _limits(tmp_path)  # Vmax 4, K 6, D 1000, M 1000000
    base = dict(v=0, y=0, p=0, f=2000)
    assert hl.decide(hl.AdmissionInputs(headroom=_head(4, 0.5, 1000000), **base), limits).admit
    busy = hl.decide(hl.AdmissionInputs(headroom=_head(4, 0.49, 9000000), **base), limits)
    assert not busy.admit and busy.binding == "cpu_headroom"
    # Young charge: one young run spends 20 threads and one full ceiling.
    young = hl.decide(hl.AdmissionInputs(
        headroom=_head(100, 0.9, 2000000), **{**base, "y": 1}), limits)
    assert young.admit and young.mem_adj == 1000000  # MemAvailable >= M x (Y+1) exactly
    short = hl.decide(hl.AdmissionInputs(
        headroom=_head(100, 0.9, 1999999), **{**base, "y": 1}), limits)
    assert not short.admit and short.binding == "memory_headroom"


def test_admission_pending_and_floor_and_ceiling(tmp_path: Path) -> None:
    limits = _limits(tmp_path)
    head = _head(4, 1.0, 9000000)
    assert hl.decide(hl.AdmissionInputs(v=0, y=0, p=5, f=2000, headroom=head), limits).admit
    refused = hl.decide(hl.AdmissionInputs(v=0, y=0, p=6, f=2000, headroom=head), limits)
    assert not refused.admit and refused.binding == "cleanup_pending_max"  # refuse at P >= K
    low = hl.decide(hl.AdmissionInputs(v=0, y=0, p=0, f=999, headroom=head), limits)
    assert not low.admit and low.binding == "slot_fs_min_free_bytes"
    full = hl.decide(hl.AdmissionInputs(v=4, y=0, p=0, f=2000, headroom=head), limits)
    assert not full.admit and full.binding == "max_concurrent_validation_runs"


def test_free_bytes_is_btrfs_minimum() -> None:
    free, parts = hl.measure_free_bytes(Path("/unused"),
                                        statvfs_fn=lambda _p: (5000, 10000),
                                        backing_free=3000, qgroup_room=4000)
    assert free == 3000 and parts["slot_fs_free_bytes"] == 5000
    preferred, _ = hl.measure_free_bytes(Path("/unused"), manager_free=2500,
                                         statvfs_fn=lambda _p: (5000, 10000))
    assert preferred == 2500  # slot-manager value preferred


def test_gate_unknown_when_unset(tmp_path: Path) -> None:
    g, cap, _note = hl.count_gate_runs({"GATE_SLOTS_CONFIG": str(tmp_path / "none.toml")})
    assert g == "unknown" and cap is None  # informational only, never blocks
    runs = tmp_path / "gate" / "runs"
    runs.mkdir(parents=True)
    (runs / "a.json").write_text("{}")
    (runs / "b.json").write_text("{}")
    g2, _cap2, _n = hl.count_gate_runs({"GATE_ADMISSION_DIR": str(tmp_path / "gate")})
    assert g2 == 2


def test_event_wait_wakes_and_expires_on_fake_clock() -> None:
    clock = {"t": 0.0}
    calls = {"n": 0}

    def decide() -> hl.Decision:
        calls["n"] += 1
        return hl.Decision(calls["n"] >= 3, "admitted" if calls["n"] >= 3 else "cpu_headroom", 0, 0)

    def wait_event(timeout: float) -> None:
        clock["t"] += min(timeout, 5.0)  # an event arrives; no real sleep

    decision, admitted = hl.wait_for_admission(
        decide, bound_s=30.0, now_fn=lambda: clock["t"], wait_event_fn=wait_event,
        jitter_fn=lambda: 0.0)
    assert admitted and decision.admit
    clock["t"] = 0.0
    _d, admitted2 = hl.wait_for_admission(
        lambda: hl.Decision(False, "cpu_headroom", 0, 0), bound_s=10.0,
        now_fn=lambda: clock["t"], wait_event_fn=wait_event, jitter_fn=lambda: 0.0)
    assert not admitted2 and clock["t"] >= 10.0  # at the bound: no result
    _d3, admitted3 = hl.wait_for_admission(
        lambda: hl.Decision(False, "cpu_headroom", 0, 0), bound_s=0.0,
        now_fn=lambda: 0.0, wait_event_fn=wait_event)
    assert not admitted3  # zero bound is a single attempt


def test_box_breach_maps_to_no_result_never_failure() -> None:
    assert hl.box_breach("oom_kill 1\n", None) == "memory"
    assert hl.box_breach(None, "max 3\n") == "pids"
    assert hl.box_breach("oom_kill 0\n", "max 0\n") is None
    assert hl.classify_outcome(validator_exit=1, breach="memory",
                               wall_breached=False, cpu_breached=False) == ("no_result", 75)
    assert hl.classify_outcome(validator_exit=1, breach=None,
                               wall_breached=True, cpu_breached=False) == ("no_result", 75)
    assert hl.classify_outcome(validator_exit=1, breach=None,
                               wall_breached=False, cpu_breached=True) == ("timeout", 1)
    # A CPU breach of a host-clamped budget is host-imposed: no result.
    assert hl.classify_outcome(validator_exit=1, breach=None, wall_breached=False,
                               cpu_breached=True, cpu_bound_by="host") == ("no_result", 75)
    # A CPU breach of a project/CLI-set budget that binds is a timeout.
    assert hl.classify_outcome(validator_exit=1, breach=None, wall_breached=False,
                               cpu_breached=True, cpu_bound_by="instance") == ("timeout", 1)
    assert hl.classify_outcome(validator_exit=1, breach=None,
                               wall_breached=False, cpu_breached=False) == ("fail", 1)
    assert hl.classify_outcome(validator_exit=0, breach=None,
                               wall_breached=False, cpu_breached=False) == ("pass", 0)


def test_sweep_defers_under_hold_and_quiet_then_clears(tmp_path: Path) -> None:
    import datetime as dt

    holds = tmp_path / "holds"
    holds.mkdir()
    marker = tmp_path / "marker.txt"
    now = dt.datetime.now(dt.timezone.utc)
    marker.write_text(f"WINDOW-START {now:%Y-%m-%dT%H:%M:%SZ}\n")
    (holds / "measure.json").write_text(json.dumps(
        {"marker_file": str(marker), "max_hold_seconds": 3600}))
    env = {"GATE_DEFER_CONFIG": str(tmp_path / "absent.toml"),
           "GATE_DEFER_HOLDS_DIR": str(holds), "GATE_DEFER_SHARED_HOLDS_DIR": "",
           "GATE_DEFER_QUIET_COMMAND": ""}
    deferred, sources = hl.sweep_deferral(now.timestamp(), env)
    assert deferred and sources  # deletes nothing while a hold is active
    expired, _ = hl.sweep_deferral(now.timestamp() + 7200, env)
    assert not expired
    quiet_env = dict(env, GATE_DEFER_HOLDS_DIR="",
                     GATE_DEFER_QUIET_COMMAND="check-quiet")
    deferred_q, _s = hl.sweep_deferral(now.timestamp(), quiet_env,
                                       quiet_runner=lambda _cmd: 1)
    assert deferred_q
    clear_q, _s2 = hl.sweep_deferral(now.timestamp(), quiet_env,
                                     quiet_runner=lambda _cmd: 0)
    assert not clear_q
