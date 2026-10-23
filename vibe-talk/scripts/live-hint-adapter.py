#!/usr/bin/env python3
"""Turn a provider event stream into vibe-talk live input: content-free hints, or the records.

Two modes, chosen on the command line:

**Hints** (the default). The upstream command is a trust boundary: it must emit only message
events for the one channel named by ``--channel`` and suppress heartbeats.  This adapter
deliberately does not interpret an event or forward any of its fields.  Every nonblank JSON-object
line causes exactly one ``POST /api/v1/live/hints`` whose entire body is ``{"channel_id": "..."}``.
vibe-talk then reads the allowlisted channel through its configured, read-only provider and
cursor-deduplicates the result.

**Records** (``--records``, ``#452 slack-live-ingest``). The stream already carries each message's
text, so reading the channel again only adds a provider round trip. Each record is posted as it
stands to ``POST /api/v1/live/tail``, where vibe-talk checks every field, normalizes the message as a
read would, and publishes it at once. The adapter reads only two fields of a record: ``type``, to
drop heartbeats locally, and ``iris_seq_id``, the stream position it keeps in ``--state-file``
after vibe-talk acknowledges a record, so a restart resumes where it stopped (``{catchup}`` in the
command). It tails every conversation vibe-talk names at ``GET /api/v1/live/tail`` unless
``--channel`` fixes the list (``{channels}`` in the command), asks again every
``--channels-refresh`` seconds, and restarts the stream when the answer changes. A stream that
cannot resume from the saved position (its retention overflowed) is reported to vibe-talk as a gap,
which reads the channels again, and the position is forgotten.

The command after the literal ``--`` is executed directly, never by a shell.  Its stderr is never
logged (in records mode it is read only to notice a retention overflow) and its stdout is accepted
one bounded line at a time so event content and credentials cannot enter this adapter's logs.
"""

from __future__ import annotations

import argparse
import contextlib
import http.client
import io
import json
import os
from pathlib import Path
import re
import signal
import ssl
import stat
import subprocess
import sys
import tempfile
import threading
import time
from collections import deque
from collections.abc import Callable, Iterator, Sequence
from dataclasses import dataclass
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from types import FrameType
from typing import NoReturn, Protocol, TypeAlias, cast
from urllib.parse import SplitResult, urlsplit


EXIT_OK = 0
EXIT_USAGE = 2
EXIT_UPSTREAM_PROTOCOL = 10
EXIT_UPSTREAM_FAILED = 11
EXIT_DELIVERY_FAILED = 12
EXIT_CONTROL_FAILED = 17

MAX_EVENT_LINE_BYTES = 64 * 1024
MAX_TOKEN_FILE_BYTES = 4096
MIN_TOKEN_BYTES = 24
REQUEST_TIMEOUT_SECONDS = 10.0
CHILD_EXIT_GRACE_SECONDS = 2.0
CHILD_TERM_GRACE_SECONDS = 2.0
RETRY_DELAYS_SECONDS = (0.25, 0.5, 1.0, 2.0)
HINT_PATH = "/api/v1/live/hints"
TAIL_PATH = "/api/v1/live/tail"
# vibe-talk's own bound on one record: Slack's 40,000-character ceiling in four-byte characters,
# and the record around it.
MAX_RECORD_LINE_BYTES = 4 * 40_000 + 8 * 1024
MAX_CHANNELS_RESPONSE_BYTES = 64 * 1024
MAX_STATE_FILE_BYTES = 4096
CHANNELS_PLACEHOLDER = "{channels}"
CATCHUP_PLACEHOLDER = "{catchup}"
CONTROL_TYPES = frozenset({"HEARTBEAT", "STALL", "REVIVAL"})
DEFAULT_CHANNELS_REFRESH_SECONDS = 300.0
MIN_CHANNELS_REFRESH_SECONDS = 10.0
CONVERSATION_ID = re.compile(r"[CGD][A-Z0-9]{8,31}")
# What the stream client writes to stderr when it cannot resume from a saved position.
OVERFLOW_MARKERS = (b"queue overflow", b"retention overflow", b"no longer queued")

Log = Callable[[str], None]
StopCheck = Callable[[], bool]
Sleep = Callable[[float], None]
Transport = Callable[[str, str, bytes, float], int]


class AdapterFailure(Exception):
    """An expected operator-facing failure with a stable exit code and contentless message."""

    def __init__(self, code: int, message: str) -> None:
        super().__init__(message)
        self.code = code
        self.message = message


class StopRequested(Exception):
    """Internal control flow after SIGINT or SIGTERM."""


class RetryableDelivery(Exception):
    """One delivery failed in a way for which retry is safe."""

    def __init__(self, status: int | None) -> None:
        super().__init__()
        self.status = status


class HintSender(Protocol):
    """The narrow interface the stream loop needs from the HTTP client."""

    def post(self, stopping: StopCheck) -> int:
        """Post one contentless hint and return its successful HTTP status."""


def stderr_log(message: str) -> None:
    """Emit one already-sanitized operator message."""

    print(f"live-hint-adapter: {message}", file=sys.stderr, flush=True)


def parse_base_url(raw: str) -> str:
    """Validate an origin-only HTTP(S) base URL and return its hint endpoint."""

    return f"{parse_origin(raw)}{HINT_PATH}"


def parse_origin(raw: str) -> str:
    """Validate an origin-only HTTP(S) base URL and return it without a trailing slash."""

    try:
        parts = urlsplit(raw)
        # Accessing port performs urllib's bracket/range validation.
        _ = parts.port
    except ValueError as exc:
        raise AdapterFailure(EXIT_USAGE, "--url is not a valid HTTP(S) origin") from exc
    if parts.scheme not in {"http", "https"} or not parts.hostname:
        raise AdapterFailure(EXIT_USAGE, "--url must be an HTTP(S) origin")
    if parts.username is not None or parts.password is not None:
        raise AdapterFailure(EXIT_USAGE, "--url must not contain credentials")
    if parts.path not in {"", "/"} or parts.query or parts.fragment:
        raise AdapterFailure(EXIT_USAGE, "--url must be an origin without a path, query, or fragment")
    if parts.scheme == "http" and parts.hostname not in {"127.0.0.1", "::1", "localhost"}:
        raise AdapterFailure(EXIT_USAGE, "plain HTTP is allowed only for a loopback --url")
    return f"{parts.scheme}://{parts.netloc}"


def validate_channel(raw: str) -> str:
    """Validate the fixed opaque channel id without imposing provider-specific syntax."""

    encoded = raw.encode("utf-8")
    if not raw or raw != raw.strip() or len(encoded) > 512:
        raise AdapterFailure(EXIT_USAGE, "--channel must be 1 to 512 UTF-8 bytes with no edge whitespace")
    if any(ord(character) < 0x20 or ord(character) == 0x7F for character in raw):
        raise AdapterFailure(EXIT_USAGE, "--channel must not contain control characters")
    return raw


def read_private_token(path: Path) -> str:
    """Read a bounded regular token file without following a final symlink."""

    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(path, flags)
    except OSError as exc:
        raise AdapterFailure(EXIT_USAGE, "cannot open --token-file as a private regular file") from exc
    try:
        metadata = os.fstat(descriptor)
        if not stat.S_ISREG(metadata.st_mode):
            raise AdapterFailure(EXIT_USAGE, "--token-file must be a regular file")
        if metadata.st_uid != os.geteuid():
            raise AdapterFailure(EXIT_USAGE, "--token-file must be owned by the current effective user")
        if metadata.st_nlink != 1:
            raise AdapterFailure(EXIT_USAGE, "--token-file must have exactly one hard link")
        if metadata.st_mode & 0o077:
            raise AdapterFailure(EXIT_USAGE, "--token-file must not be accessible by group or other")
        with os.fdopen(descriptor, "rb", closefd=False) as source:
            raw = source.read(MAX_TOKEN_FILE_BYTES + 1)
    finally:
        os.close(descriptor)
    if len(raw) > MAX_TOKEN_FILE_BYTES:
        raise AdapterFailure(EXIT_USAGE, f"--token-file exceeds {MAX_TOKEN_FILE_BYTES} bytes")
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise AdapterFailure(EXIT_USAGE, "--token-file is not UTF-8") from exc
    if text.endswith("\n"):
        text = text[:-1]
        if text.endswith("\r"):
            text = text[:-1]
    token = text
    token_bytes = token.encode("utf-8")
    if len(token_bytes) < MIN_TOKEN_BYTES or any(
        ord(character) < 0x21 or ord(character) > 0x7E for character in token
    ):
        raise AdapterFailure(
            EXIT_USAGE,
            f"--token-file must contain one printable ASCII token of at least {MIN_TOKEN_BYTES} bytes",
        )
    return token


