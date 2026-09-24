"""Shared host-admission protocol, ledger, and adversarial controls."""

from __future__ import annotations

import json
import os
import time
from pathlib import Path
from typing import cast

import pytest

from dagrun.admission import Verdict as LegacyVerdict
from dagrun.host_admission_adapter import compare_legacy_memory_decision
from host_admission import (
    AdmissionError,
    AdmissionPolicy,
    HostAdmissionLedger,
    HostSnapshot,
    LeaseView,
    OwnerState,
    ProcessOwner,
    QueueView,
    ResourceRequest,
    SwapMode,
    Verdict,
    canonical_decision_json,
    decide,
    probe_process_owner,
)

FIXTURES = Path(__file__).parents[2] / "common" / "host-admission" / "fixtures-v1.json"


def _mapping(raw: object) -> dict[str, object]:
    assert isinstance(raw, dict)
    assert all(isinstance(key, str) for key in raw)
    return raw


def _sequence(raw: object) -> list[object]:
    assert isinstance(raw, list)
    return raw


def _text(raw: object) -> str:
    assert isinstance(raw, str)
    return raw


def _integer(raw: object) -> int:
    assert isinstance(raw, int) and not isinstance(raw, bool)
    return raw


def _fixture_decision(raw_case: object) -> tuple[str, str, str]:
    case = _mapping(raw_case)
    request = ResourceRequest.from_json(case["request"])
    snapshot = HostSnapshot.from_json(case["snapshot"])
    policy = AdmissionPolicy.from_json(case["policy"])
    leases = tuple(
        LeaseView(
            _text(_mapping(raw)["lease_id"]),
            ResourceRequest.from_json(_mapping(raw)["request"]),
            1,
        )
        for raw in _sequence(case["leases"])
    )
    queue = tuple(
        QueueView(
            ResourceRequest.from_json(_mapping(raw)["request"]),
            _integer(_mapping(raw)["sequence"]),
        )
        for raw in _sequence(case["queue"])
    )
    states_raw = _mapping(case["owner_states"])
    states = {key: OwnerState(_text(value)) for key, value in states_raw.items()}
    decision = decide(
        request,
        snapshot,
        policy,
        leases,
        queue,
        now_unix_ms=_integer(case["now_unix_ms"]),
        owner_states=states,
    )
    expected = _mapping(case["expect"])
    assert decision.verdict.value == _text(expected["verdict"])
    assert decision.code == _text(expected["code"])
    return _text(case["name"]), canonical_decision_json(decision), decision.code


def test_shared_fixture_corpus_pins_all_decisions_and_canonical_bytes() -> None:
    payload = _mapping(json.loads(FIXTURES.read_text(encoding="utf-8")))
    assert payload["schema"] == "host-admission-fixtures/v1"
    results = [_fixture_decision(case) for case in _sequence(payload["cases"])]
    assert len(results) == 24
    assert len({name for name, _decision, _code in results}) == len(results)
    assert all(decision.endswith("\n") and " " not in decision for _name, decision, _code in results)


def _snapshot(*, boot: str = "boot-a") -> HostSnapshot:
    return HostSnapshot(1000, "host-a", boot, 1000, 1000, 1000, 1000, 0, 0)


def _policy(*, token_capacities: tuple[tuple[str, int], ...] = ()) -> AdmissionPolicy:
    return AdmissionPolicy(
        memory_budget_bytes=700,
        memory_reserve_bytes=0,
        token_capacities=token_capacities,
        swap_mode=SwapMode.DISABLED,
        max_snapshot_age_ms=100,
    )


def _owner(pid: int = 10, start: int = 100, boot: str = "boot-a") -> ProcessOwner:
    return ProcessOwner("host-a", boot, pid, start)


