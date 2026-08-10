#!/usr/bin/env python3
"""Black-box Python/Rust differential for :command:`herdr-agent`.

Each implementation talks to an isolated executable protocol fixture through its production
client path. The fixture models pane identity, stable sessions, native readiness events, atomic
submission, transcript reads, busy targets, and ambiguous transport failure. The harness also
compares durable queue transitions and crash-sensitive machine outcomes.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
import tempfile
import time
from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
TIMEOUT_SECONDS = 30
_MESSAGE_ID = re.compile(r"\b\d{20,}-\d+\b")


@dataclass(frozen=True)
class Outcome:
    """One normalized command result."""

    returncode: int
    stdout: str
    stderr: str


@dataclass
class Report:
    """Accumulated cross-edition assertions."""

    checks: int = 0
    failures: list[str] = field(default_factory=list)

    def require(self, label: str, condition: bool, detail: str) -> None:
        self.checks += 1
        if not condition:
            self.failures.append(f"{label}: {detail}")

    def exact(self, label: str, python: Outcome, rust: Outcome, expected_rc: int) -> None:
        self.require(
            label,
            python == rust and python.returncode == expected_rc,
            f"expected rc {expected_rc}; python={python!r} rust={rust!r}",
        )


@dataclass(frozen=True)
class PairCase:
    """Equivalent isolated state roots and protocol fixtures."""

    python_root: Path
    rust_root: Path


_FAKE_HERDR = r'''import json
import os
import sys
import time

root = os.path.dirname(os.path.realpath(__file__))
state_path = os.path.join(root, "state.json")
with open(state_path, encoding="utf-8") as handle:
    state = json.load(handle)
args = sys.argv[1:]
state.setdefault("calls", []).append(args)

def save():
    temporary = state_path + ".tmp"
    with open(temporary, "w", encoding="utf-8") as handle:
        json.dump(state, handle, sort_keys=True)
    os.replace(temporary, state_path)

def envelope(result):
    print(json.dumps({"result": result}, sort_keys=True))

if args[:2] == ["pane", "list"]:
    envelope({"panes": [{"pane_id": "w1:p1", "tab_id": "w1:t1", "workspace_id": "w1"}]})
elif args[:2] == ["pane", "get"]:
    pane = args[2]
    if pane != "w1:p1":
        print("missing pane", file=sys.stderr)
        save()
        raise SystemExit(1)
    envelope({"pane": {
        "pane_id": pane,
        "workspace_id": "w1",
        "cwd": root,
        "agent": "codex",
        "agent_status": state.get("status", "idle"),
        "agent_session": {"agent": "codex", "value": "session-1"},
    }})
elif args[:2] == ["workspace", "get"]:
    envelope({"workspace": {
        "workspace_id": state.get("workspace_response_id", "w1"),
        "label": "project",
    }})
elif args[:2] == ["pane", "run"]:
    state.setdefault("submitted", []).append(args[3])
    if state.get("run_mode") == "gate":
        save()
        with open(os.path.join(root, "run-entered"), "w", encoding="utf-8") as handle:
            handle.write(args[3])
        deadline = time.monotonic() + 20
        while not os.path.exists(os.path.join(root, "run-release")):
            if time.monotonic() >= deadline:
                print("gate timed out", file=sys.stderr)
                raise SystemExit(1)
            time.sleep(0.01)
    if state.get("run_mode") == "fail":
        print("transport lost after write", file=sys.stderr)
        save()
        raise SystemExit(1)
elif args[:2] == ["wait", "agent-status"]:
    if state.get("wait_mode") == "fail":
        print("working transition unavailable", file=sys.stderr)
        save()
        raise SystemExit(1)
    print(json.dumps({"data": {"pane_id": args[2], "agent_status": "working"}}, sort_keys=True))
elif args[:2] == ["pane", "read"]:
    source = args[args.index("--source") + 1]
    if source == "recent-unwrapped":
        sys.stdout.write(state.get("read_unwrapped", "agent transcript\n"))
    else:
        sys.stdout.write(state.get("read_recent", "fallback transcript\n"))
else:
    print("unsupported fake Herdr call: " + repr(args), file=sys.stderr)
    save()
    raise SystemExit(2)
save()
'''


class Harness:
    """Create paired fixtures and invoke both implementations."""

    def __init__(self, root: Path, python: Sequence[str], rust: Sequence[str]) -> None:
        self.root = root
        self.python = tuple(python)
        self.rust = tuple(rust)
        self.serial = 0

    def case(self, label: str, state: Mapping[str, object] | None = None) -> PairCase:
        self.serial += 1
        safe = re.sub(r"[^A-Za-z0-9_.-]+", "-", label).strip("-") or "case"
        base = self.root / f"{self.serial:03d}-{safe}"
        python_root = base / "python" / "project"
        rust_root = base / "rust" / "project"
        initial: dict[str, object] = {"status": "idle", "calls": [], "submitted": []}
        if state is not None:
            initial.update(state)
        for root in (python_root, rust_root):
            root.mkdir(parents=True, mode=0o700)
            fixture = root / "fake-herdr"
            fixture.write_text(f"#!{sys.executable}\n{_FAKE_HERDR}", encoding="utf-8")
            fixture.chmod(0o700)
            (root / "state.json").write_text(
                json.dumps(initial, sort_keys=True), encoding="utf-8"
            )
        return PairCase(python_root, rust_root)

    def invoke(self, case: PairCase, arguments: Sequence[str]) -> tuple[Outcome, Outcome]:
        return (
            self._invoke_one(self.python, case.python_root, arguments),
            self._invoke_one(self.rust, case.rust_root, arguments),
        )

    def _invoke_one(
        self, command: Sequence[str], root: Path, arguments: Sequence[str]
    ) -> Outcome:
        expanded = [
            value.replace("<ROOT>", str(root)).replace("<HERDR>", str(root / "fake-herdr"))
            for value in arguments
        ]
        environment = dict(os.environ)
        existing = environment.get("PYTHONPATH", "")
        local = str(REPO_ROOT / "py")
        environment["PYTHONPATH"] = local if not existing else local + os.pathsep + existing
        environment.update({"LANG": "C.UTF-8", "LC_ALL": "C.UTF-8", "NO_COLOR": "1"})
        try:
            completed = subprocess.run(
                [*command, *expanded],
                cwd=root,
                env=environment,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                check=False,
                timeout=TIMEOUT_SECONDS,
            )
            outcome = Outcome(completed.returncode, completed.stdout, completed.stderr)
        except subprocess.TimeoutExpired as error:
            outcome = Outcome(
                124,
                _decode(error.stdout),
                _decode(error.stderr) + "\nTIMEOUT\n",
            )
        return _normalize(outcome, root)


def _decode(value: bytes | str | None) -> str:
    if value is None:
        return ""
    if isinstance(value, bytes):
        return value.decode("utf-8", errors="replace")
    return value


def _normalize(outcome: Outcome, root: Path) -> Outcome:
    def text(value: str) -> str:
        return _MESSAGE_ID.sub("<MESSAGE_ID>", value.replace(str(root), "<ROOT>"))

    return Outcome(outcome.returncode, text(outcome.stdout), text(outcome.stderr))


def _state(root: Path) -> dict[str, object]:
    raw: object = json.loads((root / "state.json").read_text(encoding="utf-8"))
    if not isinstance(raw, dict):
        raise AssertionError("fake Herdr state is not an object")
    return {str(key): value for key, value in raw.items()}


def _queue_snapshot(root: Path, queue: str) -> dict[str, object]:
    queue_root = root / queue
    snapshot: dict[str, object] = {}
    if not queue_root.exists():
        return snapshot
    for path in sorted(queue_root.rglob("*")):
        relative = path.relative_to(queue_root).as_posix()
        if path.is_dir() or relative.startswith(".") or relative == "target.json":
            continue
        if path.suffix == ".json" or path.name.endswith(".json.error"):
            try:
                value: object = json.loads(path.read_text(encoding="utf-8"))
            except (OSError, UnicodeError, json.JSONDecodeError):
                snapshot[_MESSAGE_ID.sub("<MESSAGE_ID>", relative)] = path.read_bytes().hex()
                continue
            if isinstance(value, dict):
                for key in (
                    "queued_at",
                    "inflight_at",
                    "confirmed_at",
                    "delivery_failed_at",
                    "delivery_blocked_at",
                    "failed_at",
                ):
                    value.pop(key, None)
                if isinstance(value.get("id"), str):
                    value["id"] = _MESSAGE_ID.sub("<MESSAGE_ID>", str(value["id"]))
                if isinstance(value.get("artifact"), str):
                    value["artifact"] = _MESSAGE_ID.sub(
                        "<MESSAGE_ID>", str(value["artifact"])
                    )
            snapshot[_MESSAGE_ID.sub("<MESSAGE_ID>", relative)] = value
    return snapshot


def _bootstrap(harness: Harness, report: Report) -> None:
    case = harness.case("bootstrap")
    python, rust = harness.invoke(case, ("--version",))
    report.exact("bootstrap/version", python, rust, 0)
    python, rust = harness.invoke(case, ("--userguide",))
    report.exact("bootstrap/userguide", python, rust, 0)
    report.require(
        "bootstrap/userguide-shape",
        "possibly_submitted" in python.stdout and "inflight" in python.stdout,
        f"installed guide omitted recovery contract: {python!r}",
    )
    help_python, help_rust = harness.invoke(case, ("--help",))
    required = (
        "send",
        "drain",
        "status",
        "read",
        "--pane",
        "--session",
        "--queue",
        "--ready-timeout",
        "--working-timeout",
        "--herdr-bin",
    )
    for edition, outcome in (("python", help_python), ("rust", help_rust)):
        report.require(
            f"bootstrap/help/{edition}",
            outcome.returncode == 0
            and outcome.stderr == ""
            and all(value in outcome.stdout for value in required),
            f"help schema incomplete: {outcome!r}",
        )
    for edition, outcome in zip(
        ("python", "rust"), harness.invoke(case, ()), strict=True
    ):
        report.require(
            f"bootstrap/bare/{edition}",
            outcome.returncode == 0 and "send" in outcome.stdout and outcome.stderr == "",
            f"bare invocation is not an orientation: {outcome!r}",
        )


def _status_and_read(harness: Harness, report: Report) -> None:
    case = harness.case("status")
    common = (
        "--herdr-bin",
        "<HERDR>",
        "--pane",
        "w1:p1",
        "--agent",
        "codex",
        "--workspace",
        "project",
        "--cwd",
        "<ROOT>",
    )
    python, rust = harness.invoke(case, ("status", *common, "--queue", "<ROOT>/queue"))
    report.exact("status/exact-pane", python, rust, 0)
    report.require(
        "status/observational",
        not (case.python_root / "queue").exists() and not (case.rust_root / "queue").exists(),
        "status created an absent queue",
    )

    python, rust = harness.invoke(
        case,
        (
            "status",
            "--herdr-bin",
            "<HERDR>",
            "--session-agent",
            "codex",
            "--session",
            "session-1",
            "--queue",
            "<ROOT>/queue",
        ),
    )
    report.exact("status/stable-session", python, rust, 0)

    for root in (case.python_root, case.rust_root):
        state = _state(root)
        state["read_unwrapped"] = ""
        state["read_recent"] = "fallback transcript\n"
        (root / "state.json").write_text(json.dumps(state, sort_keys=True), encoding="utf-8")
    python, rust = harness.invoke(case, ("read", *common, "--lines", "17"))
    report.exact("read/fallback", python, rust, 0)
    report.require(
        "read/fallback-shape",
        python.stdout == "fallback transcript\n",
        f"unexpected transcript: {python!r}",
    )


def _successful_send(harness: Harness, report: Report) -> None:
    case = harness.case("successful-send")
    text = "first line\nsecond 'quoted' line\nthird line"
    success_arguments = (
        "send",
        text,
        "--herdr-bin",
        "<HERDR>",
        "--pane",
        "w1:p1",
        "--queue",
        "<ROOT>/queue",
    )
    python, rust = harness.invoke(case, success_arguments)
    report.exact("send/success", python, rust, 0)
    report.require(
        "send/literal-atomic-text",
        _state(case.python_root).get("submitted") == [text]
        and _state(case.rust_root).get("submitted") == [text],
        "one implementation changed or split the submitted text",
    )
    report.require(
        "send/durable-state",
        _queue_snapshot(case.python_root, "queue") == _queue_snapshot(case.rust_root, "queue"),
        "processed queue artifacts differ",
    )


def _pending_and_ambiguous(harness: Harness, report: Report) -> None:
    busy = harness.case("pending", {"status": "working"})
    pending_arguments = (
        "send",
        "keep pending",
        "--herdr-bin",
        "<HERDR>",
        "--pane",
        "w1:p1",
        "--queue",
        "<ROOT>/queue",
        "--ready-timeout",
        "0",
    )
    python, rust = harness.invoke(busy, pending_arguments)
    report.exact("send/pending", python, rust, 75)
    report.require(
        "send/pending-state",
        _queue_snapshot(busy.python_root, "queue")
        == _queue_snapshot(busy.rust_root, "queue"),
        "pending queue artifacts differ",
    )
    report.require(
        "send/pending-not-injected",
        _state(busy.python_root).get("submitted") == []
        and _state(busy.rust_root).get("submitted") == [],
        "busy prompt was injected",
    )

    ambiguous = harness.case("ambiguous", {"run_mode": "fail"})
    ambiguous_arguments = (
        "send",
        "only once",
        "--herdr-bin",
        "<HERDR>",
        "--pane",
        "w1:p1",
        "--queue",
        "<ROOT>/queue",
    )
    python, rust = harness.invoke(ambiguous, ambiguous_arguments)
    report.exact("send/possibly-submitted", python, rust, 76)
    report.require(
        "send/possibly-submitted-once",
        _state(ambiguous.python_root).get("submitted") == ["only once"]
        and _state(ambiguous.rust_root).get("submitted") == ["only once"],
        "ambiguous prompt was not attempted exactly once",
    )
    report.require(
        "send/ambiguous-state",
        _queue_snapshot(ambiguous.python_root, "queue")
        == _queue_snapshot(ambiguous.rust_root, "queue"),
        "failed queue artifacts differ",
    )


def _adversarial_queue(harness: Harness, report: Report) -> None:
    case = harness.case("malformed-head")
    for root in (case.python_root, case.rust_root):
        inbox = root / "queue" / "inbox"
        inbox.mkdir(parents=True, mode=0o700)
        (inbox / "000000000001.json").write_bytes(b"{not json\n")
        (inbox / "000000000002.json").write_text(
            '{"id":"good","text":"deliver after poison"}\n', encoding="utf-8"
        )
    arguments = (
        "drain",
        "--herdr-bin",
        "<HERDR>",
        "--pane",
        "w1:p1",
        "--queue",
        "<ROOT>/queue",
    )
    python, rust = harness.invoke(case, arguments)
    report.exact("queue/malformed-head", python, rust, 76)
    report.require(
        "queue/malformed-raw-preserved",
        (case.python_root / "queue/failed/000000000001.json").read_bytes()
        == (case.rust_root / "queue/failed/000000000001.json").read_bytes()
        == b"{not json\n",
        "malformed bytes were changed",
    )

    contradiction = harness.case("contradictory-target")
    python, rust = harness.invoke(
        contradiction,
        (
            "status",
            "--herdr-bin",
            "<HERDR>",
            "--pane",
            "wrong:pane",
            "--session-agent",
            "codex",
            "--session",
            "session-1",
        ),
    )
    report.require(
        "target/pane-session-contradiction",
        python.returncode == rust.returncode == 75
        and "expected exact pane" in python.stderr
        and "expected exact pane" in rust.stderr,
        f"contradictory identity did not fail closed: python={python!r} rust={rust!r}",
    )

    for label, payload in (
        ("nonfinite-json", b'{"id":"bad","text":NaN}\n'),
        ("boolean-attempts", b'{"id":"bad","text":"prompt","delivery_attempts":true}\n'),
        ("empty-text", b'{"id":"bad","text":""}\n'),
    ):
        invalid = harness.case(label)
        for root in (invalid.python_root, invalid.rust_root):
            inbox = root / "queue/inbox"
            inbox.mkdir(parents=True, mode=0o700)
            (inbox / "bad.json").write_bytes(payload)
        python, rust = harness.invoke(
            invalid,
            (
                "drain",
                "--herdr-bin",
                "<HERDR>",
                "--pane",
                "w1:p1",
                "--queue",
                "<ROOT>/queue",
            ),
        )
        report.exact(f"queue/{label}", python, rust, 76)

    exhausted = harness.case("exhausted-attempts")
    for root in (exhausted.python_root, exhausted.rust_root):
        inbox = root / "queue/inbox"
        inbox.mkdir(parents=True, mode=0o700)
        (inbox / "exhausted.json").write_text(
            '{"id":"exhausted","text":"keep me","delivery_attempts":1}\n',
            encoding="utf-8",
        )
    python, rust = harness.invoke(
        exhausted,
        (
            "drain",
            "--herdr-bin",
            "<HERDR>",
            "--pane",
            "w1:p1",
            "--queue",
            "<ROOT>/queue",
            "--max-attempts",
            "1",
        ),
    )
    report.exact("queue/exhausted-is-pending", python, rust, 75)

    special = harness.case("fifo-artifact")
    for root in (special.python_root, special.rust_root):
        inbox = root / "queue/inbox"
        inbox.mkdir(parents=True, mode=0o700)
        os.mkfifo(inbox / "pipe.json", mode=0o600)
    python, rust = harness.invoke(
        special,
        (
            "drain",
            "--herdr-bin",
            "<HERDR>",
            "--pane",
            "w1:p1",
            "--queue",
            "<ROOT>/queue",
        ),
    )
    report.exact("queue/fifo-does-not-block", python, rust, 76)

    missing = harness.case("missing-target")
    python, rust = harness.invoke(
        missing,
        ("send", "do not strand", "--herdr-bin", "<HERDR>", "--queue", "<ROOT>/queue"),
    )
    report.require(
        "target/missing-no-mutation",
        python.returncode == rust.returncode == 75
        and not (missing.python_root / "queue").exists()
        and not (missing.rust_root / "queue").exists(),
        f"missing target created durable state: python={python!r} rust={rust!r}",
    )

    wrong_workspace = harness.case("wrong-workspace-response", {"workspace_response_id": "w9"})
    python, rust = harness.invoke(
        wrong_workspace,
        (
            "status",
            "--herdr-bin",
            "<HERDR>",
            "--pane",
            "w1:p1",
            "--workspace",
            "project",
        ),
    )
    report.require(
        "target/workspace-response-id",
        python.returncode == rust.returncode == 69
        and "returned workspace" in python.stderr
        and "returned workspace" in rust.stderr,
        f"wrong workspace object was accepted: python={python!r} rust={rust!r}",
    )


def _shared_queue_interop(harness: Harness, report: Report) -> None:
    """Each edition must consume the other edition's binding and pending artifact."""

    for label, producer, consumer in (
        ("python-to-rust", harness.python, harness.rust),
        ("rust-to-python", harness.rust, harness.python),
    ):
        case = harness.case(f"interop-{label}", {"status": "working"})
        root = case.python_root
        pending = harness._invoke_one(
            producer,
            root,
            (
                "send",
                "shared queue prompt",
                "--herdr-bin",
                "<HERDR>",
                "--pane",
                "w1:p1",
                "--queue",
                "<ROOT>/queue",
                "--ready-timeout",
                "0",
            ),
        )
        state = _state(root)
        state["status"] = "idle"
        (root / "state.json").write_text(json.dumps(state, sort_keys=True), encoding="utf-8")
        drained = harness._invoke_one(
            consumer,
            root,
            (
                "drain",
                "--herdr-bin",
                "<HERDR>",
                "--pane",
                "w1:p1",
                "--queue",
                "<ROOT>/queue",
            ),
        )
        binding: object = json.loads((root / "queue/target.json").read_text(encoding="utf-8"))
        report.require(
            f"interop/{label}",
            pending.returncode == 75
            and drained.returncode == 0
            and _state(root).get("submitted") == ["shared queue prompt"]
            and isinstance(binding, dict)
            and binding.get("pane_id") == "w1:p1",
            f"shared queue was not interoperable: pending={pending!r} drained={drained!r}",
        )