def open_connection(parts: SplitResult, timeout: float) -> http.client.HTTPConnection:
    """Create one request-scoped connection; http.client never follows redirects."""

    hostname = parts.hostname
    if hostname is None:  # Guard for callers other than parse_base_url.
        raise RetryableDelivery(None)
    port = parts.port
    if parts.scheme == "https":
        return http.client.HTTPSConnection(
            hostname,
            port=port,
            timeout=timeout,
            context=ssl.create_default_context(),
        )
    return http.client.HTTPConnection(hostname, port=port, timeout=timeout)


def post_once(endpoint: str, token: str, body: bytes, timeout: float) -> int:
    """Make one bounded contentless request and return its status without reading its body."""

    parts = urlsplit(endpoint)
    connection: http.client.HTTPConnection | None = None
    try:
        connection = open_connection(parts, timeout)
        connection.request(
            "POST",
            parts.path,
            body=body,
            headers={
                "Authorization": f"Bearer {token}",
                "Content-Type": "application/json",
                "Content-Length": str(len(body)),
                "Accept": "application/json",
            },
        )
        response = connection.getresponse()
        try:
            return response.status
        finally:
            response.close()
    except (OSError, http.client.HTTPException) as exc:
        raise RetryableDelivery(None) from exc
    finally:
        if connection is not None:
            connection.close()


@dataclass(frozen=True)
class RetryPolicy:
    """A finite retry schedule; attempts equal one plus the number of delays."""

    delays: tuple[float, ...] = RETRY_DELAYS_SECONDS


class HttpHintSender:
    """Authenticated client for the contentless hint endpoint."""

    def __init__(
        self,
        endpoint: str,
        channel: str,
        token: str,
        log: Log,
        *,
        timeout: float = REQUEST_TIMEOUT_SECONDS,
        policy: RetryPolicy = RetryPolicy(),
        sleep: Sleep = time.sleep,
        transport: Transport = post_once,
    ) -> None:
        self.endpoint = endpoint
        self.token = token
        self.log = log
        self.timeout = timeout
        self.policy = policy
        self.sleep = sleep
        self.transport = transport
        # This immutable body is the privacy property: upstream fields have nowhere to enter it.
        self.body = json.dumps(
            {"channel_id": channel}, separators=(",", ":"), ensure_ascii=False
        ).encode("utf-8")

    def post(self, stopping: StopCheck) -> int:
        """Post once, retrying only transport failures, 429, and 5xx."""

        attempts = len(self.policy.delays) + 1
        last_retryable_status: int | None = None
        for attempt in range(1, attempts + 1):
            if stopping():
                raise StopRequested
            status: int | None
            try:
                status = self.transport(
                    self.endpoint,
                    self.token,
                    self.body,
                    self.timeout,
                )
            except RetryableDelivery as exc:
                status = exc.status
            if status == 200 or status == 202:
                return status
            if status == 429 or (status is not None and 500 <= status <= 599):
                last_retryable_status = status
            elif status is None:
                last_retryable_status = None
            else:
                raise AdapterFailure(
                    EXIT_DELIVERY_FAILED,
                    f"hint endpoint permanently refused a request with HTTP {status}",
                )
            if attempt == attempts:
                break
            delay = self.policy.delays[attempt - 1]
            result = (
                "a transport failure"
                if last_retryable_status is None
                else f"retryable HTTP {last_retryable_status}"
            )
            self.log(f"hint delivery attempt {attempt}/{attempts} had {result}; retrying")
            self.sleep(delay)
        result = (
            "transport failure"
            if last_retryable_status is None
            else f"HTTP {last_retryable_status}"
        )
        raise AdapterFailure(
            EXIT_DELIVERY_FAILED,
            f"hint delivery failed after {attempts} attempts; last result was {result}",
        )


def reject_nonstandard_json_constant(value: str) -> NoReturn:
    """Reject NaN and infinities, which json.loads otherwise accepts as extensions."""

    raise ValueError(f"nonstandard constant {value}")


def require_json_object(raw: bytes, line_number: int) -> None:
    """Validate one line without retaining or returning its potentially sensitive fields."""

    try:
        decoded = raw.decode("utf-8")
        value: object = json.loads(decoded, parse_constant=reject_nonstandard_json_constant)
    except (UnicodeDecodeError, ValueError, RecursionError) as exc:
        raise AdapterFailure(
            EXIT_UPSTREAM_PROTOCOL,
            f"upstream line {line_number} is not a valid JSON object",
        ) from exc
    if not isinstance(value, dict):
        raise AdapterFailure(
            EXIT_UPSTREAM_PROTOCOL,
            f"upstream line {line_number} is valid JSON but not an object",
        )


class ShutdownController:
    """Signal state shared by the handler, stream loop, and retry loop."""

    def __init__(self) -> None:
        self.signal_number: int | None = None
        self.child: subprocess.Popen[bytes] | None = None

    def handler(self, signum: int, _frame: FrameType | None) -> None:
        """Record the first signal and unblock a stdout read by terminating the child group."""

        if self.signal_number is None:
            self.signal_number = signum
        child = self.child
        if child is not None:
            signal_process_group(child.pid, signal.SIGTERM)

    def attach(self, child: subprocess.Popen[bytes]) -> None:
        """Attach a child and honor a signal that arrived just before it started."""

        self.child = child
        if self.signal_number is not None:
            signal_process_group(child.pid, signal.SIGTERM)

    def detach(self) -> None:
        """Forget the child after it has been reaped."""

        self.child = None

    def stopping(self) -> bool:
        """Whether SIGINT or SIGTERM has requested shutdown."""

        return self.signal_number is not None


def signal_process_group(group: int, requested_signal: signal.Signals) -> None:
    """Signal an owned child process group, tolerating a group that already exited."""

    try:
        os.killpg(group, requested_signal)
    except OSError:
        pass