def test_ledger_keeps_one_fifo_ticket_and_acquires_memory_and_token_atomically(tmp_path: Path) -> None:
    ledger_path = tmp_path / "host-admission.json"
    ledger = HostAdmissionLedger(ledger_path)
    policy = _policy(token_capacities=(("build", 1),))
    first = ResourceRequest("first", "test", _owner(), 100, (("build", 1),))
    second = ResourceRequest("second", "test", _owner(11, 101), 100, (("build", 1),))
    granted, first_lease = ledger.request(first, _snapshot(), policy, now_unix_ms=1010, owner_probe=lambda _owner, _snapshot: OwnerState.ALIVE)
    assert granted.verdict is Verdict.GRANT and first_lease is not None
    queued, no_lease = ledger.request(second, _snapshot(), policy, now_unix_ms=1010, owner_probe=lambda _owner, _snapshot: OwnerState.ALIVE)
    assert queued.verdict is Verdict.QUEUE and queued.code == "named_tokens_busy"
    assert no_lease is None
    first_bytes = ledger_path.read_bytes()
    queued_again, no_lease = ledger.request(second, _snapshot(), policy, now_unix_ms=1011, owner_probe=lambda _owner, _snapshot: OwnerState.ALIVE)
    assert queued_again.queue_sequence == queued.queue_sequence
    assert no_lease is None and ledger_path.read_bytes() == first_bytes
    state = _mapping(json.loads(first_bytes))
    assert len(_sequence(state["leases"])) == 1, "a token queue must not partially reserve memory"
    first_lease.release()
    admitted, second_lease = ledger.request(second, _snapshot(), policy, now_unix_ms=1012, owner_probe=lambda _owner, _snapshot: OwnerState.ALIVE)
    assert admitted.verdict is Verdict.GRANT and second_lease is not None
    second_lease.release()


def test_unknown_owner_is_not_reclaimed_and_pid_or_boot_reuse_is_absent(monkeypatch: pytest.MonkeyPatch) -> None:
    snapshot = _snapshot()
    old_boot = _owner(10, 100, "boot-old")
    assert probe_process_owner(old_boot, snapshot) is OwnerState.ABSENT
    from host_admission import core

    monkeypatch.setattr(core, "_proc_start_ticks", lambda _pid: (OwnerState.ALIVE, 101))
    assert probe_process_owner(_owner(10, 100), snapshot) is OwnerState.ABSENT
    monkeypatch.setattr(core, "_proc_start_ticks", lambda _pid: (OwnerState.UNKNOWN, None))
    assert probe_process_owner(_owner(), snapshot) is OwnerState.UNKNOWN


def test_dead_queue_owner_is_swept_unknown_is_retained_and_conflict_refuses(tmp_path: Path) -> None:
    path = tmp_path / "host-admission.json"
    ledger = HostAdmissionLedger(path)
    policy = _policy(token_capacities=(("build", 1),))
    first = ResourceRequest("holder", "test", _owner(), 1, (("build", 1),))
    dead = ResourceRequest("dead-ticket", "test", _owner(11, 101), 1, (("build", 1),))
    granted, first_lease = ledger.request(
        first, _snapshot(), policy, now_unix_ms=1010,
        owner_probe=lambda _owner, _snapshot: OwnerState.ALIVE,
    )
    assert granted.verdict is Verdict.GRANT and first_lease is not None
    queued, _ = ledger.request(
        dead, _snapshot(), policy, now_unix_ms=1011,
        owner_probe=lambda _owner, _snapshot: OwnerState.ALIVE,
    )
    assert queued.verdict is Verdict.QUEUE
    first_lease.release()
    unknown_request = ResourceRequest("unknown-current", "test", _owner(12, 102), 1)

    def unknown_probe(owner: ProcessOwner, _snapshot: HostSnapshot) -> OwnerState:
        return OwnerState.UNKNOWN if owner.pid == dead.owner.pid else OwnerState.ALIVE

    before_unknown = path.read_bytes()
    unknown, no_lease = ledger.request(
        unknown_request, _snapshot(), policy, now_unix_ms=1012, owner_probe=unknown_probe
    )
    assert unknown.verdict is Verdict.UNKNOWN and no_lease is None
    assert path.read_bytes() == before_unknown, "unknown ownership must not mutate its queue row"

    replacement = ResourceRequest("replacement", "test", _owner(13, 103), 1)

    def absent_probe(owner: ProcessOwner, _snapshot: HostSnapshot) -> OwnerState:
        return OwnerState.ABSENT if owner.pid == dead.owner.pid else OwnerState.ALIVE

    admitted, replacement_lease = ledger.request(
        replacement, _snapshot(), policy, now_unix_ms=1013, owner_probe=absent_probe
    )
    assert admitted.verdict is Verdict.GRANT and replacement_lease is not None
    state = _mapping(json.loads(path.read_text(encoding="utf-8")))
    assert all(
        _text(_mapping(_mapping(entry)["request"])["request_id"]) != dead.request_id
        for entry in _sequence(state["queue"])
    )
    replacement_lease.release()

    blocker = ResourceRequest("holder-two", "test", _owner(14, 104), 1, (("build", 1),))
    blocker_result, blocker_lease = ledger.request(
        blocker, _snapshot(), policy, now_unix_ms=1014,
        owner_probe=lambda _owner, _snapshot: OwnerState.ALIVE,
    )
    assert blocker_result.verdict is Verdict.GRANT and blocker_lease is not None
    original = ResourceRequest("same-id", "test", _owner(15, 105), 1, (("build", 1),), metadata_digest="original")
    queued, _ = ledger.request(
        original, _snapshot(), policy, now_unix_ms=1015,
        owner_probe=lambda _owner, _snapshot: OwnerState.ALIVE,
    )
    assert queued.verdict is Verdict.QUEUE
    before_conflict = path.read_bytes()
    changed = ResourceRequest("same-id", "test", _owner(15, 105), 2, (("build", 1),), metadata_digest="changed")
    conflict, no_lease = ledger.request(
        changed, _snapshot(), policy, now_unix_ms=1016,
        owner_probe=lambda _owner, _snapshot: OwnerState.ALIVE,
    )
    assert conflict.verdict is Verdict.UNKNOWN and conflict.code == "request_id_conflict"
    assert no_lease is None and path.read_bytes() == before_conflict
    blocker_lease.release()


