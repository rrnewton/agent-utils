"""Real command boundaries and fake HTTP cover the public Chat transport contract."""

from __future__ import annotations

import hashlib
import io
import json
import os
import subprocess
import sys
import time
from email.message import Message
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import parse_qs, urlsplit
from urllib.request import Request

import pytest

import agentctl.chat as chat_module
from agentctl.agent import Target
from agentctl.chat import (
    Bridge, CommandTransport, Config, GoogleChatTransport,
    _read, _run_command, _write, submit_reply,
)
from agentctl.client import AgentPaneInfo, HerdrClient, Pane
from agentctl.errors import AgentDeliveryError, HerdrUnavailable

_SPACE = "spaces/test"
_THREAD = "spaces/test/threads/thread-one"
_REQUEST_ID = "f17fb68a-5597-49a9-a1ab-d14b26331b0e"
_START = "2026-01-01T00:00:00Z"


class Http:
    def __init__(self, *responses: dict[str, object]) -> None:
        self.responses = list(responses)
        self.requests: list[Request] = []

    def __call__(self, request: Request, *, timeout: float) -> io.BytesIO:
        assert timeout == 45
        self.requests.append(request)
        return io.BytesIO(json.dumps(self.responses.pop(0)).encode())


def _poll(cursor: str | None = None) -> dict[str, object]:
    return {"action": "poll", "space": _SPACE, "after": _START, "cursor": cursor}


def _send() -> dict[str, object]:
    return {"action": "send", "space": _SPACE, "thread": _THREAD,
            "request_id": _REQUEST_ID, "text": "[fixture-agent] Done: π\nSecond line."}