def process_group_exists(group: int) -> bool:
    """Whether any process still has the owned child's process-group id."""

    try:
        os.killpg(group, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def clean_child(child: subprocess.Popen[bytes]) -> None:
    """Terminate the entire owned group and reap its leader, escalating after a bound."""

    signal_process_group(child.pid, signal.SIGTERM)
    try:
        child.wait(timeout=CHILD_TERM_GRACE_SECONDS)
    except subprocess.TimeoutExpired:
        pass
    deadline = time.monotonic() + CHILD_TERM_GRACE_SECONDS
    while process_group_exists(child.pid) and time.monotonic() < deadline:
        time.sleep(0.02)
    if process_group_exists(child.pid):
        signal_process_group(child.pid, signal.SIGKILL)
    try:
        child.wait(timeout=CHILD_TERM_GRACE_SECONDS)
    except subprocess.TimeoutExpired as exc:
        raise AdapterFailure(EXIT_UPSTREAM_FAILED, "could not reap the upstream command") from exc


@contextlib.contextmanager
def installed_signal_handlers(controller: ShutdownController) -> Iterator[None]:
    """Install and then restore SIGINT/SIGTERM handlers in the main thread."""

    previous_int = signal.getsignal(signal.SIGINT)
    previous_term = signal.getsignal(signal.SIGTERM)
    signal.signal(signal.SIGINT, controller.handler)
    signal.signal(signal.SIGTERM, controller.handler)
    try:
        yield
    finally:
        signal.signal(signal.SIGINT, previous_int)
        signal.signal(signal.SIGTERM, previous_term)


def run_upstream(
    command: Sequence[str], sender: HintSender, controller: ShutdownController, log: Log
) -> NoReturn:
    """Consume one bounded line at a time and synchronously settle its one hint request."""

    try:
        child = subprocess.Popen(
            list(command),
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
            close_fds=True,
        )
    except OSError as exc:
        raise AdapterFailure(EXIT_UPSTREAM_FAILED, "could not start the upstream command") from exc
    controller.attach(child)
    line_number = 0
    event_count = 0
    try:
        stdout = child.stdout
        if stdout is None:  # Popen contract above always creates this pipe.
            raise AdapterFailure(EXIT_UPSTREAM_FAILED, "upstream stdout pipe was not created")
        while True:
            raw = stdout.readline(MAX_EVENT_LINE_BYTES + 2)
            if controller.stopping():
                raise StopRequested
            if not raw:
                break
            line_number += 1
            line = raw.removesuffix(b"\n").removesuffix(b"\r")
            if len(line) > MAX_EVENT_LINE_BYTES:
                raise AdapterFailure(
                    EXIT_UPSTREAM_PROTOCOL,
                    f"upstream line {line_number} exceeds {MAX_EVENT_LINE_BYTES} bytes",
                )
            candidate = line.strip()
            if not candidate:
                continue
            require_json_object(candidate, line_number)
            status = sender.post(controller.stopping)
            if controller.stopping():
                raise StopRequested
            event_count += 1
            log(f"hint {event_count} accepted with HTTP {status}")
        if controller.stopping():
            raise StopRequested
        try:
            return_code = child.wait(timeout=CHILD_EXIT_GRACE_SECONDS)
        except subprocess.TimeoutExpired as exc:
            raise AdapterFailure(
                EXIT_UPSTREAM_FAILED,
                "upstream closed stdout but did not exit",
            ) from exc
        if controller.stopping():
            raise StopRequested
        if return_code == 0:
            raise AdapterFailure(
                EXIT_UPSTREAM_FAILED,
                f"upstream event stream ended unexpectedly after {event_count} event(s)",
            )
        raise AdapterFailure(
            EXIT_UPSTREAM_FAILED,
            f"upstream command exited with status {return_code}",
        )
    finally:
        clean_child(child)
        controller.detach()


def request_once(
    method: str, endpoint: str, token: str, body: bytes | None, timeout: float, limit: int
) -> tuple[int, bytes]:
    """Make one bounded request and return its status and at most ``limit`` body bytes."""

    parts = urlsplit(endpoint)
    connection: http.client.HTTPConnection | None = None
    headers = {"Authorization": f"Bearer {token}", "Accept": "application/json"}
    if body is not None:
        headers["Content-Type"] = "application/json"
        headers["Content-Length"] = str(len(body))
    try:
        connection = open_connection(parts, timeout)
        connection.request(method, parts.path, body=body, headers=headers)
        response = connection.getresponse()
        try:
            return response.status, response.read(limit + 1)
        finally:
            response.close()
    except (OSError, http.client.HTTPException) as exc:
        raise RetryableDelivery(None) from exc
    finally:
        if connection is not None:
            connection.close()


RecordTransport = Callable[[str, str, str, bytes | None, float, int], tuple[int, bytes]]


class RecordSender(Protocol):
    """The narrow interface the records loop needs from the HTTP client."""

    def post(self, record: bytes, stopping: StopCheck) -> int:
        """Post one record and return 200, 202, or 400 (a record vibe-talk refuses as malformed)."""

    def conversations(self, stopping: StopCheck) -> list[str]:
        """The conversations vibe-talk asks to be tailed, sorted."""


class HttpRecordSender:
    """Authenticated client for ``/api/v1/live/tail``."""

    def __init__(
        self,
        origin: str,
        token: str,
        log: Log,
        *,
        timeout: float = REQUEST_TIMEOUT_SECONDS,
        policy: RetryPolicy = RetryPolicy(),
        sleep: Sleep = time.sleep,
        transport: RecordTransport = request_once,
    ) -> None:
        self.endpoint = f"{origin}{TAIL_PATH}"
        self.token = token
        self.log = log
        self.timeout = timeout
        self.policy = policy
        self.sleep = sleep
        self.transport = transport

    def _request(
        self, method: str, body: bytes | None, limit: int, stopping: StopCheck, accept: set[int]
    ) -> tuple[int, bytes]:
        attempts = len(self.policy.delays) + 1
        last: int | None = None
        for attempt in range(1, attempts + 1):
            if stopping():
                raise StopRequested
            status: int | None
            payload = b""
            try:
                status, payload = self.transport(
                    method, self.endpoint, self.token, body, self.timeout, limit
                )
            except RetryableDelivery as exc:
                status = exc.status
            if status in accept:
                return status, payload
            if status == 429 or (status is not None and 500 <= status <= 599):
                last = status
            elif status is None:
                last = None
            else:
                raise AdapterFailure(
                    EXIT_DELIVERY_FAILED,
                    f"the tail endpoint permanently refused a {method} with HTTP {status}",
                )
            if attempt == attempts:
                break
            result = "a transport failure" if last is None else f"retryable HTTP {last}"
            self.log(f"{method} attempt {attempt}/{attempts} had {result}; retrying")
            self.sleep(self.policy.delays[attempt - 1])
        result = "transport failure" if last is None else f"HTTP {last}"
        raise AdapterFailure(
            EXIT_DELIVERY_FAILED,
            f"{method} to the tail endpoint failed after {attempts} attempts; last result was {result}",
        )

    def post(self, record: bytes, stopping: StopCheck) -> int:
        status, _ = self._request("POST", record, 1024, stopping, {200, 202, 400})
        return status

    def conversations(self, stopping: StopCheck) -> list[str]:
        _, payload = self._request("GET", None, MAX_CHANNELS_RESPONSE_BYTES, stopping, {200})
        return parse_conversations(payload)


def parse_conversations(payload: bytes) -> list[str]:
    """Validate vibe-talk's list of conversations to tail."""

    if len(payload) > MAX_CHANNELS_RESPONSE_BYTES:
        raise AdapterFailure(EXIT_DELIVERY_FAILED, "the conversation list is too large")
    try:
        value: object = json.loads(
            payload.decode("utf-8"), parse_constant=reject_nonstandard_json_constant
        )
    except (UnicodeDecodeError, ValueError, RecursionError) as exc:
        raise AdapterFailure(EXIT_DELIVERY_FAILED, "the conversation list is not JSON") from exc
    listed = value.get("conversations") if isinstance(value, dict) else None
    if not isinstance(listed, list):
        raise AdapterFailure(EXIT_DELIVERY_FAILED, "the conversation list has no conversations")
    conversations: list[str] = []
    for item in cast(list[object], listed):
        if not isinstance(item, str) or CONVERSATION_ID.fullmatch(item) is None:
            raise AdapterFailure(
                EXIT_DELIVERY_FAILED, "the conversation list names an id of the wrong shape"
            )
        conversations.append(item)
    return sorted(set(conversations))


def validate_conversation(raw: str) -> str:
    """Validate one ``--channel`` given in records mode: a provider conversation id."""

    if CONVERSATION_ID.fullmatch(raw) is None:
        raise AdapterFailure(
            EXIT_USAGE, "--channel in --records mode must be a conversation id such as C0123ABCD"
        )
    return raw


class StreamCursor:
    """The stream position of the last record vibe-talk acknowledged, kept in a private file."""

    def __init__(self, path: Path) -> None:
        self.path = path
        self.value: int | None = None

    def load(self) -> None:
        """Read the saved position; a missing file is no position."""

        try:
            raw = self.path.read_bytes()[: MAX_STATE_FILE_BYTES + 1]
        except FileNotFoundError:
            self.value = None
            return
        except OSError as exc:
            raise AdapterFailure(EXIT_USAGE, "cannot read --state-file") from exc
        try:
            value: object = json.loads(raw.decode("utf-8"))
        except (UnicodeDecodeError, ValueError, RecursionError) as exc:
            raise AdapterFailure(EXIT_USAGE, "--state-file is not this adapter's JSON") from exc
        cursor = value.get("cursor") if isinstance(value, dict) else None
        if cursor is not None and (not isinstance(cursor, int) or isinstance(cursor, bool) or cursor < 0):
            raise AdapterFailure(EXIT_USAGE, "--state-file holds no valid stream position")
        self.value = cursor

    def advance(self, sequence: int) -> None:
        """Move the position forward to ``sequence``, never back, and save it."""

        if self.value is not None and sequence <= self.value:
            return
        self.value = sequence
        self._save()

    def forget(self) -> None:
        """Forget the position: the stream cannot resume from it."""

        self.value = None
        self._save()

    def _save(self) -> None:
        body = json.dumps({"cursor": self.value}).encode("utf-8")
        directory = self.path.parent
        descriptor, temporary = tempfile.mkstemp(prefix=".tail-state-", dir=directory)
        try:
            os.fchmod(descriptor, 0o600)
            with os.fdopen(descriptor, "wb") as target:
                target.write(body)
                target.flush()
                os.fsync(target.fileno())
            os.replace(temporary, self.path)
        except OSError as exc:
            with contextlib.suppress(OSError):
                os.unlink(temporary)
            raise AdapterFailure(EXIT_USAGE, "cannot write --state-file") from exc


def validate_template(command: Sequence[str]) -> None:
    """Require both placeholders, so the adapter really chooses the channels and the position."""

    joined = "\0".join(command)
    for placeholder in (CHANNELS_PLACEHOLDER, CATCHUP_PLACEHOLDER):
        if placeholder not in joined:
            raise AdapterFailure(
                EXIT_USAGE,
                f"in --records mode the command after -- must contain {placeholder}",
            )


def build_command(command: Sequence[str], channels: Sequence[str], catchup: str) -> list[str]:
    """Fill the placeholders in each argument; nothing is split or joined by a shell."""

    filled = ",".join(channels)
    return [
        argument.replace(CHANNELS_PLACEHOLDER, filled).replace(CATCHUP_PLACEHOLDER, catchup)
        for argument in command
    ]


def record_header(raw: bytes, line_number: int) -> tuple[str, int | None]:
    """The two fields this adapter reads of a record: its type and its stream position."""

    try:
        value: object = json.loads(
            raw.decode("utf-8"), parse_constant=reject_nonstandard_json_constant
        )
    except (UnicodeDecodeError, ValueError, RecursionError) as exc:
        raise AdapterFailure(
            EXIT_UPSTREAM_PROTOCOL, f"upstream line {line_number} is not a valid JSON object"
        ) from exc
    if not isinstance(value, dict):
        raise AdapterFailure(
            EXIT_UPSTREAM_PROTOCOL, f"upstream line {line_number} is valid JSON but not an object"
        )
    record = cast(dict[str, object], value)
    kind = record.get("type")
    sequence = record.get("iris_seq_id")
    position = (
        sequence
        if isinstance(sequence, int) and not isinstance(sequence, bool) and sequence >= 0
        else None
    )
    return (kind if isinstance(kind, str) else ""), position


class StreamWatch:
    """What a records stream's side threads learned: a retention overflow, a channel change."""

    def __init__(self) -> None:
        self.overflow = threading.Event()
        self.restart = threading.Event()
        self.child: subprocess.Popen[bytes] | None = None


def drain_stderr(stream: io.BufferedReader | None, watch: StreamWatch) -> None:
    """Read and discard the child's stderr, noting only whether it reported an overflow."""

    if stream is None:
        return
    for line in iter(lambda: stream.readline(4096), b""):
        lowered = line.lower()
        if any(marker in lowered for marker in OVERFLOW_MARKERS):
            watch.overflow.set()


def watch_channels(
    sender: RecordSender,
    channels: list[str],
    refresh_seconds: float,
    watch: StreamWatch,
    done: threading.Event,
    log: Log,
) -> None:
    """Ask vibe-talk for its conversations every ``refresh_seconds``; restart the stream on change."""

    while not done.wait(refresh_seconds):
        try:
            current = sender.conversations(done.is_set)
        except (AdapterFailure, StopRequested):
            log("could not refresh the conversation list; keeping the current stream")
            continue
        if current != channels:
            log(f"the conversation list changed to {len(current)} conversation(s); restarting the stream")
            watch.restart.set()
            child = watch.child
            if child is not None:
                signal_process_group(child.pid, signal.SIGTERM)
            return


GAP_RECORD = b'{"type":"GAP","reason":"the stream could not resume from the saved position"}'
GAP_RECORD_SKIPPED = b'{"type":"GAP","reason":"the adapter skipped a record it could not read"}'


STREAM_ENDED = "ended"
STREAM_RESTART = "restart"
STREAM_OVERFLOW = "overflow"


def run_record_stream(
    command: Sequence[str],
    sender: RecordSender,
    controller: ShutdownController,
    cursor: StreamCursor,
    watch: StreamWatch,
    log: Log,
) -> str:
    """Post each record of one stream run; return why the stream ended."""

    try:
        child = subprocess.Popen(
            list(command),
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            start_new_session=True,
            close_fds=True,
        )
    except OSError as exc:
        raise AdapterFailure(EXIT_UPSTREAM_FAILED, "could not start the upstream command") from exc
    controller.attach(child)
    watch.child = child
    if watch.restart.is_set():
        # The list changed before the child existed to be stopped.
        signal_process_group(child.pid, signal.SIGTERM)
    drainer = threading.Thread(
        target=drain_stderr,
        args=(cast(io.BufferedReader | None, child.stderr), watch),
        name="tail-stderr",
        daemon=True,
    )
    drainer.start()
    line_number = 0
    posted = 0
    try:
        stdout = child.stdout
        if stdout is None:  # Popen contract above always creates this pipe.
            raise AdapterFailure(EXIT_UPSTREAM_FAILED, "upstream stdout pipe was not created")
        while True:
            if watch.restart.is_set():
                return STREAM_RESTART
            raw = stdout.readline(MAX_RECORD_LINE_BYTES + 2)
            if controller.stopping():
                raise StopRequested
            if not raw:
                if watch.restart.is_set():
                    return STREAM_RESTART
                break
            line_number += 1
            line = raw.removesuffix(b"\n").removesuffix(b"\r")
            if len(line) > MAX_RECORD_LINE_BYTES:
                # One record no bound can carry. Stopping would replay it from the saved position
                # forever, so the rest of the line is read and dropped, and vibe-talk is told of
                # the gap: it reads the channels, which brings the message the record carried.
                while not raw.endswith(b"\n"):
                    raw = stdout.readline(64 * 1024)
                    if not raw:
                        break
                log(f"upstream line {line_number} exceeds {MAX_RECORD_LINE_BYTES} bytes; skipped as a gap")
                sender.post(GAP_RECORD_SKIPPED, controller.stopping)
                continue
            candidate = line.strip()
            if not candidate:
                continue
            try:
                kind, sequence = record_header(candidate, line_number)
            except AdapterFailure as failure:
                log(f"{failure.message}; skipped as a gap")
                sender.post(GAP_RECORD_SKIPPED, controller.stopping)
                continue
            if kind in CONTROL_TYPES:
                continue
            status = sender.post(candidate, controller.stopping)
            if controller.stopping():
                raise StopRequested
            posted += 1
            if status == 400:
                # Retrying the same bytes cannot succeed, and stopping would replay this record
                # forever: it is skipped, by number, and the position moves past it.
                log(f"record {posted} was refused as malformed (HTTP 400); skipping it")
            if sequence is not None:
                cursor.advance(sequence)
        drainer.join(timeout=CHILD_EXIT_GRACE_SECONDS)
        if controller.stopping():
            raise StopRequested
        if watch.restart.is_set():
            return STREAM_RESTART
        if watch.overflow.is_set():
            return STREAM_OVERFLOW
        return STREAM_ENDED
    finally:
        watch.child = None
        clean_child(child)
        controller.detach()


def run_records(
    template: Sequence[str],
    sender: RecordSender,
    controller: ShutdownController,
    cursor: StreamCursor,
    fixed_channels: list[str] | None,
    initial_catchup: str,
    refresh_seconds: float,
    log: Log,
) -> NoReturn:
    """Tail the conversations, restarting the stream whenever vibe-talk's list changes."""

    waiting_logged = False
    while True:
        if controller.stopping():
            raise StopRequested
        channels = (
            fixed_channels
            if fixed_channels is not None
            else sender.conversations(controller.stopping)
        )
        if not channels:
            if not waiting_logged:
                log("vibe-talk names no conversation to tail yet; asking again later")
                waiting_logged = True
            deadline = time.monotonic() + refresh_seconds
            while time.monotonic() < deadline:
                if controller.stopping():
                    raise StopRequested
                time.sleep(min(0.2, refresh_seconds))
            continue
        waiting_logged = False
        catchup = str(cursor.value) if cursor.value is not None else initial_catchup
        watch = StreamWatch()
        done = threading.Event()
        watcher: threading.Thread | None = None
        if fixed_channels is None:
            watcher = threading.Thread(
                target=watch_channels,
                args=(sender, channels, refresh_seconds, watch, done, log),
                name="tail-channels",
                daemon=True,
            )
            watcher.start()
        log(f"tailing {len(channels)} conversation(s) from {'the saved position' if cursor.value is not None else initial_catchup}")
        try:
            ended = run_record_stream(
                build_command(template, channels, catchup), sender, controller, cursor, watch, log
            )
        finally:
            done.set()
            if watcher is not None:
                watcher.join(timeout=REQUEST_TIMEOUT_SECONDS + 1.0)
        if ended == STREAM_RESTART:
            continue
        if ended == STREAM_OVERFLOW:
            sender.post(GAP_RECORD, controller.stopping)
            cursor.forget()
            raise AdapterFailure(
                EXIT_UPSTREAM_FAILED,
                "the stream could not resume from the saved position; vibe-talk was told of the gap",
            )
        raise AdapterFailure(EXIT_UPSTREAM_FAILED, "upstream event stream ended unexpectedly")


class RecordingHintSender:
    """Content-free test double used by the offline controls."""

    def __init__(self, after_post: Callable[[int], None] | None = None) -> None:
        self.calls = 0
        self.after_post = after_post

    def post(self, stopping: StopCheck) -> int:
        if stopping():
            raise StopRequested
        self.calls += 1
        if self.after_post is not None:
            self.after_post(self.calls)
        return 202


RequestRecord: TypeAlias = tuple[str, str, bytes]


class RecordingServer(ThreadingHTTPServer):
    """Loopback-only HTTP fixture with a deterministic response queue."""

    records: list[RequestRecord]
    statuses: deque[int]
    get_payload: bytes

    def __init__(self, statuses: Sequence[int]) -> None:
        super().__init__(("127.0.0.1", 0), RecordingHandler)
        self.records = []
        self.statuses = deque(statuses)
        self.get_payload = b'{"conversations":[]}'


class RecordingHandler(BaseHTTPRequestHandler):
    """Record only what the adapter sends; never print requests or headers."""

    server: RecordingServer

    def do_POST(self) -> None:
        length_text = self.headers.get("Content-Length", "0")
        try:
            length = int(length_text)
        except ValueError:
            length = 0
        body = self.rfile.read(max(0, length))
        authorization = self.headers.get("Authorization", "")
        self.server.records.append((self.path, authorization, body))
        status = self.server.statuses.popleft() if self.server.statuses else 500
        payload = b'{"accepted":true,"duplicate":false}'
        self.send_response(status)
        if status == 302:
            self.send_header("Location", "/must-not-follow")
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        with contextlib.suppress(BrokenPipeError, ConnectionResetError):
            self.wfile.write(payload)

    def do_GET(self) -> None:
        authorization = self.headers.get("Authorization", "")
        self.server.records.append((f"GET {self.path}", authorization, b""))
        status = self.server.statuses.popleft() if self.server.statuses else 500
        payload = self.server.get_payload
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        with contextlib.suppress(BrokenPipeError, ConnectionResetError):
            self.wfile.write(payload)

    def log_message(self, _format: str, *_args: object) -> None:
        pass


@contextlib.contextmanager
def recording_server(statuses: Sequence[int]) -> Iterator[tuple[RecordingServer, str]]:
    """Run the loopback HTTP fixture and stop it deterministically."""

    server = RecordingServer(statuses)
    host, port = cast(tuple[str, int], server.server_address)
    thread = threading.Thread(target=server.serve_forever, name="hint-adapter-self-test", daemon=True)
    thread.start()
    try:
        yield server, f"http://{host}:{port}"
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=2.0)
        if thread.is_alive():
            raise AssertionError("HTTP fixture did not stop")