def test_absent_queue_owner_is_swept_even_when_a_peer_is_unknown(tmp_path: Path) -> None:
    path = tmp_path / "host-admission.json"
    ledger = HostAdmissionLedger(path)
    policy = _policy(token_capacities=(("build", 1),))
    holder = ResourceRequest("holder", "test", _owner(), 1, (("build", 1),))
    dead = ResourceRequest("dead", "test", _owner(11, 101), 1, (("build", 1),))
    uncertain = ResourceRequest(
        "uncertain", "test", _owner(12, 102), 1, (("build", 1),)
    )
    _decision, holder_lease = ledger.request(
        holder,
        _snapshot(),
        policy,
        now_unix_ms=1010,
        owner_probe=lambda _owner, _snapshot: OwnerState.ALIVE,
    )
    assert holder_lease is not None
    for queued in (dead, uncertain):
        decision, lease = ledger.request(
            queued,
            _snapshot(),
            policy,
            now_unix_ms=1011,
            owner_probe=lambda _owner, _snapshot: OwnerState.ALIVE,
        )
        assert decision.verdict is Verdict.QUEUE and lease is None
    holder_lease.release()

    def mixed_probe(owner: ProcessOwner, _snapshot: HostSnapshot) -> OwnerState:
        if owner.pid == dead.owner.pid:
            return OwnerState.ABSENT
        if owner.pid == uncertain.owner.pid:
            return OwnerState.UNKNOWN
        return OwnerState.ALIVE

    decision, lease = ledger.request(
        ResourceRequest("current", "test", _owner(13, 103), 1),
        _snapshot(),
        policy,
        now_unix_ms=1012,
        owner_probe=mixed_probe,
    )
    assert decision.verdict is Verdict.UNKNOWN and lease is None
    state = _mapping(json.loads(path.read_text(encoding="utf-8")))
    queued_ids = {
        _text(_mapping(_mapping(entry)["request"])["request_id"])
        for entry in _sequence(state["queue"])
    }
    assert queued_ids == {"uncertain"}


def test_malformed_ledger_is_never_replaced_or_granted(tmp_path: Path) -> None:
    path = tmp_path / "host-admission.json"
    original = b'{"schema":"host-admission-ledger/v1","schema":"wrong"}\n'
    path.write_bytes(original)
    path.chmod(0o600)
    ledger = HostAdmissionLedger(path)
    with pytest.raises(AdmissionError):
        ledger.request(ResourceRequest("request", "test", _owner(), 1), _snapshot(), _policy(), now_unix_ms=1010)
    assert path.read_bytes() == original


