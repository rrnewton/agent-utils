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
    parse_irq_affinity,
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


# Designated IRQ affinity is the confound CONDITION: (irq_name, smp_affinity_list, desc).
# irq 24 (a NIC) is steered to core 2; irq 130 (ahci) to core 0; irq 25 (nvme) to core 3;
# irq 9 is FULL-SPAN (0-3 == every core) -> unsteered default, designates nothing.
_AFFINITY = [
    ("24", "2", "IR-PCI-MSI 512000-edge eth0-TxRx-0"),
    ("130", "0", "IR-PCI-MSI 0000:aa ahci"),
    ("25", "3", "IR-PCI-MSI 0000:00 nvme0q1"),
    ("9", "0-3", "IR-IO-APIC 9-fasteoi acpi"),
]


def test_parse_interrupts_numbered_rows_only_and_network_tagging() -> None:
    # SECONDARY activity view: cores where an IRQ actually fired (count > 0).
    got = parse_interrupts(_INTERRUPTS)
    assert got[2] == ("net-irq:24",)
    assert got[3] == ("dev-irq:25",)  # nvme0q1 is a storage queue, not a network IRQ
    assert got[0] == ("dev-irq:130",)
    # CPU1 saw no *device* IRQ activity (only arch rows) -> absent.
    assert 1 not in got


def test_parse_interrupts_empty_and_headerless() -> None:
    assert parse_interrupts("") == {}
    assert parse_interrupts("\n\n") == {}


def test_parse_irq_affinity_steered_subset_only() -> None:
    # The condition: a proper-subset affinity list designates its cores; a full-span list
    # (every core) is the unsteered default and designates NONE (mirror-failure guard).
    got = parse_irq_affinity(_AFFINITY, ncpu=4)
    assert got[2] == ("net-irq:24",)
    assert got[0] == ("dev-irq:130",)
    assert got[3] == ("dev-irq:25",)
    assert 1 not in got  # nothing steered to core 1
    # irq 9 spanned all four cores -> excluded from every core, not confounding the machine.
    assert all("dev-irq:9" not in tags for tags in got.values())


def test_scan_confounds_cpu0_always_and_affinity_reasons() -> None:
    verdict = scan_confounds(
        range(4),
        interrupts_text=_INTERRUPTS,
        irq_affinity_entries=_AFFINITY,
        include_kthreads=False,
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
        range(6),
        interrupts_text="",
        irq_affinity_entries=[],
        proc_root=proc,
        include_kthreads=True,
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


def test_irq_core_skipped_and_ordinary_cores_allocatable(tmp_path: Path) -> None:
    """IRQ half, both directions: a NETWORK-IRQ core is skipped while ordinary cores remain,
    and the count of ordinary cores is asserted (guards the 'filter excludes everything'
    mirror failure)."""
    import os

    # 6-core synthetic host: every core carries a per-core nvme queue (steered to itself);
    # cores 1 and 4 ALSO carry a network IRQ. So 0 cores are strictly clean, but 4 cores
    # (0,2,3,5) are network-free ordinary cores. (Core 0 also gets cpu0-timer-sink.)
    ncpu = 6
    affinity = [(f"{100 + c}", str(c), f"nvme0q{c}") for c in range(ncpu)]
    affinity += [("200", "1", "eth0-TxRx-0"), ("201", "4", "eth0-TxRx-3")]
    confounds = scan_confounds(
        range(ncpu),
        interrupts_text=" ".join(f"CPU{c}" for c in range(ncpu)),
        irq_affinity_entries=affinity,
        include_kthreads=False,
    )
    ordinary = sorted(c for c, v in confounds.items() if not v.is_network)
    N_ORDINARY = 4  # STATED: cores 0,2,3,5 are network-free
    assert ordinary == [0, 2, 3, 5]
    assert len(ordinary) == N_ORDINARY
    net_cores = sorted(c for c, v in confounds.items() if v.is_network)
    assert net_cores == [1, 4]

    alloc = CoreAllocator(
        cores=range(ncpu), state_dir=tmp_path / "leases", confounds=confounds, pid=os.getpid()
    )
    # Leasing all N_ORDINARY cores must hand out exactly the network-free set, skipping the
    # network cores entirely (they rank worse by severity).
    lease = alloc.acquire(N_ORDINARY)
    assert set(lease.cores) == set(ordinary)
    for net in net_cores:
        assert net not in lease.cores  # a core WITH network-IRQ affinity is SKIPPED
    # The two network cores are only handed out once ordinary cores are exhausted, ANNOTATED.
    fallback = alloc.acquire(2)
    assert set(fallback.cores) == set(net_cores)
    assert set(fallback.confounded) == set(net_cores)


def test_n_sequential_acquire_and_release_all_return(tmp_path: Path) -> None:
    """N sequential acquire+release cycles each grant AND free (guards an allocator that
    hands out nothing). N is STATED."""
    import os

    N = 5  # STATED number of cycles
    d = tmp_path / "leases"
    for i in range(N):
        alloc = CoreAllocator(
            cores=range(3), state_dir=d, confounds=_clean(range(3)), pid=os.getpid()
        )
        lease = alloc.acquire(3)
        assert len(lease.cores) == 3, f"cycle {i}: acquire returned nothing"
        alloc.release(lease)
        # After release the whole host is free again -> next cycle can take all 3.
        assert not alloc._load(), f"cycle {i}: release did not free the leases"


def test_pin_current_process_roundtrip() -> None:
    import os

    original = os.sched_getaffinity(0)
    target = {min(original)}
    try:
        pin_current_process(target)
        assert os.sched_getaffinity(0) == target
    finally:
        os.sched_setaffinity(0, original)