def expect_failure(code: int, action: Callable[[], object]) -> AdapterFailure:
    """Run one negative control and require its exact failure class and code."""

    try:
        action()
    except AdapterFailure as exc:
        if exc.code != code:
            raise AssertionError(f"expected exit {code}, got {exc.code}") from exc
        return exc
    raise AssertionError(f"expected exit {code}, action passed")


def self_test() -> int:
    """Run deterministic, offline controls over framing, privacy, retry, and cleanup."""

    controls = 0
    secret = "self-test-secret-token-value"
    content_marker = "MESSAGE-CONTENT-MUST-NOT-LEAK"

    # The real HTTP transport sends exactly the contentless body, retries only retryable statuses,
    # and neither redirects nor places the credential or upstream marker in its logs.
    with recording_server([500, 429, 202]) as (server, base_url):
        endpoint = parse_base_url(base_url)
        logs: list[str] = []
        sleeps: list[float] = []
        sender = HttpHintSender(
            endpoint,
            "fixed-channel",
            secret,
            logs.append,
            policy=RetryPolicy((0.0, 0.0)),
            sleep=sleeps.append,
        )
        if sender.post(lambda: False) != 202:
            raise AssertionError("202 was not accepted")
        expected_body = b'{"channel_id":"fixed-channel"}'
        if len(server.records) != 3 or any(
            record != (HINT_PATH, f"Bearer {secret}", expected_body) for record in server.records
        ):
            raise AssertionError("HTTP request was not the exact contentless contract")
        if sleeps != [0.0, 0.0]:
            raise AssertionError("retry schedule was not followed")
        if secret in "\n".join(logs) or content_marker in "\n".join(logs):
            raise AssertionError("a secret or event marker entered retry logs")
    controls += 1

    with recording_server([200]) as (server, base_url):
        endpoint = parse_base_url(base_url)
        http_sender = HttpHintSender(endpoint, "fixed-channel", secret, lambda _line: None)
        if http_sender.post(lambda: False) != 200 or len(server.records) != 1:
            raise AssertionError("200 coalesced response was not accepted")
    controls += 1

    for permanent_status in (302, 400, 401, 404):
        with recording_server([permanent_status, 202]) as (server, base_url):
            endpoint = parse_base_url(base_url)
            http_sender = HttpHintSender(endpoint, "fixed-channel", secret, lambda _line: None)
            expect_failure(EXIT_DELIVERY_FAILED, lambda: http_sender.post(lambda: False))
            if len(server.records) != 1:
                raise AssertionError(f"HTTP {permanent_status} was retried or redirected")
    controls += 1

    with recording_server([503, 503, 503]) as (server, base_url):
        endpoint = parse_base_url(base_url)
        http_sender = HttpHintSender(
            endpoint,
            "fixed-channel",
            secret,
            lambda _line: None,
            policy=RetryPolicy((0.0, 0.0)),
            sleep=lambda _delay: None,
        )
        expect_failure(EXIT_DELIVERY_FAILED, lambda: http_sender.post(lambda: False))
        if len(server.records) != 3:
            raise AssertionError("retry exhaustion did not use the finite attempt bound")
    controls += 1

    transport_calls = 0

    def flaky_transport(_endpoint: str, _token: str, _body: bytes, _timeout: float) -> int:
        nonlocal transport_calls
        transport_calls += 1
        if transport_calls < 3:
            raise RetryableDelivery(None)
        return 202

    transport_logs: list[str] = []
    transport_sender = HttpHintSender(
        "http://127.0.0.1:1/api/v1/live/hints",
        "fixed-channel",
        secret,
        transport_logs.append,
        policy=RetryPolicy((0.0, 0.0)),
        sleep=lambda _delay: None,
        transport=flaky_transport,
    )
    if transport_sender.post(lambda: False) != 202 or transport_calls != 3:
        raise AssertionError("transport failures were not retried to a later success")
    if not all("transport failure" in line for line in transport_logs):
        raise AssertionError("transport retry diagnostics lost their safe failure class")
    controls += 1

    # Exercise the actual no-shell subprocess path. Blank lines are ignored, two arbitrary JSON
    # objects each trigger one request, and no content is logged. The second post requests shutdown,
    # proving that signal cleanup unblocks the read and reaps the owned child process group.
    with tempfile.TemporaryDirectory(prefix="live-hint-adapter-test-") as temporary:
        sentinel = Path(temporary) / "shell-was-used"
        literal = f"; touch {sentinel} ; {content_marker}"
        program = (
            "import json,sys; "
            "print(); "
            "print(json.dumps({'payload':sys.argv[1]})); "
            "print(json.dumps({'second':True})); "
            "sys.stdout.flush(); "
            "__import__('time').sleep(30)"
        )
        logs = []
        controller = ShutdownController()
        recording_sender = RecordingHintSender(
            lambda count: controller.handler(signal.SIGTERM, None) if count == 2 else None
        )
        try:
            run_upstream(
                [sys.executable, "-c", program, literal],
                recording_sender,
                controller,
                logs.append,
            )
        except StopRequested:
            pass
        else:
            raise AssertionError("requested shutdown did not stop the upstream loop")
        if recording_sender.calls != 2 or sentinel.exists() or controller.child is not None:
            raise AssertionError("literal argv or one-object/one-hint behavior failed")
        rendered = "\n".join(logs)
        if secret in rendered or content_marker in rendered or literal in rendered:
            raise AssertionError("event content entered adapter logs")
    controls += 1

    clean_sender = RecordingHintSender()
    clean_failure = expect_failure(
        EXIT_UPSTREAM_FAILED,
        lambda: run_upstream(
            [sys.executable, "-c", "print('{}')"],
            clean_sender,
            ShutdownController(),
            lambda _line: None,
        ),
    )
    if clean_sender.calls != 1 or "ended unexpectedly" not in clean_failure.message:
        raise AssertionError("a clean event-stream EOF was not diagnosed as a dead source")
    controls += 1

    for invalid_output in (b"[]\n", b"not-json\n", b'{"value": NaN}\n'):
        recording_sender = RecordingHintSender()
        controller = ShutdownController()
        encoded = invalid_output.hex()
        # The long sleep proves failure cleanup terminates and reaps the child instead of waiting
        # for upstream cooperation.
        program = (
            "import sys,time; "
            f"sys.stdout.buffer.write(bytes.fromhex('{encoded}')); "
            "sys.stdout.flush(); time.sleep(30)"
        )
        started = time.monotonic()
        expect_failure(
            EXIT_UPSTREAM_PROTOCOL,
            lambda: run_upstream(
                [sys.executable, "-c", program],
                recording_sender,
                controller,
                lambda _line: None,
            ),
        )
        if time.monotonic() - started >= CHILD_TERM_GRACE_SECONDS:
            raise AssertionError("invalid input did not promptly clean up the child")
        if recording_sender.calls:
            raise AssertionError("invalid input triggered a hint")
    controls += 1

    oversized_program = (
        f"import sys; sys.stdout.write('{{' + 'x' * {MAX_EVENT_LINE_BYTES + 1} + '\\n'); "
        "sys.stdout.flush()"
    )
    expect_failure(
        EXIT_UPSTREAM_PROTOCOL,
        lambda: run_upstream(
            [sys.executable, "-c", oversized_program],
            RecordingHintSender(),
            ShutdownController(),
            lambda _line: None,
        ),
    )
    controls += 1

    with tempfile.TemporaryDirectory(prefix="live-hint-adapter-token-test-") as temporary:
        token_path = Path(temporary) / "token"
        token_path.write_text(secret + "\n", encoding="utf-8")
        token_path.chmod(0o600)
        if read_private_token(token_path) != secret:
            raise AssertionError("private token file did not round-trip")
        token_alias = Path(temporary) / "token-alias"
        os.link(token_path, token_alias)
        expect_failure(EXIT_USAGE, lambda: read_private_token(token_path))
        token_alias.unlink()
        token_path.chmod(0o644)
        expect_failure(EXIT_USAGE, lambda: read_private_token(token_path))
    controls += 1

    controls += records_self_test(secret, content_marker)

    print(f"live-hint-adapter self-test: PASS ({controls} controls)")
    return EXIT_OK