def _cross_process_serialization(harness: Harness, report: Report) -> None:
    """Mixed target forms and different TMPDIR values still serialize one resolved pane."""

    case = harness.case("cross-process-lock", {"run_mode": "gate"})
    root = case.python_root

    def launch(command: Sequence[str], arguments: Sequence[str], tmpdir: Path) -> subprocess.Popen[str]:
        tmpdir.mkdir(mode=0o700)
        expanded = [
            value.replace("<ROOT>", str(root)).replace("<HERDR>", str(root / "fake-herdr"))
            for value in arguments
        ]
        environment = dict(os.environ)
        existing = environment.get("PYTHONPATH", "")
        local = str(REPO_ROOT / "py")
        environment["PYTHONPATH"] = local if not existing else local + os.pathsep + existing
        environment.update(
            {"LANG": "C.UTF-8", "LC_ALL": "C.UTF-8", "NO_COLOR": "1", "TMPDIR": str(tmpdir)}
        )
        return subprocess.Popen(
            [*command, *expanded],
            cwd=root,
            env=environment,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
        )

    first = launch(
        harness.python,
        (
            "send",
            "from pane",
            "--herdr-bin",
            "<HERDR>",
            "--pane",
            "w1:p1",
            "--queue",
            "<ROOT>/pane-queue",
        ),
        root / "tmp-a",
    )
    second: subprocess.Popen[str] | None = None
    try:
        deadline = time.monotonic() + 5
        while not (root / "run-entered").exists() and time.monotonic() < deadline:
            time.sleep(0.01)
        second = launch(
            harness.rust,
            (
                "send",
                "from session",
                "--herdr-bin",
                "<HERDR>",
                "--session-agent",
                "codex",
                "--session",
                "session-1",
                "--queue",
                "<ROOT>/session-queue",
            ),
            root / "tmp-b",
        )
        time.sleep(0.25)
        serialized = _state(root).get("submitted") == ["from pane"]
        (root / "run-release").write_text("release\n", encoding="utf-8")
        first_out, first_err = first.communicate(timeout=10)
        second_out, second_err = second.communicate(timeout=10)
        report.require(
            "interop/cross-process-canonical-lock",
            serialized and first.returncode == second.returncode == 0,
            "mixed target forms overlapped: "
            f"state={_state(root)!r} first=({first.returncode},{first_out!r},{first_err!r}) "
            f"second=({second.returncode},{second_out!r},{second_err!r})",
        )
    finally:
        (root / "run-release").touch()
        for process in (first, second):
            if process is not None and process.poll() is None:
                process.kill()
                process.communicate()