def test_ledger_parent_and_final_components_fail_closed(tmp_path: Path) -> None:
    request = ResourceRequest("request", "test", _owner(), 1)
    target = tmp_path / "target"
    target.mkdir()
    target.chmod(0o700)
    linked = tmp_path / "linked"
    linked.symlink_to(target, target_is_directory=True)
    with pytest.raises(AdmissionError):
        HostAdmissionLedger(linked / "ledger.json").request(
            request, _snapshot(), _policy(), now_unix_ms=1010
        )

    writable = tmp_path / "writable"
    writable.mkdir()
    writable.chmod(0o777)
    with pytest.raises(AdmissionError):
        HostAdmissionLedger(writable / "ledger.json").request(
            request, _snapshot(), _policy(), now_unix_ms=1010
        )

    final_parent = tmp_path / "final"
    final_parent.mkdir()
    final_parent.chmod(0o700)
    payload = final_parent / "payload"
    payload.write_text("{}", encoding="utf-8")
    payload.chmod(0o600)
    (final_parent / "symlink.json").symlink_to(payload)
    with pytest.raises(AdmissionError):
        HostAdmissionLedger(final_parent / "symlink.json").request(
            request, _snapshot(), _policy(), now_unix_ms=1010
        )
    os.link(payload, final_parent / "hardlink.json")
    with pytest.raises(AdmissionError):
        HostAdmissionLedger(final_parent / "hardlink.json").request(
            request, _snapshot(), _policy(), now_unix_ms=1010
        )

    lock_target = final_parent / "lock-target"
    lock_target.write_text("", encoding="utf-8")
    lock_target.chmod(0o600)
    os.link(lock_target, final_parent / "lock-hardlink.json.lock")
    with pytest.raises(AdmissionError):
        HostAdmissionLedger(final_parent / "lock-hardlink.json").request(
            request, _snapshot(), _policy(), now_unix_ms=1010
        )


def test_descriptor_binding_detects_lock_unlink_and_contains_parent_retarget(
    tmp_path: Path,
) -> None:
    parent = tmp_path / "parent"
    moved = tmp_path / "moved"
    attacker = tmp_path / "attacker"
    parent.mkdir()
    parent.chmod(0o700)
    attacker.mkdir()
    attacker.chmod(0o700)
    ledger = HostAdmissionLedger(parent / "ledger.json")

    def unlink_lock(_owner: ProcessOwner, _snapshot: HostSnapshot) -> OwnerState:
        (parent / "ledger.json.lock").unlink()
        return OwnerState.ALIVE

    with pytest.raises(AdmissionError):
        ledger.request(
            ResourceRequest("unlink", "test", _owner(), 1),
            _snapshot(),
            _policy(),
            now_unix_ms=1010,
            owner_probe=unlink_lock,
        )
    assert not (parent / "ledger.json").exists()

    def retarget_parent(_owner: ProcessOwner, _snapshot: HostSnapshot) -> OwnerState:
        parent.rename(moved)
        parent.symlink_to(attacker, target_is_directory=True)
        return OwnerState.ALIVE

    decision, lease = ledger.request(
        ResourceRequest("retarget", "test", _owner(), 1),
        _snapshot(),
        _policy(),
        now_unix_ms=1011,
        owner_probe=retarget_parent,
    )
    assert decision.verdict is Verdict.GRANT and lease is not None
    assert (moved / "ledger.json").is_file()
    assert not (attacker / "ledger.json").exists()
    HostAdmissionLedger(moved / "ledger.json").release("retarget", _owner())