class ScriptedRecordSender:
    """Content-recording test double for the records loop; never logs what it holds."""

    def __init__(
        self,
        statuses: Sequence[int] = (),
        lists: Sequence[list[str]] = (),
        after_post: Callable[[int], None] | None = None,
    ) -> None:
        self.posts: list[bytes] = []
        self.statuses = deque(statuses)
        self.lists = deque(lists)
        self.after_post = after_post
        self.lists_asked = 0

    def post(self, record: bytes, stopping: StopCheck) -> int:
        if stopping():
            raise StopRequested
        self.posts.append(record)
        status = self.statuses.popleft() if self.statuses else 202
        if self.after_post is not None:
            self.after_post(len(self.posts))
        return status

    def conversations(self, stopping: StopCheck) -> list[str]:
        if stopping():
            raise StopRequested
        self.lists_asked += 1
        if len(self.lists) > 1:
            return self.lists.popleft()
        return list(self.lists[0]) if self.lists else []


def records_self_test(secret: str, content_marker: str) -> int:
    """Offline controls for --records: framing, position, refusals, overflow, and channel changes."""

    controls = 0
    record = json.dumps(
        {
            "type": "message.created",
            "iris_seq_id": 5,
            "payload": {"slack_message": {"text": content_marker}},
        }
    ).encode("utf-8")

    # The real transport posts each record byte for byte to the tail route, accepts vibe-talk's
    # 400 for a malformed record, retries only retryable statuses, and logs no content.
    with recording_server([503, 202, 400]) as (server, base_url):
        logs: list[str] = []
        sender = HttpRecordSender(
            parse_origin(base_url),
            secret,
            logs.append,
            policy=RetryPolicy((0.0,)),
            sleep=lambda _delay: None,
        )
        if sender.post(record, lambda: False) != 202 or sender.post(record, lambda: False) != 400:
            raise AssertionError("record statuses were not returned")
        if [entry for entry in server.records] != [(TAIL_PATH, f"Bearer {secret}", record)] * 3:
            raise AssertionError("records were not posted verbatim to the tail route")
        if secret in "\n".join(logs) or content_marker in "\n".join(logs):
            raise AssertionError("a secret or record content entered the logs")
    with recording_server([401]) as (server, base_url):
        sender = HttpRecordSender(parse_origin(base_url), secret, lambda _line: None)
        expect_failure(EXIT_DELIVERY_FAILED, lambda: sender.post(record, lambda: False))
        if len(server.records) != 1:
            raise AssertionError("a permanent refusal was retried")
    controls += 1

    # The conversation list is read with the same credential and checked id by id.
    with recording_server([200, 200]) as (server, base_url):
        server.get_payload = b'{"conversations":["C0000BBBB2","C0000AAAA1","C0000AAAA1"]}'
        sender = HttpRecordSender(parse_origin(base_url), secret, lambda _line: None)
        if sender.conversations(lambda: False) != ["C0000AAAA1", "C0000BBBB2"]:
            raise AssertionError("the conversation list was not deduplicated and sorted")
        server.get_payload = b'{"conversations":["general; rm -rf"]}'
        expect_failure(EXIT_DELIVERY_FAILED, lambda: sender.conversations(lambda: False))
        if server.records[0] != (f"GET {TAIL_PATH}", f"Bearer {secret}", b""):
            raise AssertionError("the conversation list was not asked for with the credential")
    controls += 1

    # The position file starts empty, moves only forward, survives a reload, is private, and a
    # corrupt one refuses to start rather than replaying from a guess.
    with tempfile.TemporaryDirectory(prefix="live-hint-adapter-state-") as temporary:
        path = Path(temporary) / "tail.json"
        cursor = StreamCursor(path)
        cursor.load()
        if cursor.value is not None:
            raise AssertionError("a missing state file was a position")
        cursor.advance(9)
        cursor.advance(4)
        reloaded = StreamCursor(path)
        reloaded.load()
        if reloaded.value != 9 or stat.S_IMODE(path.stat().st_mode) != 0o600:
            raise AssertionError("the position did not persist privately and monotonically")
        reloaded.forget()
        forgotten = StreamCursor(path)
        forgotten.load()
        if forgotten.value is not None:
            raise AssertionError("a forgotten position came back")
        path.write_text('{"cursor": -1}', encoding="utf-8")
        expect_failure(EXIT_USAGE, StreamCursor(path).load)
    controls += 1

    # Placeholders are filled argument by argument and both are required.
    template = ["tail", "--channel-id={channels}", "--catchup={catchup}", "; {channels}"]
    if build_command(template, ["C0000AAAA1", "C0000BBBB2"], "41") != [
        "tail",
        "--channel-id=C0000AAAA1,C0000BBBB2",
        "--catchup=41",
        "; C0000AAAA1,C0000BBBB2",
    ]:
        raise AssertionError("placeholders were not filled literally")
    expect_failure(EXIT_USAGE, lambda: validate_template(["tail", "--channel-id={channels}"]))
    expect_failure(EXIT_USAGE, lambda: validate_conversation("general"))
    controls += 1

    # One stream run: heartbeats stay local, records go out verbatim, a record vibe-talk refuses
    # as malformed is skipped with the position moved past it, and a clean end is reported.
    lines = [
        {"type": "HEARTBEAT"},
        json.loads(record.decode("utf-8")),
        {"type": "message.created", "iris_seq_id": 6, "payload": {}},
        {"type": "GAP", "lost_from_iris_seq_id": 1},
    ]
    program = (
        "import json,sys; "
        f"[print(json.dumps(line)) for line in json.loads({json.dumps(json.dumps(lines))})]; "
        "sys.stdout.flush()"
    )
    with tempfile.TemporaryDirectory(prefix="live-hint-adapter-stream-") as temporary:
        cursor = StreamCursor(Path(temporary) / "tail.json")
        scripted = ScriptedRecordSender(statuses=[202, 400, 202])
        logs = []
        ended = run_record_stream(
            [sys.executable, "-c", program],
            scripted,
            ShutdownController(),
            cursor,
            StreamWatch(),
            logs.append,
        )
        if ended != STREAM_ENDED or len(scripted.posts) != 3 or cursor.value != 6:
            raise AssertionError("heartbeats, refusals, or the position were mishandled")
        if json.loads(scripted.posts[0].decode("utf-8")) != lines[1]:
            raise AssertionError("a record was not forwarded as it stands")
        if content_marker in "\n".join(logs):
            raise AssertionError("record content entered the logs")
    controls += 1

    # A line no record can be (not JSON, or past the bound) is skipped as a gap rather than
    # stopping the adapter, which would replay it from the saved position forever.
    unreadable_program = (
        "import json,sys; "
        "print('not json'); "
        f"print('{{' + 'x' * {MAX_RECORD_LINE_BYTES + 10} + '}}'); "
        "print(json.dumps({'type':'message.created','iris_seq_id':12,'payload':{}})); "
        "sys.stdout.flush()"
    )
    with tempfile.TemporaryDirectory(prefix="live-hint-adapter-unreadable-") as temporary:
        cursor = StreamCursor(Path(temporary) / "tail.json")
        scripted = ScriptedRecordSender()
        ended = run_record_stream(
            [sys.executable, "-c", unreadable_program],
            scripted,
            ShutdownController(),
            cursor,
            StreamWatch(),
            lambda _line: None,
        )
        if ended != STREAM_ENDED or scripted.posts[:2] != [GAP_RECORD_SKIPPED] * 2:
            raise AssertionError("unreadable lines were not skipped as gaps")
        if len(scripted.posts) != 3 or cursor.value != 12:
            raise AssertionError("the record after an unreadable line was lost")
    controls += 1

    # A stream that cannot resume from its saved position says so on stderr: vibe-talk is told
    # of the gap, the position is forgotten, and the adapter stops for its service manager.
    overflow_program = (
        "import sys; sys.stderr.write('queue overflow: no longer queued\\n'); sys.stderr.flush()"
    )
    with tempfile.TemporaryDirectory(prefix="live-hint-adapter-overflow-") as temporary:
        cursor = StreamCursor(Path(temporary) / "tail.json")
        cursor.advance(77)
        scripted = ScriptedRecordSender()
        failure = expect_failure(
            EXIT_UPSTREAM_FAILED,
            lambda: run_records(
                [sys.executable, "-c", overflow_program, "{channels}", "{catchup}"],
                scripted,
                ShutdownController(),
                cursor,
                ["C0000AAAA1"],
                "none",
                60.0,
                lambda _line: None,
            ),
        )
        if scripted.posts != [GAP_RECORD] or cursor.value is not None:
            raise AssertionError("an overflow did not report a gap and forget the position")
        if "gap" not in failure.message:
            raise AssertionError("the overflow failure did not say what happened")
    controls += 1

    # vibe-talk's list changes while a stream runs: the stream is restarted with the new list
    # and the saved position. Each child prints its own arguments as a record, and the second
    # record stops the adapter.
    echo_program = (
        "import json,sys; "
        "print(json.dumps({'type':'message.created','iris_seq_id':len(sys.argv)+10,"
        "'payload':{'argv':sys.argv[1:]}})); sys.stdout.flush(); "
        "__import__('time').sleep(30)"
    )
    with tempfile.TemporaryDirectory(prefix="live-hint-adapter-channels-") as temporary:
        cursor = StreamCursor(Path(temporary) / "tail.json")
        controller = ShutdownController()
        scripted = ScriptedRecordSender(
            lists=[["C0000AAAA1"], ["C0000AAAA1", "C0000BBBB2"]],
            after_post=lambda count: controller.handler(signal.SIGTERM, None) if count == 2 else None,
        )
        started = time.monotonic()
        try:
            run_records(
                [sys.executable, "-c", echo_program, "--channel-id={channels}", "--catchup={catchup}"],
                scripted,
                controller,
                cursor,
                None,
                "none",
                0.2,
                lambda _line: None,
            )
        except StopRequested:
            pass
        else:
            raise AssertionError("the records loop did not stop when asked")
        if time.monotonic() - started > 20:
            raise AssertionError("a channel change did not restart the stream promptly")
        argvs = [
            cast(dict[str, dict[str, list[str]]], json.loads(post.decode("utf-8")))["payload"]["argv"]
            for post in scripted.posts
        ]
        if argvs != [
            ["--channel-id=C0000AAAA1", "--catchup=none"],
            ["--channel-id=C0000AAAA1,C0000BBBB2", "--catchup=13"],
        ]:
            raise AssertionError(f"the restarted stream was not built from the new list: {argvs}")
    controls += 1
    return controls