def _invalid_cli(harness: Harness, report: Report) -> None:
    case = harness.case("invalid-cli")
    for label, arguments, expected_code in (
        ("unknown-command", ("unknown",), 2),
        ("nan-timeout", ("status", "--ready-timeout", "nan"), 2),
        ("missing-message", ("send", "--herdr-bin", "<HERDR>", "--pane", "w1:p1"), 2),
        ("abbreviated-option", ("status", "--pan", "w1:p1"), 2),
        ("extra-userguide-positionals", ("userguide", "one", "two"), 2),
        ("userguide-invalid-command", ("--userguide", "unknown"), 2),
        ("option-value-stolen-by-help", ("status", "--pane", "--help"), 2),
        ("option-value-stolen-by-version", ("status", "--pane", "--version"), 2),
        ("option-value-stolen-by-option", ("status", "--pane", "--queue", "state"), 2),
        ("attempts-over-bound", ("status", "--max-attempts", "1000001"), 2),
        ("attempts-underscore", ("status", "--max-attempts", "1_0"), 2),
        ("attempts-unicode", ("status", "--max-attempts", "١٢"), 2),
        ("lines-over-bound", ("read", "--lines", "1000001"), 2),
        ("timeout-underscore", ("status", "--ready-timeout", "1_0"), 2),
        ("timeout-unicode", ("status", "--ready-timeout", "١.0"), 2),
        ("empty-exact-pane", ("status", "--pane", ""), 75),
    ):
        python, rust = harness.invoke(case, arguments)
        report.require(
            f"cli/{label}",
            python.returncode == rust.returncode == expected_code
            and "traceback" not in (python.stderr + rust.stderr).lower()
            and "panicked" not in (python.stderr + rust.stderr).lower(),
            f"invalid invocation was not a clean usage error: python={python!r} rust={rust!r}",
        )


