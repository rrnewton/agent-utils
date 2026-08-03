"""Tests for the stateful IRQ-aware core allocator.

Confound scanning is exercised with synthetic ``/proc`` content (no dependence on the host
layout); leasing is exercised against a shared ``state_dir`` in ``tmp_path`` so the
cross-process disjointness and PID-liveness reclamation properties are checked directly.
"""

from __future__ import annotations

import subprocess
from collections.abc import Iterator
from pathlib import Path

import pytest

from safe_ci_dag_runner.coreallocator import (
    CoreAllocator,
    CoreConfound,
    CoreExhausted,
    parse_interrupts,
    pin_current_process,
    scan_confounds,
)


@pytest.fixture
def live_pids() -> "Iterator[list[int]]":
    """Two distinct, genuinely-alive PIDs (real background processes), cleaned up after."""
    procs = [subprocess.Popen(["sleep", "30"]) for _ in range(2)]
    try:
        yield [p.pid for p in procs]
    finally:
        for p in procs:
            p.terminate()
            p.wait()

# A small but realistic /proc/interrupts: numbered device rows (one network, one plain) plus
# an architectural per-cpu row that MUST be ignored.
_INTERRUPTS = """\
            CPU0       CPU1       CPU2       CPU3
  24:          0          0        900          0   IR-PCI-MSI 512000-edge      eth0-TxRx-0
  25:          0          0          0         12   IR-PCI-MSI 0000:00 nvme0q1
 130:          5          0          0          0   IR-PCI-MSI 0000:aa ahci
 LOC:      10000      10000      10000      10000   Local timer interrupts
 RES:          1          2          3          4   Rescheduling interrupts
"""


def test_parse_interrupts_numbered_rows_only_and_network_tagging() -> None:
    got = parse_interrupts(_INTERRUPTS)
    # eth0 fired on CPU2 -> network tag; nvme on CPU3 and ahci on CPU0 are storage/device
    # IRQs, NOT network -> dev-irq (the narrowed regex matches only genuine NICs).
    assert got[2] == ("net-irq:24",)
    assert got[3] == ("dev-irq:25",)  # nvme0q1 is a storage queue, not a network IRQ
    assert got[0] == ("dev-irq:130",)
    # CPU1 saw no *device* IRQ activity (only arch rows) -> absent.
    assert 1 not in got


def test_parse_interrupts_empty_and_headerless() -> None:
    assert parse_interrupts("") == {}
    assert parse_interrupts("\n\n") == {}


def test_scan_confounds_cpu0_always_and_irq_reasons() -> None:
    verdict = scan_confounds(
        range(4), interrupts_text=_INTERRUPTS, include_kthreads=False
    )
    assert verdict[0].reasons[0] == "cpu0-timer-sink"  # CPU0 always flagged
    assert "dev-irq:130" in verdict[0].reasons
    assert verdict[1].is_clean  # only core with no confound at all
    assert verdict[2].is_network
    assert not verdict[1].is_network


def test_scan_confounds_pinned_kthread_but_not_percpu(tmp_path: Path) -> None:
    proc = tmp_path / "proc"
    # A genuinely-pinned helper kthread on core 3 (comm does not encode core 3) -> confound.
    _make_kthread(proc, pid=101, comm="my-worker/0", cpus_allowed="3")
    # A per-cpu kthread naturally on core 2 (bare index form) -> must NOT confound.
    _make_kthread(proc, pid=102, comm="ksoftirqd/2", cpus_allowed="2")
    # A userspace process pinned to core 1 -> not a kernel thread, ignored.
    _make_kthread(proc, pid=103, comm="app", cpus_allowed="1", cmdline=b"app\x00--flag\x00")
    # Per-cpu KWORKERS on cores 4 and 5 (kworker/N:M[-suffix]) -> must NOT confound.
    # These are the common case the old bare-endswith test wrongly flagged.
    _make_kthread(proc, pid=104, comm="kworker/4:1", cpus_allowed="4")
    _make_kthread(proc, pid=105, comm="kworker/5:1-events", cpus_allowed="5")

    verdict = scan_confounds(
        range(6), interrupts_text="", proc_root=proc, include_kthreads=True
    )
    assert verdict[3].reasons == ("kthread:my-worker/0",)
    assert verdict[2].is_clean  # bare-index per-cpu kthread excluded
    assert verdict[1].is_clean  # userspace process excluded
    assert verdict[4].is_clean  # kworker/4:1 recognised as per-cpu home
    assert verdict[5].is_clean  # kworker/5:1-events recognised as per-cpu home