def build_parser() -> argparse.ArgumentParser:
    """Build the operator-side parser; upstream argv is split off before this parser runs."""

    parser = argparse.ArgumentParser(
        usage=(
            "%(prog)s --url URL --channel ID --token-file FILE -- COMMAND [ARG ...]\n"
            "       %(prog)s --records --url URL --token-file FILE --state-file FILE "
            "[--channel ID ...] -- COMMAND [ARG ...]\n"
            "       %(prog)s --self-test"
        ),
        description=(
            "Convert a provider's JSON-lines event stream into vibe-talk live input. By default "
            "every nonblank JSON object triggers one content-free hint for one channel; filter "
            "message events and heartbeats in the upstream command. With --records, each record "
            "is posted as it stands, text included, so vibe-talk publishes the message without "
            "reading the channel again."
        ),
        epilog=(
            "Hints example: live-hint-adapter.py --url http://127.0.0.1:8080 --channel C0123 "
            "--token-file /run/credentials/hint-token -- provider-events --json-lines C0123. "
            "Records example: live-hint-adapter.py --records --url http://127.0.0.1:8080 "
            "--token-file /run/credentials/hint-token --state-file ~/.local/state/tail.json -- "
            "provider-cli conversation tail --channel-id={channels} --catchup={catchup} "
            "--event-types=message.created,reaction.created,reaction.deleted "
            "--include-iris-seq-id --heartbeats --quiet --output=json. "
            "Run --self-test for deterministic offline controls."
        ),
        allow_abbrev=False,
    )
    parser.add_argument(
        "--url",
        required=True,
        help=(
            "vibe-talk HTTP(S) origin, with no path (plain HTTP is accepted only on loopback); "
            f"hints are posted to {HINT_PATH}, records to {TAIL_PATH}"
        ),
    )
    parser.add_argument(
        "--records",
        action="store_true",
        help=(
            "post each stream record, text included, instead of a content-free hint; the command "
            f"after -- must contain {CHANNELS_PLACEHOLDER} (the comma-separated conversations) and "
            f"{CATCHUP_PLACEHOLDER} (the saved stream position, or --initial-catchup)"
        ),
    )
    parser.add_argument(
        "--channel",
        action="append",
        default=[],
        help=(
            "hints: the one allowlisted provider channel id, the only event-derived identity sent "
            "to vibe-talk (required, once). Records: a conversation to tail, repeatable; when "
            f"absent, every conversation vibe-talk names at GET {TAIL_PATH} is tailed"
        ),
    )
    parser.add_argument(
        "--token-file",
        required=True,
        type=Path,
        help=(
            "private (mode 0600 or stricter) regular file containing the ingest bearer token; "
            "the token is never accepted on argv or printed"
        ),
    )
    parser.add_argument(
        "--state-file",
        type=Path,
        help=(
            "records only (required there): where the stream position of the last record "
            "vibe-talk acknowledged is kept, written atomically with mode 0600"
        ),
    )
    parser.add_argument(
        "--initial-catchup",
        choices=("none", "oldest"),
        default="none",
        help=(
            "records only: where a stream with no saved position starts (default none: from now; "
            "vibe-talk's poller reads what came before)"
        ),
    )
    parser.add_argument(
        "--channels-refresh",
        type=float,
        default=DEFAULT_CHANNELS_REFRESH_SECONDS,
        help=(
            "records only: seconds between asking vibe-talk which conversations to tail, when "
            f"--channel is not given (default {DEFAULT_CHANNELS_REFRESH_SECONDS:g}, at least "
            f"{MIN_CHANNELS_REFRESH_SECONDS:g})"
        ),
    )
    return parser


