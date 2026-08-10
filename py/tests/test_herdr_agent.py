"""Focused contract tests for the shared interactive-agent queue."""

from __future__ import annotations

import json
import os
import stat
import threading
from pathlib import Path
from typing import cast

import pytest

from herdr_run import __version__
import herdr_run.agent as agent_api
from herdr_run.agent import Target, drain, enqueue, read, send, status
from herdr_run.agent import QueueResult
import herdr_run.agent_cli as agent_cli
from herdr_run.client import AgentPaneInfo, HerdrClient, Pane
from herdr_run.errors import AgentDeliveryError, AgentPending, AgentPossiblySubmitted, HerdrUnavailable


def test_agent_cli_version(capsys: pytest.CaptureFixture[str]) -> None:
    with pytest.raises(SystemExit) as exc_info:
        agent_cli.main(["--version"])
    assert exc_info.value.code == 0
    assert capsys.readouterr().out == f"herdr-agent {__version__}\n"


def test_agent_cli_userguide_option(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(agent_cli, "_guide", lambda: 0)
    assert agent_cli.main(["--userguide"]) == 0


def test_agent_cli_bare_invocation_is_a_successful_orientation(
    capsys: pytest.CaptureFixture[str],
) -> None:
    assert agent_cli.main([]) == 0
    captured = capsys.readouterr()
    assert "send" in captured.out
    assert "--session" in captured.out
    assert captured.err == ""


@pytest.mark.parametrize(
    "arguments",
    [
        ["status", "--pane", "p", "--ready-timeout", "nan"],
        ["status", "--pane", "p", "--working-timeout", "inf"],
        ["status", "--pane", "p", "--ready-timeout", "31536001"],
    ],
)
def test_agent_cli_rejects_nonfinite_or_excessive_waits(arguments: list[str]) -> None:
    with pytest.raises(SystemExit) as exc_info:
        agent_cli.main(arguments)
    assert exc_info.value.code == 2


@pytest.mark.parametrize(
    "arguments",
    [
        ["status", "--pane", "--help"],
        ["status", "--pane", "--version"],
        ["status", "--pane", "--queue", "state"],
        ["status", "--lines", "1_0"],
        ["status", "--lines", "١٢"],
        ["status", "--lines", "1000001"],
        ["status", "--ready-timeout", "1_0"],
        ["status", "--ready-timeout", "١.0"],
    ],
)
def test_agent_cli_rejects_stolen_options_and_non_ascii_or_unbounded_numbers(
    arguments: list[str],
) -> None:
    with pytest.raises(SystemExit) as exc_info:
        agent_cli.main(arguments)
    assert exc_info.value.code == 2


def test_agent_cli_accepts_documented_ascii_numeric_forms() -> None:
    args = agent_cli._parser().parse_intermixed_args(
        ["status", "--pane=p", "--ready-timeout=.5", "--lines=00017"]
    )
    assert args.ready_timeout == 0.5
    assert args.lines == 17


class FakeAgentHerdr:
    def __init__(self, states: list[str] | None = None) -> None:
        self.states = states or ["idle"]
        self.index = 0
        self.runs: list[str] = []
        self.waits: list[tuple[str, str, int]] = []
        self.workspace = "deepscry"
        self.cwd = "/work/mtg"
        self.session = "session-1"
        self.read_text = "agent transcript\n"
        self.run_entered = threading.Event()
        self.run_release: threading.Event | None = None

    def panes(self, workspace_id: str | None = None) -> tuple[Pane, ...]:
        del workspace_id
        return (Pane("w1:p1", "w1:t1", "w1"),)

    def pane_info(self, pane_id: str) -> AgentPaneInfo:
        assert pane_id == "w1:p1"
        state = self.states[min(self.index, len(self.states) - 1)]
        self.index += 1
        return AgentPaneInfo(pane_id, "w1", self.cwd, "codex", state, "codex", self.session)

    def workspace_label(self, workspace_id: str) -> str:
        assert workspace_id == "w1"
        return self.workspace

    def run(self, pane_id: str, text: str) -> None:
        assert pane_id == "w1:p1"
        self.run_entered.set()
        if self.run_release is not None:
            assert self.run_release.wait(5)
        self.runs.append(text)

    def wait_agent_status(self, pane_id: str, state: str, timeout_ms: int) -> None:
        self.waits.append((pane_id, state, timeout_ms))
        if state != "working":
            raise AssertionError(state)

    def read(self, pane_id: str, *, source: str, lines: int) -> str:
        assert pane_id == "w1:p1" and source in ("recent-unwrapped", "recent") and lines == 17
        return self.read_text


def client(fake: FakeAgentHerdr) -> HerdrClient:
    return cast(HerdrClient, fake)


def target(**changes: str) -> Target:
    values = {
        "pane_id": "w1:p1", "session_agent": "codex", "session_value": "session-1",
        "expected_agent": "codex", "expected_workspace": "deepscry", "expected_cwd": "/work/mtg",
    }
    values.update(changes)
    return Target(**values)


def test_multiline_busy_then_idle_is_atomic_and_confirmed(tmp_path: object) -> None:
    fake = FakeAgentHerdr(["working", "working", "idle"])
    text = "first line\nsecond line\nthird line"
    result = send(client(fake), target(), str(tmp_path), text, ready_timeout=1, sleep=lambda _s: None)
    assert fake.runs == [text]
    assert fake.waits == [("w1:p1", "working", 30000)]
    assert result.message_id in result.delivered


def test_busy_for_more_than_thirty_seconds_still_delivers_with_turn_sized_budget(tmp_path: object) -> None:
    # Session resolution reads the matching pane once while listing and once for final
    # validation, both before and after acquiring the canonical pane lock.
    fake = FakeAgentHerdr(["working", "working", "working", "working", "idle", "idle"])
    clock = {"now": 0.0}

    def monotonic() -> float:
        return clock["now"]

    def sleep(_seconds: float) -> None:
        clock["now"] += 31.0

    result = send(
        client(fake), target(), str(tmp_path), "after a normal turn",
        ready_timeout=900, sleep=sleep, monotonic=monotonic,
    )
    assert clock["now"] > 30
    assert result.delivered == (result.message_id,)


def test_done_is_submit_safe(tmp_path: object) -> None:
    fake = FakeAgentHerdr(["done"])
    send(client(fake), target(), str(tmp_path), "from done")
    assert fake.runs == ["from done"]


def test_concurrent_senders_are_serialized_and_fifo(tmp_path: object) -> None:
    root = str(tmp_path)
    fake = FakeAgentHerdr(["idle"])
    fake.run_release = threading.Event()
    errors: list[BaseException] = []

    def invoke(text: str) -> None:
        try:
            send(client(fake), target(), root, text)
        except BaseException as exc:
            errors.append(exc)

    first = threading.Thread(target=invoke, args=("first",), daemon=True)
    second = threading.Thread(target=invoke, args=("second",), daemon=True)
    first.start()
    assert fake.run_entered.wait(5)
    second.start()
    second.join(0.05)
    assert second.is_alive()
    fake.run_release.set()
    first.join(5)
    second.join(5)
    assert errors == []
    assert fake.runs == ["first", "second"]


def test_distinct_queue_roots_serialize_one_shared_target(tmp_path: Path) -> None:
    fake = FakeAgentHerdr(["idle"])
    fake.run_release = threading.Event()
    errors: list[BaseException] = []

    def invoke(root: Path, text: str) -> None:
        try:
            send(client(fake), target(), str(root), text)
        except BaseException as exc:
            errors.append(exc)

    first = threading.Thread(target=invoke, args=(tmp_path / "queue-a", "from a"), daemon=True)
    second = threading.Thread(target=invoke, args=(tmp_path / "queue-b", "from b"), daemon=True)
    first.start()
    assert fake.run_entered.wait(5)
    second.start()
    second.join(0.05)
    assert second.is_alive(), "a different queue root must wait on the host-wide target lock"
    assert fake.runs == []
    fake.run_release.set()
    first.join(5)
    second.join(5)
    assert errors == []
    assert fake.runs == ["from a", "from b"]


def test_exact_pane_and_session_forms_share_one_target_lock(tmp_path: Path) -> None:
    fake = FakeAgentHerdr(["idle"])
    fake.run_release = threading.Event()
    errors: list[BaseException] = []
    pane_target = Target(pane_id="w1:p1")
    session_target = Target(session_agent="codex", session_value="session-1")

    def invoke(root: Path, selected: Target, text: str) -> None:
        try:
            send(client(fake), selected, str(root), text)
        except BaseException as exc:
            errors.append(exc)

    first = threading.Thread(
        target=invoke, args=(tmp_path / "pane-queue", pane_target, "from pane"), daemon=True
    )
    second = threading.Thread(
        target=invoke,
        args=(tmp_path / "session-queue", session_target, "from session"),
        daemon=True,
    )
    first.start()
    assert fake.run_entered.wait(5)
    second.start()
    second.join(0.05)
    assert second.is_alive(), "session targeting bypassed the resolved-pane lock"
    fake.run_release.set()
    first.join(5)
    second.join(5)
    assert errors == []
    assert fake.runs == ["from pane", "from session"]


def test_working_confirmation_failure_never_resubmits_and_marks_ambiguous(tmp_path: object) -> None:
    fake = FakeAgentHerdr(["idle"])

    def fail(*_args: object) -> None:
        raise HerdrUnavailable("no working transition")

    fake.wait_agent_status = fail  # type: ignore[assignment]
    with pytest.raises(AgentDeliveryError, match="retained"):
        send(client(fake), target(), str(tmp_path), "preserve me", max_attempts=2)
    failed = list((tmp_path / "failed").glob("*.json"))  # type: ignore[operator]
    assert len(failed) == 1
    document = json.loads(failed[0].read_text())
    assert document["text"] == "preserve me"
    assert document["delivery_attempts"] == 1
    assert document["possibly_submitted"] is True
    assert len(fake.runs) == 1


def test_send_inspects_terminal_artifact_when_another_drain_consumed_its_id(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    original_enqueue = agent_api._enqueue

    def consumed_enqueue(
        root: str,
        text: str,
        *,
        message_id: str | None,
        serialize: bool,
    ) -> str:
        del message_id, serialize
        identifier = original_enqueue(
            root, text, message_id="cross-drained", serialize=True
        )
        source = tmp_path / "inbox/cross-drained.json"
        document = json.loads(source.read_text())
        document["delivery_error"] = "other drain lost confirmation"
        destination = tmp_path / "failed/cross-drained.json"
        source.replace(destination)
        destination.write_text(json.dumps(document), encoding="utf-8")
        return identifier

    monkeypatch.setattr(agent_api, "_enqueue", consumed_enqueue)
    monkeypatch.setattr(
        agent_api,
        "drain",
        lambda *_args, **_kwargs: QueueResult("", (), (), (), None, "delivered"),
    )

    with pytest.raises(AgentPossiblySubmitted, match="other drain lost confirmation"):
        agent_api.send(client(FakeAgentHerdr()), target(), str(tmp_path), "raced")


@pytest.mark.parametrize(
    "change,expected",
    [
        ({"session_value": "wrong"}, "found 0"),
        ({"expected_workspace": "wrong"}, "workspace"),
        ({"expected_cwd": "/wrong"}, "cwd"),
        ({"expected_agent": "claude"}, "agent"),
    ],
)
def test_wrong_identity_workspace_or_cwd_refuses(
    tmp_path: object, change: dict[str, str], expected: str
) -> None:
    fake = FakeAgentHerdr(["idle"])
    with pytest.raises(AgentDeliveryError, match=expected):
        send(client(fake), target(**change), str(tmp_path), "never submit", max_attempts=1)
    assert fake.runs == []
    assert len(list((tmp_path / "inbox").glob("*.json"))) == 1  # type: ignore[operator]
    assert len(list((tmp_path / "failed").glob("*.json"))) == 0  # type: ignore[operator]


def test_session_resolution_cannot_silently_override_asserted_pane(tmp_path: object) -> None:
    fake = FakeAgentHerdr(["idle"])
    with pytest.raises(AgentDeliveryError, match="expected exact pane"):
        send(client(fake), target(pane_id="w9:p9"), str(tmp_path), "never retarget")
    assert fake.runs == []


@pytest.mark.parametrize("identifier", ["../escape", "/absolute", ".hidden", "bad/name", ""])
def test_enqueue_rejects_unsafe_message_ids(tmp_path: object, identifier: str) -> None:
    with pytest.raises(AgentDeliveryError, match="message id"):
        enqueue(str(tmp_path), "safe text", message_id=identifier)


def test_concurrent_explicit_message_id_is_created_exactly_once(tmp_path: Path) -> None:
    barrier = threading.Barrier(2)
    successes: list[str] = []
    failures: list[BaseException] = []

    def create() -> None:
        barrier.wait()
        try:
            successes.append(enqueue(str(tmp_path), "one durable value", message_id="same-id"))
        except BaseException as exc:
            failures.append(exc)

    threads = [threading.Thread(target=create) for _ in range(2)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join(5)

    assert successes == ["same-id"]
    assert len(failures) == 1
    assert isinstance(failures[0], AgentDeliveryError)
    assert json.loads((tmp_path / "inbox/same-id.json").read_text())["text"] == "one durable value"


@pytest.mark.parametrize("missing", [Target(), Target(pane_id="")])
def test_missing_target_is_rejected_before_queue_mutation(
    tmp_path: Path, missing: Target
) -> None:
    root = tmp_path / "queue"
    with pytest.raises(AgentDeliveryError, match="target needs"):
        send(client(FakeAgentHerdr()), missing, str(root), "do not strand me")
    assert not root.exists()


@pytest.mark.parametrize(
    "payload",
    [
        '{"id":"bad","text":NaN}\n',
        '{"id":"bad","text":"prompt","delivery_attempts":true}\n',
        '{"id":"bad","text":""}\n',
    ],
)
def test_nonstandard_or_unsafe_queue_documents_are_quarantined(
    tmp_path: Path, payload: str
) -> None:
    inbox = tmp_path / "inbox"
    inbox.mkdir(parents=True, mode=0o700)
    (inbox / "bad.json").write_text(payload, encoding="utf-8")

    result = drain(client(FakeAgentHerdr()), target(), str(tmp_path))

    assert result.quarantined == ("bad",)
    assert list((tmp_path / "failed").glob("bad.json"))


def test_exhausted_fifo_head_is_pending_not_success(tmp_path: Path) -> None:
    inbox = tmp_path / "inbox"
    inbox.mkdir(parents=True, mode=0o700)
    (inbox / "exhausted.json").write_text(
        '{"id":"exhausted","text":"keep me","delivery_attempts":1}\n',
        encoding="utf-8",
    )

    result = drain(
        client(FakeAgentHerdr()), target(), str(tmp_path), max_attempts=1
    )

    assert result.outcome == "pending"
    assert result.pending == ("exhausted",)
    assert result.blocked is not None and "maximum delivery-attempt" in result.blocked


def test_fifo_queue_artifact_is_quarantined_without_blocking(tmp_path: Path) -> None:
    inbox = tmp_path / "inbox"
    inbox.mkdir(parents=True, mode=0o700)
    os.mkfifo(inbox / "pipe.json", mode=0o600)

    result = drain(client(FakeAgentHerdr()), target(), str(tmp_path))

    assert result.quarantined == ("pipe",)
    assert stat.S_ISFIFO((tmp_path / "failed/pipe.json").lstat().st_mode)


def test_queue_directories_are_tightened_and_symlink_lock_is_refused(tmp_path: Path) -> None:
    inbox = tmp_path / "inbox"
    inbox.mkdir(mode=0o755)
    enqueue(str(tmp_path), "queued")
    assert stat.S_IMODE(inbox.stat().st_mode) == 0o700

    victim = tmp_path / "victim"
    victim.write_text("do not touch", encoding="utf-8")
    (tmp_path / ".delivery.lock").unlink()
    (tmp_path / ".delivery.lock").symlink_to(victim)
    with pytest.raises(AgentDeliveryError, match="cannot open queue delivery lock"):
        drain(client(FakeAgentHerdr(["idle"])), target(), str(tmp_path))
    assert victim.read_text(encoding="utf-8") == "do not touch"


def test_timeout_retains_prompt_and_loud_error(tmp_path: object) -> None:
    fake = FakeAgentHerdr(["working"])
    with pytest.raises(AgentDeliveryError, match="remains pending"):
        send(client(fake), target(), str(tmp_path), "poll artifact survives", ready_timeout=0, max_attempts=1)
    pending = next((tmp_path / "inbox").glob("*.json"))  # type: ignore[operator]
    document = json.loads(pending.read_text())
    assert document["text"] == "poll artifact survives"
    assert document["delivery_attempts"] == 0
    assert list((tmp_path / "failed").glob("*.json")) == []  # type: ignore[operator]


def test_fast_idle_working_idle_transition_is_confirmed_without_screen_matching(tmp_path: object) -> None:
    fake = FakeAgentHerdr(["idle", "idle"])

    def fast_transition(pane_id: str, state: str, timeout_ms: int) -> None:
        assert (pane_id, state, timeout_ms) == ("w1:p1", "working", 30000)
        # Native wait observed working even though a later point probe is already idle.
        fake.states = ["idle"]

    fake.wait_agent_status = fast_transition  # type: ignore[method-assign]
    result = send(client(fake), target(), str(tmp_path), "fast turn")
    assert result.delivered == (result.message_id,)
    assert fake.runs == ["fast turn"]
    assert status(client(fake), target(), str(tmp_path))["agent_status"] == "idle"


def test_post_run_transport_crash_is_quarantined_once_as_possibly_submitted(tmp_path: object) -> None:
    fake = FakeAgentHerdr(["idle"])

    def crash_after_possible_accept(_pane_id: str, text: str) -> None:
        fake.runs.append(text)
        raise RuntimeError("connection vanished after write")

    fake.run = crash_after_possible_accept  # type: ignore[assignment]
    with pytest.raises(AgentDeliveryError, match="may have been submitted"):
        send(client(fake), target(), str(tmp_path), "only once", max_attempts=3)
    assert fake.runs == ["only once"]
    document = json.loads(next((tmp_path / "failed").glob("*.json")).read_text())  # type: ignore[operator]
    assert document["possibly_submitted"] is True
    assert document["delivery_attempts"] == 1


def test_queue_binding_refuses_a_different_session_without_moving_prompt(tmp_path: object) -> None:
    fake = FakeAgentHerdr(["working"])
    with pytest.raises(AgentDeliveryError, match="remains pending"):
        send(client(fake), target(), str(tmp_path), "bound prompt", ready_timeout=0)
    with pytest.raises(AgentDeliveryError, match="bound to"):
        drain(client(fake), target(session_value="different"), str(tmp_path))
    pending = next((tmp_path / "inbox").glob("*.json"))  # type: ignore[operator]
    assert "bound prompt" in pending.read_text()


def test_status_and_read_cover_arbitrary_target(tmp_path: object) -> None:
    fake = FakeAgentHerdr(["idle"])
    queued = enqueue(str(tmp_path), "queued")
    snapshot = status(client(fake), target(), str(tmp_path))
    assert snapshot["pending"] == [queued]
    assert snapshot["session_value"] == "session-1"
    assert read(client(fake), target(), lines=17) == "agent transcript\n"


def test_status_on_absent_queue_creates_no_files(tmp_path: Path) -> None:
    root = tmp_path / "does-not-exist"
    fake = FakeAgentHerdr(["idle"])
    snapshot = status(client(fake), target(), str(root))
    assert snapshot["pending"] == []
    assert snapshot["inflight"] == []
    assert snapshot["failed"] == []
    assert not root.exists()


def test_status_refuses_symlinked_queue_or_binding_without_touching_target(tmp_path: Path) -> None:
    real = tmp_path / "real"
    real.mkdir(mode=0o700)
    alias = tmp_path / "alias"
    alias.symlink_to(real, target_is_directory=True)
    fake = FakeAgentHerdr(["idle"])
    with pytest.raises(AgentDeliveryError, match="unsafe queue directory"):
        status(client(fake), target(), str(alias))

    binding = real / "target.json"
    victim = tmp_path / "victim.json"
    victim.write_text("{}", encoding="utf-8")
    binding.symlink_to(victim)
    with pytest.raises(AgentDeliveryError, match="cannot read queue target binding"):
        status(client(fake), target(), str(real))
    assert victim.read_text(encoding="utf-8") == "{}"


def test_status_validates_processed_directory_too(tmp_path: Path) -> None:
    enqueue(str(tmp_path), "queued")
    processed = tmp_path / "processed"
    processed.chmod(0o755)

    with pytest.raises(AgentDeliveryError, match="queue state directory is not private"):
        status(client(FakeAgentHerdr()), target(), str(tmp_path))

    assert stat.S_IMODE(processed.stat().st_mode) == 0o755


def test_status_read_only_validates_existing_binding(tmp_path: Path) -> None:
    fake = FakeAgentHerdr(["working"])
    with pytest.raises(AgentPending):
        send(client(fake), target(), str(tmp_path), "bound", ready_timeout=0)
    before = sorted(str(path.relative_to(tmp_path)) for path in tmp_path.rglob("*"))
    with pytest.raises(AgentDeliveryError, match="bound to"):
        status(client(fake), target(session_value="different"), str(tmp_path))
    after = sorted(str(path.relative_to(tmp_path)) for path in tmp_path.rglob("*"))
    assert after == before


def test_read_falls_back_when_unwrapped_source_is_empty(tmp_path: object) -> None:
    fake = FakeAgentHerdr(["idle"])
    sources: list[str] = []

    def source_read(_pane_id: str, *, source: str, lines: int) -> str:
        assert lines == 17
        sources.append(source)
        return "" if source == "recent-unwrapped" else "fallback transcript\n"

    fake.read = source_read  # type: ignore[assignment]
    assert read(client(fake), target(), lines=17) == "fallback transcript\n"
    assert sources == ["recent-unwrapped", "recent"]


def test_drain_accepts_existing_subagent_message_shape(tmp_path: object) -> None:
    inbox = tmp_path / "inbox"  # type: ignore[operator]
    inbox.mkdir()
    path = inbox / "000000000007.json"
    path.write_text(json.dumps({"seq": 7, "text": "legacy fifo", "tui_delivery_attempts": 0}))
    fake = FakeAgentHerdr(["idle"])
    result = drain(client(fake), target(), str(tmp_path))
    assert result.delivered == ("000000000007",)
    assert fake.runs == ["legacy fifo"]
    assert os.path.isfile(tmp_path / "processed" / path.name)  # type: ignore[operator]


def test_drain_cli_returns_temporary_failure_when_fifo_remains_blocked(
    tmp_path: object, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(
        agent_cli,
        "drain",
        lambda *_args, **_kwargs: QueueResult("", (), (), ("pending",), "agent still working"),
    )
    assert agent_cli.main(["drain", "--pane", "w1:p1", "--queue", str(tmp_path)]) == 75


@pytest.mark.parametrize(
    "raw",
    [b"{not json\n", b'{"id":"bad","text":7}\n', b'{"id":[],"text":"bad"}\n'],
)
def test_invalid_fifo_head_preserves_raw_and_does_not_block_valid(
    tmp_path: object, raw: bytes
) -> None:
    inbox = tmp_path / "inbox"  # type: ignore[operator]
    inbox.mkdir()
    bad = inbox / "000000000001.json"
    bad.write_bytes(raw)
    good = inbox / "000000000002.json"
    good.write_text(json.dumps({"id": "good", "text": "deliver me"}) + "\n")
    fake = FakeAgentHerdr(["idle"])
    result = drain(client(fake), target(), str(tmp_path))
    failed = tmp_path / "failed" / bad.name  # type: ignore[operator]
    assert failed.read_bytes() == raw
    metadata = json.loads((tmp_path / "failed" / f"{bad.name}.error").read_text())  # type: ignore[operator]
    assert metadata["outcome"] == "invalid_message"
    assert result.quarantined == ("000000000001",)
    assert result.delivered == ("good",)
    assert fake.runs == ["deliver me"]


def test_crash_after_run_before_wait_is_never_resubmitted_on_restart(tmp_path: object) -> None:
    fake = FakeAgentHerdr(["idle"])

    def crash_after_run(_pane: str, _state: str, _timeout: int) -> None:
        raise KeyboardInterrupt("simulated process death")

    fake.wait_agent_status = crash_after_run  # type: ignore[assignment]
    with pytest.raises(KeyboardInterrupt):
        send(client(fake), target(), str(tmp_path), "at most once")
    assert fake.runs == ["at most once"]
    assert len(list((tmp_path / "inflight").glob("*.json"))) == 1  # type: ignore[operator]

    restarted = FakeAgentHerdr(["idle"])
    result = drain(client(restarted), target(), str(tmp_path))
    assert restarted.runs == []
    assert len(list((tmp_path / "inflight").glob("*.json"))) == 0  # type: ignore[operator]
    assert len(list((tmp_path / "failed").glob("*.json"))) == 1  # type: ignore[operator]
    assert result.outcome == "possibly_submitted"


def test_crash_after_inflight_rename_before_metadata_or_run_is_never_resubmitted(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    fake = FakeAgentHerdr(["idle"])
    original = agent_api._atomic_json

    def crash_on_inflight(path: str, document: dict[str, object]) -> None:
        if Path(path).parent.name == "inflight":
            raise KeyboardInterrupt("simulated death immediately after durable rename")
        original(path, document)

    monkeypatch.setattr(agent_api, "_atomic_json", crash_on_inflight)
    with pytest.raises(KeyboardInterrupt):
        send(client(fake), target(), str(tmp_path), "rename barrier")
    assert fake.runs == []
    assert list((tmp_path / "inbox").glob("*.json")) == []
    assert len(list((tmp_path / "inflight").glob("*.json"))) == 1

    monkeypatch.setattr(agent_api, "_atomic_json", original)
    restarted = FakeAgentHerdr(["idle"])
    result = drain(client(restarted), target(), str(tmp_path))
    assert restarted.runs == []
    assert result.outcome == "possibly_submitted"
    assert len(list((tmp_path / "failed").glob("*.json"))) == 1


def test_crash_during_busy_wait_stays_pending_and_delivers_after_restart(tmp_path: Path) -> None:
    busy = FakeAgentHerdr(["working"])

    def crash_while_waiting(_seconds: float) -> None:
        raise KeyboardInterrupt("simulated process death before injection")

    with pytest.raises(KeyboardInterrupt):
        send(
            client(busy),
            target(),
            str(tmp_path),
            "still safe",
            ready_timeout=900,
            sleep=crash_while_waiting,
        )
    assert busy.runs == []
    assert len(list((tmp_path / "inbox").glob("*.json"))) == 1
    assert list((tmp_path / "inflight").glob("*.json")) == []
    assert list((tmp_path / "failed").glob("*.json")) == []

    restarted = FakeAgentHerdr(["idle"])
    result = drain(client(restarted), target(), str(tmp_path))
    assert restarted.runs == ["still safe"]
    assert result.outcome == "delivered"
    assert result.pending == ()


def test_send_failures_have_distinct_typed_machine_outcomes(tmp_path: Path) -> None:
    busy = FakeAgentHerdr(["working"])
    with pytest.raises(AgentPending) as pending:
        send(client(busy), target(), str(tmp_path / "pending"), "safe", ready_timeout=0)
    assert pending.value.outcome == "pending"
    assert os.path.isfile(pending.value.artifact)

    ambiguous = FakeAgentHerdr(["idle"])
    ambiguous.wait_agent_status = lambda *_args: (_ for _ in ()).throw(HerdrUnavailable("lost"))  # type: ignore[method-assign]
    with pytest.raises(AgentPossiblySubmitted) as submitted:
        send(client(ambiguous), target(), str(tmp_path / "ambiguous"), "unsafe")
    assert submitted.value.outcome == "possibly_submitted"
    assert os.path.isfile(submitted.value.artifact)


def test_send_cli_emits_structured_distinct_failure(monkeypatch: pytest.MonkeyPatch, tmp_path: Path, capsys: pytest.CaptureFixture[str]) -> None:
    artifact = str(tmp_path / "inbox" / "m.json")
    monkeypatch.setattr(
        agent_cli,
        "send",
        lambda *_a, **_k: (_ for _ in ()).throw(
            AgentPending("busy", message_id="m", artifact=artifact)
        ),
    )
    assert agent_cli.main(["send", "--pane", "w1:p1", "hello"]) == 75
    assert json.loads(capsys.readouterr().out) == {
        "artifact": artifact,
        "error": "busy",
        "message_id": "m",
        "outcome": "pending",
        "safe_to_retry": True,
    }

    failed_artifact = str(tmp_path / "failed" / "m2.json")
    monkeypatch.setattr(
        agent_cli,
        "send",
        lambda *_a, **_k: (_ for _ in ()).throw(
            AgentPossiblySubmitted("unknown", message_id="m2", artifact=failed_artifact)
        ),
    )
    assert agent_cli.main(["send", "--pane", "w1:p1", "hello"]) == 76
    assert json.loads(capsys.readouterr().out)["safe_to_retry"] is False


def test_drain_cli_distinguishes_quarantine_from_pending(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(
        agent_cli,
        "drain",
        lambda *_args, **_kwargs: QueueResult(
            "", (), ("ambiguous",), (), None, "possibly_submitted"
        ),
    )
    assert agent_cli.main(["drain", "--pane", "w1:p1", "--queue", str(tmp_path)]) == 76


def test_enqueue_and_delivery_state_transitions_fsync(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    calls: list[int] = []
    real_fsync = os.fsync

    def recording_fsync(descriptor: int) -> None:
        calls.append(descriptor)
        real_fsync(descriptor)

    monkeypatch.setattr("herdr_run.agent.os.fsync", recording_fsync)
    fake = FakeAgentHerdr(["idle"])
    send(client(fake), target(), str(tmp_path), "durable")
    # Enqueue file+directory, inbox->inflight, inflight update, and
    # inflight->processed each require syncs; keep the assertion structural.
    assert len(calls) >= 8