def build_report(python_command: Sequence[str], rust_command: Sequence[str]) -> Report:
    """Run the complete black-box differential."""

    report = Report()
    with tempfile.TemporaryDirectory(prefix="herdr-agent-cross-") as temporary:
        harness = Harness(Path(temporary), python_command, rust_command)
        _bootstrap(harness, report)
        _status_and_read(harness, report)
        _successful_send(harness, report)
        _pending_and_ambiguous(harness, report)
        _adversarial_queue(harness, report)
        _shared_queue_interop(harness, report)
        _cross_process_serialization(harness, report)
        _invalid_cli(harness, report)
    return report


def compare_herdr_agent(python_command: Sequence[str], rust_command: Sequence[str]) -> int:
    """Print the paired report and return a conventional status."""

    report = build_report(python_command, rust_command)
    if report.failures:
        for failure in report.failures:
            print(f"DIVERGENCE [{failure}]")
        print(
            f"cross[herdr-agent]: {len(report.failures)} divergence(s) "
            f"out of {report.checks} paired checks"
        )
        return 1
    print(f"cross[herdr-agent]: OK - {report.checks} paired checks agree")
    return 0


def main(argv: Sequence[str] | None = None) -> int:
    """Standalone source-tree entry point used for focused development."""

    del argv
    python = [sys.executable, "-m", "herdr_run.agent_cli"]
    launcher = REPO_ROOT / "rs/bin/herdr-agent"
    environment = dict(os.environ)
    environment["AGENT_UTILS_RS_ENSURE_ONLY"] = "1"
    ensured = subprocess.run(
        [str(launcher)],
        cwd=REPO_ROOT,
        env=environment,
        capture_output=True,
        text=True,
        check=False,
        timeout=300,
    )
    if ensured.returncode != 0 or not ensured.stdout.strip():
        print(f"cannot resolve Rust herdr-agent: {ensured.stderr}", file=sys.stderr)
        return 1
    rust = [ensured.stdout.strip()]
    return compare_herdr_agent(python, rust)


if __name__ == "__main__":
    raise SystemExit(main())