def split_operator_and_upstream(argv: Sequence[str]) -> tuple[list[str], list[str]]:
    """Require a literal separator so upstream flags cannot be consumed as adapter flags."""

    try:
        separator = argv.index("--")
    except ValueError as exc:
        raise AdapterFailure(EXIT_USAGE, "a literal -- must precede the upstream command") from exc
    operator = list(argv[:separator])
    upstream = list(argv[separator + 1 :])
    if not upstream:
        raise AdapterFailure(EXIT_USAGE, "the literal -- must be followed by an upstream command")
    return operator, upstream


def main(argv: Sequence[str] | None = None) -> int:
    """CLI entry point."""

    arguments = list(sys.argv[1:] if argv is None else argv)
    if arguments == ["--self-test"]:
        try:
            return self_test()
        except (AssertionError, AdapterFailure) as exc:
            # Self-test failures name only the failed invariant, never a request or credential.
            print(f"live-hint-adapter self-test: FAIL: {exc}", file=sys.stderr)
            return EXIT_CONTROL_FAILED
    if arguments in (["-h"], ["--help"]):
        build_parser().parse_args(arguments)
        return EXIT_OK  # argparse's help action exits before this line.
    controller = ShutdownController()
    try:
        operator_arguments, upstream = split_operator_and_upstream(arguments)
        namespace = build_parser().parse_args(operator_arguments)
        channels = cast(list[str], namespace.channel)
        token_file = cast(Path, namespace.token_file)
        if cast(bool, namespace.records):
            origin = parse_origin(cast(str, namespace.url))
            state_file = cast(Path | None, namespace.state_file)
            if state_file is None:
                raise AdapterFailure(EXIT_USAGE, "--records requires --state-file")
            refresh = cast(float, namespace.channels_refresh)
            if not refresh >= MIN_CHANNELS_REFRESH_SECONDS:
                raise AdapterFailure(
                    EXIT_USAGE,
                    f"--channels-refresh must be at least {MIN_CHANNELS_REFRESH_SECONDS:g} seconds",
                )
            validate_template(upstream)
            fixed = sorted({validate_conversation(channel) for channel in channels}) or None
            token = read_private_token(token_file)
            cursor = StreamCursor(state_file)
            cursor.load()
            record_sender = HttpRecordSender(origin, token, stderr_log)
            with installed_signal_handlers(controller):
                run_records(
                    upstream,
                    record_sender,
                    controller,
                    cursor,
                    fixed,
                    cast(str, namespace.initial_catchup),
                    refresh,
                    stderr_log,
                )
        if len(channels) != 1:
            raise AdapterFailure(EXIT_USAGE, "hints need exactly one --channel")
        if namespace.state_file is not None:
            raise AdapterFailure(EXIT_USAGE, "--state-file applies only to --records")
        endpoint = parse_base_url(cast(str, namespace.url))
        channel = validate_channel(channels[0])
        token = read_private_token(token_file)
        sender = HttpHintSender(endpoint, channel, token, stderr_log)
        with installed_signal_handlers(controller):
            run_upstream(upstream, sender, controller, stderr_log)
    except StopRequested:
        signum = controller.signal_number or signal.SIGTERM
        return 128 + int(signum or signal.SIGTERM)
    except AdapterFailure as exc:
        stderr_log(exc.message)
        return exc.code


if __name__ == "__main__":
    raise SystemExit(main())