def test_bare_relative_ledger_path_commits_without_ambiguous_error(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    tmp_path.chmod(0o700)
    monkeypatch.chdir(tmp_path)
    ledger = HostAdmissionLedger(Path("ledger.json"))
    decision, lease = ledger.request(
        ResourceRequest("relative", "test", _owner(), 1),
        _snapshot(),
        _policy(),
        now_unix_ms=1010,
        owner_probe=lambda _owner, _snapshot: OwnerState.ALIVE,
    )
    assert decision.verdict is Verdict.GRANT and lease is not None
    ledger.validate()
    lease.release()


def test_malformed_queue_sequences_and_duplicate_tokens_are_rejected(tmp_path: Path) -> None:
    request = ResourceRequest("queued", "test", _owner(), 1, (("build", 1),))
    base = {
        "leases": [],
        "next_sequence": 1,
        "queue": [{"request": request.to_json(), "sequence": 1}],
        "schema": "host-admission-ledger/v1",
    }
    invalid_sequence = (json.dumps(base, sort_keys=True, separators=(",", ":")) + "\n").encode()
    duplicate_token = invalid_sequence.replace(
        b'"named_tokens":{"build":1}', b'"named_tokens":{"build":1,"build":2}'
    ).replace(b'"sequence":1', b'"sequence":0')
    for index, original in enumerate((invalid_sequence, duplicate_token)):
        path = tmp_path / f"invalid-{index}.json"
        path.write_bytes(original)
        path.chmod(0o600)
        ledger = HostAdmissionLedger(path)
        with pytest.raises(AdmissionError):
            ledger.request(
                ResourceRequest("current", "test", _owner(20 + index, 200 + index), 1),
                _snapshot(),
                _policy(),
                now_unix_ms=1010,
                owner_probe=lambda _owner, _snapshot: OwnerState.ALIVE,
            )
        assert path.read_bytes() == original


def test_queue_sequence_exhaustion_is_unknown_without_mutation(tmp_path: Path) -> None:
    path = tmp_path / "host-admission.json"
    payload = {
        "leases": [],
        "next_sequence": (1 << 64) - 1,
        "queue": [],
        "schema": "host-admission-ledger/v1",
    }
    original = (json.dumps(payload, sort_keys=True, separators=(",", ":")) + "\n").encode()
    path.write_bytes(original)
    path.chmod(0o600)
    decision, lease = HostAdmissionLedger(path).request(
        ResourceRequest("current", "test", _owner(), 1),
        _snapshot(),
        _policy(),
        now_unix_ms=1010,
        owner_probe=lambda _owner, _snapshot: OwnerState.ALIVE,
    )
    assert decision.verdict is Verdict.UNKNOWN
    assert decision.code == "queue_sequence_exhausted"
    assert lease is None and path.read_bytes() == original


def test_record_capacity_boundary_is_non_mutating_and_recovers_after_sweep(
    tmp_path: Path,
) -> None:
    from host_admission import core

    path = tmp_path / "host-admission.json"
    queue = [
        {
            "request": ResourceRequest(
                f"queued-{index}",
                "test",
                _owner(100 + index, 1000 + index),
                0,
            ).to_json(),
            "sequence": index,
        }
        for index in range(core.MAX_RECORDS)
    ]
    payload = {
        "leases": [],
        "next_sequence": core.MAX_RECORDS,
        "queue": queue,
        "schema": "host-admission-ledger/v1",
    }
    original = (json.dumps(payload, sort_keys=True, separators=(",", ":")) + "\n").encode()
    assert len(original) < core.MAX_LEDGER_BYTES
    near_payload = dict(payload)
    near_payload["queue"] = queue[:-1]
    near = (
        json.dumps(near_payload, sort_keys=True, separators=(",", ":")) + "\n"
    ).encode()
    path.write_bytes(near)
    path.chmod(0o600)
    near_current = ResourceRequest("near-current", "test", _owner(), 1, (), 10)
    near_decision, near_lease = HostAdmissionLedger(path).request(
        near_current,
        _snapshot(),
        _policy(),
        now_unix_ms=1009,
        owner_probe=lambda _owner, _snapshot: OwnerState.ALIVE,
    )
    assert near_decision.verdict is Verdict.GRANT and near_lease is not None
    near_state = _mapping(json.loads(path.read_text(encoding="utf-8")))
    assert (
        len(_sequence(near_state["leases"]))
        + len(_sequence(near_state["queue"]))
        == core.MAX_RECORDS
    )

    path.write_bytes(original)
    path.chmod(0o600)
    ledger = HostAdmissionLedger(path)
    current = ResourceRequest("current", "test", _owner(), 1, (), 10)
    decision, lease = ledger.request(
        current,
        _snapshot(),
        _policy(),
        now_unix_ms=1010,
        owner_probe=lambda _owner, _snapshot: OwnerState.ALIVE,
    )
    assert decision.verdict is Verdict.UNKNOWN
    assert decision.code == "record_capacity_exhausted"
    assert lease is None and path.read_bytes() == original

    first_pid = 100
    decision, lease = ledger.request(
        current,
        _snapshot(),
        _policy(),
        now_unix_ms=1011,
        owner_probe=lambda owner, _snapshot: (
            OwnerState.ABSENT if owner.pid == first_pid else OwnerState.ALIVE
        ),
    )
    assert decision.verdict is Verdict.GRANT and lease is not None
    state = _mapping(json.loads(path.read_text(encoding="utf-8")))
    assert len(_sequence(state["leases"])) + len(_sequence(state["queue"])) == core.MAX_RECORDS
    lease.release()


def test_psi_decimal_parser_is_exact_and_rejects_hidden_precision() -> None:
    from host_admission import core

    assert core._parse_percent_micros("0.000001") == 1
    assert core._parse_percent_micros("100.000000") == 100_000_000
    assert core._parse_percent_micros("1.0000000") == 1_000_000
    assert core._parse_percent_micros("1.0000001") is None
    assert core._parse_percent_micros("+1") is None
    assert core._parse_percent_micros("1.") is None


def test_python_integer_domains_match_the_wire_types() -> None:
    with pytest.raises(ValueError):
        ProcessOwner("host-a", "boot-a", True, 1)
    with pytest.raises(ValueError):
        ProcessOwner("host-a", "boot-a", 1 << 32, 1)
    with pytest.raises(ValueError):
        ProcessOwner("host-a", "boot-a", 1, 1 << 64)
    with pytest.raises(ValueError):
        HostSnapshot(1 << 64, "host-a", "boot-a", 1, 1, 0, 0, 0, 0)
    with pytest.raises(ValueError):
        HostSnapshot(1, "host-a", "boot-a", 1, 1, 0, 0, 100_000_001, 0)
    with pytest.raises(ValueError):
        ResourceRequest("current", "test", _owner(), True)
    with pytest.raises(ValueError):
        decide(
            ResourceRequest("current", "test", _owner(), 1),
            _snapshot(),
            _policy(),
            (),
            (),
            now_unix_ms=1 << 64,
            owner_states={},
        )


def test_python_enums_owner_states_and_snapshot_relations_fail_closed() -> None:
    with pytest.raises(ValueError):
        AdmissionPolicy(
            memory_budget_bytes=700,
            memory_reserve_bytes=0,
            swap_mode=cast(SwapMode, "floor"),
        )
    peer = ResourceRequest("peer", "test", _owner(), 700)
    with pytest.raises(ValueError):
        decide(
            ResourceRequest("current", "test", _owner(11, 101), 1),
            _snapshot(),
            _policy(),
            (LeaseView("peer", peer, 1),),
            (),
            now_unix_ms=1010,
            owner_states={"peer": cast(OwnerState, "unknown")},
        )
    with pytest.raises(ValueError):
        HostSnapshot(1, "host-a", "boot-a", 1, 2, 0, 0, 0, 0)
    with pytest.raises(ValueError):
        HostSnapshot(1, "host-a", "boot-a", 1, 1, 1, 2, 0, 0)


def test_invalid_owner_probe_state_fails_closed_without_writing(tmp_path: Path) -> None:
    path = tmp_path / "ledger.json"
    with pytest.raises(AdmissionError):
        HostAdmissionLedger(path).request(
            ResourceRequest("current", "test", _owner(), 1),
            _snapshot(),
            _policy(),
            now_unix_ms=1010,
            owner_probe=lambda _owner, _snapshot: cast(OwnerState, "alive"),
        )
    assert not path.exists()


def test_host_snapshot_timestamp_precedes_all_measurement_reads(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from host_admission import core

    events: list[str] = []

    def fake_time() -> int:
        events.append("timestamp")
        return 1_000_000_000

    def fake_read(path: Path, **_kwargs: object) -> str:
        assert events and events[0] == "timestamp"
        events.append(str(path))
        if str(path) == "/proc/meminfo":
            return "MemTotal: 1 kB\nMemAvailable: 1 kB\nSwapTotal: 0 kB\nSwapFree: 0 kB\n"
        if str(path) == "/proc/pressure/memory":
            return "some avg10=0 total=0\nfull avg10=0 total=0\n"
        return "host-a" if str(path) == "/etc/machine-id" else "boot-a"

    monkeypatch.setattr(time, "time_ns", fake_time)
    monkeypatch.setattr(Path, "read_text", fake_read)
    snapshot = core.sample_host()
    assert snapshot.captured_at_unix_ms == 1000
    assert events[0] == "timestamp"


def test_measurement_parsers_reject_malformed_and_duplicate_fields(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from host_admission import core

    monkeypatch.setattr(
        Path,
        "read_text",
        lambda path, **_kwargs: (
            "MemTotal: abc kB\nMemAvailable: 1 kB\nSwapTotal: 0 kB\nSwapFree: 0 kB\n"
            if str(path) == "/proc/meminfo"
            else "some avg10=0.1 avg10=0.2 total=1\nfull avg10=0 total=0\n"
        ),
    )
    _values, mem_errors = core._read_meminfo()
    _some, _full, psi_errors = core._read_psi()
    assert "mem_total_malformed" in mem_errors
    assert "memory_psi_some_malformed" in psi_errors


def test_mutations_of_fifo_owner_and_token_guards_change_the_decision() -> None:
    request = ResourceRequest("current", "test", _owner(12, 102), 1, (("build", 1),), 5)
    policy = _policy(token_capacities=(("build", 1),))
    peer = ResourceRequest("peer", "test", _owner(), 1, (("build", 1),), 5)
    leases = (LeaseView("peer", peer, 1),)
    queue = (QueueView(ResourceRequest("earlier", "test", _owner(11, 101), 1, (), 5), 1), QueueView(request, 2))
    unknown = decide(request, _snapshot(), policy, leases, queue, now_unix_ms=1010, owner_states={"peer": OwnerState.UNKNOWN})
    assert unknown.verdict is Verdict.UNKNOWN
    absent = decide(
        request,
        _snapshot(),
        policy,
        leases,
        queue,
        now_unix_ms=1010,
        owner_states={"peer": OwnerState.ABSENT, "earlier": OwnerState.ALIVE},
    )
    assert absent.code == "queued_behind_prior_request"
    no_fifo = decide(request, _snapshot(), policy, leases, (QueueView(request, 2),), now_unix_ms=1010, owner_states={"peer": OwnerState.ABSENT})
    assert no_fifo.verdict is Verdict.GRANT
    token_busy = decide(request, _snapshot(), policy, leases, (QueueView(request, 2),), now_unix_ms=1010, owner_states={"peer": OwnerState.ALIVE})
    assert token_busy.code == "named_tokens_busy"


def test_dagrun_compatibility_adapter_preserves_legacy_authority() -> None:
    cases = [
        (701, 700, 1000, 0, LegacyVerdict.REFUSE),
        (400, 700, 1000, 400, LegacyVerdict.QUEUE),
        (400, 700, 399, 0, LegacyVerdict.QUEUE),
        (400, 700, 400, 0, LegacyVerdict.GRANT),
        (1 << 40, None, None, 0, LegacyVerdict.GRANT),
    ]
    for requested, budget, headroom, reserved, expected in cases:
        comparison = compare_legacy_memory_decision(requested, budget_bytes=budget, headroom_bytes=headroom, reserved_bytes=reserved)
        assert comparison.authoritative_verdict is expected
        assert comparison.shadow_verdict is Verdict.UNKNOWN
    assert compare_legacy_memory_decision(1 << 40, budget_bytes=None, headroom_bytes=None, reserved_bytes=0).shadow_verdict is Verdict.UNKNOWN


def test_snapshot_and_diagnostics_never_echo_caller_metadata() -> None:
    request = ResourceRequest("opaque-id", "safe-caller", _owner(), 701, (), 0, "digest-only")
    decision = decide(request, _snapshot(), _policy(), (), (), now_unix_ms=1010, owner_states={})
    encoded = canonical_decision_json(decision)
    assert "safe-caller" not in encoded and "digest-only" not in encoded
    assert decision.verdict is Verdict.REFUSE
    assert os.linesep == "\n"