def test_rest_poll_encodes_filter_and_opaque_page_token(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    http = Http({"messages": [{
        "name": _SPACE + "/messages/message-one", "text": "Please inspect",
        "sender": {"name": "users/owner", "type": "HUMAN"},
        "thread": {"name": _THREAD}, "createTime": "2026-01-01T00:01:00.123456Z",
    }], "nextPageToken": "next+/= cursor"})
    monkeypatch.setattr(chat_module, "urlopen", http)
    monkeypatch.setenv("TEST_CHAT_ACCESS_TOKEN", "fixture-access-token")
    cursor = 'opaque+/= token & filter="different"'
    result = GoogleChatTransport("TEST_CHAT_ACCESS_TOKEN")(_poll(cursor))
    request = http.requests[0]
    url = urlsplit(request.full_url)
    assert (url.scheme, url.netloc, url.path) == (
        "https", "chat.googleapis.com", "/v1/spaces/test/messages",
    )
    assert parse_qs(url.query) == {
        "pageSize": ["100"], "orderBy": ["createTime asc"],
        "filter": [f'createTime > "{_START}"'], "pageToken": [cursor],
    }
    assert request.get_method() == "GET"
    assert request.data is None
    assert request.get_header("Authorization") == "Bearer fixture-access-token"
    assert result == {"messages": [{
        "id": _SPACE + "/messages/message-one", "text": "Please inspect",
        "sender": "users/owner", "thread": _THREAD,
        "created_at": "2026-01-01T00:01:00.123456Z",
    }], "cursor": "next+/= cursor"}


def test_rest_send_preserves_text_thread_and_retry_identity(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    response: dict[str, object] = {"name": _SPACE + "/messages/reply"}
    http = Http(response, response)
    monkeypatch.setattr(chat_module, "urlopen", http)
    monkeypatch.setenv("TEST_CHAT_ACCESS_TOKEN", "fixture-access-token")
    transport = GoogleChatTransport("TEST_CHAT_ACCESS_TOKEN")
    assert transport(_send()) == transport(_send()) == {"id": response["name"]}
    first, second = http.requests
    assert first.full_url == second.full_url
    assert first.data == second.data
    assert first.get_method() == "POST"
    assert parse_qs(urlsplit(first.full_url).query) == {
        "requestId": [_REQUEST_ID], "messageReplyOption": ["REPLY_MESSAGE_OR_FAIL"],
    }
    assert isinstance(first.data, bytes)
    assert json.loads(first.data) == {
        "text": _send()["text"], "thread": {"name": _THREAD},
    }
    assert first.get_header("Content-type") == "application/json"


@pytest.mark.parametrize("token", ["secret-token\nhelper diagnostic", "secret-token\r", "secret-token π"])
def test_malformed_token_is_not_echoed_in_diagnostics(
    token: str, monkeypatch: pytest.MonkeyPatch,
) -> None:
    http = Http()
    monkeypatch.setattr(chat_module, "urlopen", http)
    monkeypatch.setenv("TEST_CHAT_ACCESS_TOKEN", token)
    with pytest.raises(ValueError, match="ASCII without whitespace") as error:
        GoogleChatTransport("TEST_CHAT_ACCESS_TOKEN")(_poll())
    assert "secret-token" not in str(error.value)
    assert not http.requests


def test_misspelled_authority_option_is_refused() -> None:
    with pytest.raises(ValueError, match="unknown chat configuration"):
        Config.parse({"agent_naem": "coordinator"})


def test_token_command_refreshes_on_every_page_and_send(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    counter = tmp_path / "token-counter"
    script = (
        "from pathlib import Path; import sys; p=Path(sys.argv[1]); "
        "n=int(p.read_text())+1 if p.exists() else 1; "
        "p.write_text(str(n)); print('  fixture-token-'+str(n)+'  ')"
    )
    command = (sys.executable, "-c", script, str(counter))
    http = Http({"messages": [], "nextPageToken": "next"},
                {"messages": []}, {"name": _SPACE + "/messages/reply"})
    monkeypatch.setattr(chat_module, "urlopen", http)
    monkeypatch.setenv("TEST_CHAT_ACCESS_TOKEN", "stale-environment-token")
    transport = GoogleChatTransport("TEST_CHAT_ACCESS_TOKEN", command)
    transport(_poll())
    transport(_poll("next"))
    transport(_send())
    assert counter.read_text() == "3"
    assert [request.get_header("Authorization") for request in http.requests] == [
        "Bearer fixture-token-1", "Bearer fixture-token-2", "Bearer fixture-token-3",
    ]


@pytest.mark.parametrize("mode", ["failure", "empty"])
def test_failed_token_refresh_never_uses_stale_environment_credentials(
    monkeypatch: pytest.MonkeyPatch, mode: str,
) -> None:
    http = Http()
    monkeypatch.setattr(chat_module, "urlopen", http)
    monkeypatch.setenv("TEST_CHAT_ACCESS_TOKEN", "stale-environment-token")
    script = "import sys; print('credential-detail', file=sys.stderr); sys.exit(7)" if mode == "failure" else "print('')"
    transport = GoogleChatTransport("TEST_CHAT_ACCESS_TOKEN", (sys.executable, "-c", script))
    with pytest.raises(ValueError) as caught:
        transport(_poll())
    assert "credential-detail" not in str(caught.value)
    assert http.requests == []


def test_http_error_is_propagated_for_durable_retry(monkeypatch: pytest.MonkeyPatch) -> None:
    def fail(request: Request, *, timeout: float) -> io.BytesIO:
        raise HTTPError(request.full_url, 429, "rate limited", Message(), None)

    monkeypatch.setattr(chat_module, "urlopen", fail)
    monkeypatch.setenv("TEST_CHAT_ACCESS_TOKEN", "fixture-access-token")
    with pytest.raises(HTTPError) as caught:
        GoogleChatTransport("TEST_CHAT_ACCESS_TOKEN")(_send())
    assert caught.value.code == 429
    assert _REQUEST_ID in caught.value.url


def test_rest_rejects_invalid_boundaries_before_http(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    http = Http()
    monkeypatch.setattr(chat_module, "urlopen", http)
    monkeypatch.setenv("TEST_CHAT_ACCESS_TOKEN", "fixture-access-token")
    transport = GoogleChatTransport("TEST_CHAT_ACCESS_TOKEN")
    for key, value in (("space", "spaces/test/../other"),
                       ("thread", "spaces/other/threads/thread-one"),
                       ("request_id", "not-a-uuid")):
        request = _send()
        request[key] = value
        with pytest.raises(ValueError):
            transport(request)
    request = _poll()
    request["cursor"] = 37
    with pytest.raises(ValueError, match="cursor"):
        transport(request)
    assert http.requests == []


def test_rest_rejects_reply_resource_from_another_space(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    http = Http({"name": "spaces/other/messages/reply"})
    monkeypatch.setattr(chat_module, "urlopen", http)
    monkeypatch.setenv("TEST_CHAT_ACCESS_TOKEN", "fixture-access-token")
    with pytest.raises(ValueError, match="outside"):
        GoogleChatTransport("TEST_CHAT_ACCESS_TOKEN")(_send())


def test_command_adapter_preserves_literal_arguments_and_request(tmp_path: Path) -> None:
    marker = tmp_path / "must-not-exist"
    literal = f"$(touch {marker}); `echo substituted`\nspace and π"
    script = "import json,sys; print(json.dumps({'args':sys.argv[1:], 'request':json.load(sys.stdin)}))"
    request: dict[str, object] = {"action": "send", "text": literal}
    response = CommandTransport((sys.executable, "-c", script, literal))(request)
    assert response == {"args": [literal], "request": request}
    assert not marker.exists()


@pytest.mark.parametrize(
    ("script", "error", "message"),
    [
        ("import sys; print('adapter detail',file=sys.stderr); sys.exit(7)", ValueError, "exited 7"),
        ("print('not-json')", ValueError, "Expecting value"),
        ("print('[]')", TypeError, "expected an object"),
    ],
)
def test_command_adapter_failures_are_not_empty_successes(
    script: str, error: type[Exception], message: str,
) -> None:
    with pytest.raises(error, match=message):
        CommandTransport((sys.executable, "-c", script))(_poll())


def test_command_timeout_terminates_descendants_holding_output_pipes(tmp_path: Path) -> None:
    pid_path = tmp_path / "child.pid"
    script = (
        "import pathlib,subprocess,sys,time; "
        "child=subprocess.Popen([sys.executable,'-c','import time; time.sleep(60)']); "
        "pathlib.Path(sys.argv[1]).write_text(str(child.pid)); time.sleep(60)"
    )
    started = time.monotonic()
    with pytest.raises(subprocess.TimeoutExpired):
        _run_command((sys.executable, "-c", script, str(pid_path)), timeout=0.5)
    assert time.monotonic() - started < 5
    pid = int(pid_path.read_text())
    deadline = time.monotonic() + 2
    while time.monotonic() < deadline:
        try:
            os.kill(pid, 0)
            state = Path(f"/proc/{pid}/stat").read_text().rpartition(") ")[2][:1]
        except (ProcessLookupError, FileNotFoundError):
            return
        if state == "Z":
            return
        time.sleep(0.01)
    pytest.fail(f"timed-out adapter child {pid} remains running")


class Pages:
    def __init__(self, *pages: dict[str, object]) -> None:
        self.pages = list(pages)
        self.calls: list[dict[str, object]] = []

    def __call__(self, request: dict[str, object]) -> dict[str, object]:
        self.calls.append(request)
        return self.pages.pop(0)


def _message(identifier: str, created_at: str = "2026-01-01T00:02:00Z") -> dict[str, object]:
    return {"id": _SPACE + "/messages/" + identifier, "text": identifier,
            "sender": "users/owner", "thread": _THREAD, "created_at": created_at}


def _bridge(state: Path, transport: Pages, *, target: Target | None = None) -> Bridge:
    if not (state / "bridge.json").exists():
        target = target or Target(pane_id="w1:p1", expected_agent="codex",
                                  expected_cwd="/work/project", expected_workspace="project")
        Bridge.initialize(state, Config(_SPACE, ("users/owner",), target, "fixture-agent", ack_reaction=None), after=_START)
    return Bridge(state, transport=transport)


def test_pagination_restarts_same_query_then_overlaps_without_duplicate_inputs(tmp_path: Path) -> None:
    pages = Pages(
        {"messages": [_message("one")], "cursor": "page-two"},
        {"messages": [_message("one"), _message("two")], "cursor": None},
        {"messages": [_message("one"), _message("two"), _message("three")], "cursor": None},
    )
    _bridge(tmp_path, pages)._ingest(_read(tmp_path / "bridge.json"))
    assert _read(tmp_path / "bridge.json")["after"] == _START
    restarted = _bridge(tmp_path, pages)
    restarted._ingest(_read(tmp_path / "bridge.json"))
    assert pages.calls[1] == {"action": "poll", "space": _SPACE, "after": _START, "cursor": "page-two"}
    assert _read(tmp_path / "bridge.json")["after"] == "2026-01-01T00:01:00Z"
    restarted._ingest(_read(tmp_path / "bridge.json"))
    assert len(list((tmp_path / "requests").glob("*.json"))) == 3


def test_failed_page_replays_persisted_messages_without_advancing_cursor(tmp_path: Path) -> None:
    pages = Pages({"messages": [_message("one")], "cursor": 42},
                  {"messages": [_message("one"), _message("two")], "cursor": None})
    bridge = _bridge(tmp_path, pages)
    before = _read(tmp_path / "bridge.json")
    with pytest.raises(ValueError, match="cursor"):
        bridge._ingest(_read(tmp_path / "bridge.json"))
    assert _read(tmp_path / "bridge.json") == before
    _bridge(tmp_path, pages)._ingest(_read(tmp_path / "bridge.json"))
    assert pages.calls[0] == pages.calls[1]
    assert len(list((tmp_path / "requests").glob("*.json"))) == 2


def test_wrong_reply_resource_remains_pending_and_keeps_retry_id(tmp_path: Path) -> None:
    pages = Pages({"id": "spaces/other/messages/reply"}, {"id": _SPACE + "/messages/reply"})
    bridge = _bridge(tmp_path, pages)
    message = _message("one")
    key = hashlib.sha256(str(message["id"]).encode()).hexdigest()
    path = tmp_path / "requests" / f"{key}.json"
    _write(path, {"key": key, "queue_id": "source-one", "phase": "awaiting_reply",
                  "message": message, "request_id": _REQUEST_ID, "received_at": _START})
    submit_reply(tmp_path, key, "Done")
    with pytest.raises(ValueError, match="outside"):
        bridge._deliver()
    assert _read(path)["phase"] == "reply_pending"
    _bridge(tmp_path, pages)._deliver()
    assert _read(path)["phase"] == "replied"
    assert pages.calls[0]["request_id"] == pages.calls[1]["request_id"] == _REQUEST_ID


def test_restart_rejects_replacement_session_before_any_chat_access(tmp_path: Path) -> None:
    class ReplacementHarness(HerdrClient):
        def panes(self, workspace_id: str | None = None) -> tuple[Pane, ...]:
            return (Pane("w1:p1", "w1:t1", "w1"),)

        def pane_info(self, pane_id: str) -> AgentPaneInfo:
            return AgentPaneInfo(pane_id, "w1", "/work/project", "codex", "idle", "codex", "replacement")

    pages = Pages()
    target = Target(pane_id="w1:p1", session_agent="codex", session_value="original",
                    expected_agent="codex", expected_cwd="/work/project", expected_workspace="project")
    _bridge(tmp_path, pages, target=target)
    restarted = Bridge(tmp_path, ReplacementHarness(), pages)
    with pytest.raises(AgentDeliveryError, match="exactly one live pane"):
        restarted.tick()
    assert pages.calls == []


class NamedHarness(HerdrClient):
    def __init__(self) -> None:
        self.named_pane = "w1:p1"
        self.probes = 0
        self.replace_on_probe: int | None = None
        self.prompts: list[str] = []

    def agent_pane(self, name: str) -> str:
        assert name == "coordinator"
        return self.named_pane

    def pane_info(self, pane_id: str) -> AgentPaneInfo:
        self.probes += 1
        if self.probes == self.replace_on_probe:
            self.named_pane = "w1:p2"
        return AgentPaneInfo(pane_id, "w1", "/work/project", "codex", "idle", None, None)

    def workspace_label(self, workspace_id: str) -> str:
        return "project"

    def prompt_agent(self, pane_id: str, command: str) -> None:
        self.prompts.append(command)

    def wait_agent_status(self, pane_id: str, status: str, timeout_ms: int) -> None:
        assert status == "working"


def _named_bridge(state: Path, client: NamedHarness, transport: Pages) -> Bridge:
    target = Target(pane_id="w1:p1", expected_agent="codex",
                    expected_cwd="/work/project", expected_workspace="project")
    Bridge.initialize(state, Config(_SPACE, ("users/owner",), target, "fixture-agent",
                                    agent_name="coordinator", ack_reaction=None), after=_START)
    return Bridge(state, client, transport)


def test_named_binding_survives_restart_and_blocks_changed_owner_before_chat(tmp_path: Path) -> None:
    client, pages = NamedHarness(), Pages()
    _named_bridge(tmp_path, client, pages)
    client.named_pane = "w1:p2"
    restarted = Bridge(tmp_path, client, pages)
    assert restarted.config.agent_name == "coordinator"
    with pytest.raises(HerdrUnavailable, match="no longer owns"):
        restarted.tick()
    assert client.prompts == []
    assert pages.calls == []


def test_named_binding_checked_again_after_poll_before_delivery(tmp_path: Path) -> None:
    client = NamedHarness()

    class ChangedDuringPoll(Pages):
        def __call__(self, request: dict[str, object]) -> dict[str, object]:
            client.named_pane = "w1:p2"
            return super().__call__(request)

    pages = ChangedDuringPoll({"messages": [_message("one")], "cursor": None})
    bridge = _named_bridge(tmp_path, client, pages)
    bridge.tick()
    record = _read(next((tmp_path / "requests").glob("*.json")))
    assert record["phase"] == "queued"
    # The queue preserves known-unsubmitted work; the next tick rejects its
    # stale named target before even polling again.
    with pytest.raises(HerdrUnavailable, match="no longer owns"):
        bridge.tick()
    assert client.prompts == []
    assert len(pages.calls) == 1


def test_named_binding_checked_immediately_before_terminal_submission(tmp_path: Path) -> None:
    client = NamedHarness()
    # Tick, queue-lock resolution, then confirmation under that lock. The last
    # readiness snapshot still describes the old occupant; run must recheck.
    client.replace_on_probe = 3
    pages = Pages({"messages": [_message("one")], "cursor": None})
    bridge = _named_bridge(tmp_path, client, pages)
    bridge.tick()
    assert client.prompts == []
    record = _read(next((tmp_path / "requests").glob("*.json")))
    assert record["phase"] == "delivery_uncertain"


def test_named_coordinator_delivers_when_binding_is_unchanged(tmp_path: Path) -> None:
    client = NamedHarness()
    pages = Pages({"messages": [_message("one")], "cursor": None})
    bridge = _named_bridge(tmp_path, client, pages)
    bridge.tick()
    assert len(client.prompts) == 1
    assert client.probes >= 5


def test_run_loop_retries_malformed_transport_output(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str],
) -> None:
    calls = 0
    now = 0.0

    class RetryBridge:
        def __init__(self, state: Path) -> None:
            self.state = state
            self.config = Config(_SPACE, ("users/owner",), Target(), "test-agent", reply_mode="file")

        def tick(self) -> dict[str, object]:
            nonlocal calls
            calls += 1
            if calls == 1:
                raise TypeError("malformed adapter message")
            raise KeyboardInterrupt

        def output_requests(self, *, retry_failed: bool = False) -> dict[str, str]:
            return {}

    def skip_sleep(seconds: float) -> None:
        nonlocal now
        now += seconds

    monkeypatch.setattr(chat_module, "Bridge", RetryBridge)
    monkeypatch.setattr(time, "sleep", skip_sleep)
    monkeypatch.setattr(time, "monotonic", lambda: now)
    assert chat_module.run_cli(["run", "--state", str(tmp_path)]) == 130
    assert calls == 2
    assert "malformed adapter message" in capsys.readouterr().err


@pytest.mark.parametrize(("interval_arguments", "failures", "expected_delays"), [
    (("--interval", "10"), 4, [20, 40, 60, 60, 10]),
    ((), 2, [7200, 14400, 3600]),
])
def test_run_loop_backs_off_failures_and_recovers_configured_interval(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
    interval_arguments: tuple[str, ...], failures: int, expected_delays: list[float],
) -> None:
    calls = 0
    now = 0.0
    delays: list[float] = []

    class RecoveringBridge:
        def __init__(self, state: Path) -> None:
            self.state = state
            self.config = Config(_SPACE, ("users/owner",), Target(), "test-agent", reply_mode="file")

        def tick(self) -> dict[str, object]:
            nonlocal calls
            calls += 1
            if calls <= failures:
                raise ValueError("temporary quota failure")
            if calls == failures + 1:
                return {}
            raise KeyboardInterrupt

        def output_requests(self, *, retry_failed: bool = False) -> dict[str, str]:
            return {}

    def sleep(seconds: float) -> None:
        nonlocal now
        delays.append(seconds)
        now += seconds

    monkeypatch.setattr(chat_module, "Bridge", RecoveringBridge)
    monkeypatch.setattr(time, "sleep", sleep)
    monkeypatch.setattr(time, "monotonic", lambda: now)
    assert chat_module.run_cli([
        "run", "--state", str(tmp_path), *interval_arguments,
    ]) == 130
    assert delays == expected_delays


def test_run_loop_long_interval_failures_back_off_toward_one_day(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    calls = 0
    now = 0.0
    delays: list[float] = []

    class RecoveringBridge:
        def __init__(self, state: Path) -> None:
            self.state = state
            self.config = Config(_SPACE, ("users/owner",), Target(), "test-agent", reply_mode="file")

        def tick(self) -> dict[str, object]:
            nonlocal calls
            calls += 1
            if calls <= 2:
                raise ValueError("temporary quota failure")
            if calls == 3:
                return {}
            raise KeyboardInterrupt

        def output_requests(self, *, retry_failed: bool = False) -> dict[str, str]:
            return {}

    def sleep(seconds: float) -> None:
        nonlocal now
        delays.append(seconds)
        now += seconds

    monkeypatch.setattr(chat_module, "Bridge", RecoveringBridge)
    monkeypatch.setattr(time, "sleep", sleep)
    monkeypatch.setattr(time, "monotonic", lambda: now)
    assert chat_module.run_cli([
        "run", "--state", str(tmp_path), "--interval", "3600",
    ]) == 130
    assert delays == [7200, 14400, 3600]


def test_run_accepts_one_day_poll_interval_and_rejects_larger(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str],
) -> None:
    intervals: list[float] = []
    monkeypatch.setattr(chat_module, "Bridge", lambda state: object())
    monkeypatch.setattr(chat_module, "_run_bridge",
                        lambda bridge, interval, prog, **kwargs: intervals.append(interval))

    assert chat_module.run_cli([
        "run", "--state", str(tmp_path), "--interval", "86400",
    ]) == 0
    assert intervals == [86400]
    assert chat_module.run_cli([
        "run", "--state", str(tmp_path), "--interval", "86400.1",
    ]) == 1
    assert "between 0.1 and 86400 seconds" in capsys.readouterr().err