def _make_kthread(
    proc: Path, *, pid: int, comm: str, cpus_allowed: str, cmdline: bytes = b""
) -> None:
    d = proc / str(pid)
    d.mkdir(parents=True)
    (d / "cmdline").write_bytes(cmdline)
    (d / "comm").write_text(comm + "\n")
    (d / "status").write_text(
        f"Name:\t{comm}\nCpus_allowed_list:\t{cpus_allowed}\n"
    )


def _clean(cores: range) -> dict[int, CoreConfound]:
    return {c: CoreConfound(core=c, reasons=()) for c in cores}


def test_acquire_prefers_clean_then_least_confounded(tmp_path: Path) -> None:
    import os

    confounds = {
        0: CoreConfound(0, ("cpu0-timer-sink",)),  # severity 1
        1: CoreConfound(1, ("net-irq:24",)),  # severity 3
        2: CoreConfound(2, ()),  # clean, severity 0
        3: CoreConfound(3, ("dev-irq:130",)),  # severity 1
    }
    # Own (live) pid so successive leases persist rather than being reclaimed.
    alloc = CoreAllocator(
        cores=range(4), state_dir=tmp_path / "leases", confounds=confounds, pid=os.getpid()
    )
    # Clean core first, then least-confounded (ties by index), worst (network) last.
    assert alloc.acquire(1).cores == (2,)
    assert alloc.acquire(1).cores == (0,)
    assert alloc.acquire(1).cores == (3,)
    assert alloc.acquire(1).cores == (1,)


def test_leasing_disjoint_across_shared_state(tmp_path: Path, live_pids: list[int]) -> None:
    d = tmp_path / "leases"
    # Two distinct LIVE owners (real background processes) so neither is reclaimed.
    a = CoreAllocator(cores=range(4), state_dir=d, confounds=_clean(range(4)), pid=live_pids[0])
    b = CoreAllocator(cores=range(4), state_dir=d, confounds=_clean(range(4)), pid=live_pids[1])

    la = a.acquire(2)
    lb = b.acquire(2)
    assert set(la.cores).isdisjoint(lb.cores)  # never share a core
    assert set(la.cores) | set(lb.cores) == {0, 1, 2, 3}

    with pytest.raises(CoreExhausted):
        b.acquire(1)  # all four are leased

    a.release(la)
    lb2 = b.acquire(2)  # a's cores are now free again
    assert set(lb2.cores) == set(la.cores)


def test_release_by_lease_id_only_frees_own(tmp_path: Path) -> None:
    import os

    d = tmp_path / "leases"
    a = CoreAllocator(cores=range(2), state_dir=d, confounds=_clean(range(2)), pid=os.getpid())
    l1 = a.acquire(1)
    l2 = a.acquire(1)
    a.release(l1)
    # l2 still held (release freed only l1) -> one core free, so acquiring 2 must fail.
    with pytest.raises(CoreExhausted):
        a.acquire(2)
    a.release(l2)
    assert len(a.acquire(2).cores) == 2


def test_lazy_reclaim_of_dead_owner(tmp_path: Path) -> None:
    d = tmp_path / "leases"
    dead_pid = _make_dead_pid()
    ghost = CoreAllocator(cores=range(2), state_dir=d, confounds=_clean(range(2)), pid=dead_pid)
    ghost.acquire(2)  # both cores leased by a now-dead owner

    import os

    live = CoreAllocator(cores=range(2), state_dir=d, confounds=_clean(range(2)), pid=os.getpid())
    lease = live.acquire(2)  # must reclaim the dead owner's leases, not raise
    assert set(lease.cores) == {0, 1}


def _make_dead_pid() -> int:
    p = subprocess.Popen(["true"])
    p.wait()  # reaped -> os.kill(pid, 0) now raises ESRCH
    return p.pid


def test_confounded_annotation_when_no_clean_core(tmp_path: Path) -> None:
    d = tmp_path / "leases"
    confounds = {0: CoreConfound(0, ("cpu0-timer-sink",)), 1: CoreConfound(1, ("net-irq:9",))}
    alloc = CoreAllocator(cores=range(2), state_dir=d, confounds=confounds, pid=4001)
    lease = alloc.acquire(1)
    assert lease.cores == (0,)  # lower severity first
    assert lease.confounded == (0,)  # handed out, but ANNOTATED (never silent)
    assert not lease.all_clean


def test_pin_current_process_roundtrip() -> None:
    import os

    original = os.sched_getaffinity(0)
    target = {min(original)}
    try:
        pin_current_process(target)
        assert os.sched_getaffinity(0) == target
    finally:
        os.sched_setaffinity(0, original)
