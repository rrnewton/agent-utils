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
import shlex
import re
import shutil
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
import shlex
import signal
import subprocess
import sys
import time

root = os.path.dirname(os.path.realpath(__file__))
state_path = os.path.join(root, "state.json")
with open(state_path, encoding="utf-8") as handle:
    state = json.load(handle)
args = sys.argv[1:]
state.setdefault("calls", []).append(args)

if args == ["goal-rpc"]:
    for line in sys.stdin:
        request = json.loads(line)
        if "id" not in request:
            continue
        result = {}
        if request.get("method") == "thread/goal/get":
            goals = [text[6:] for text in state.get("submitted", []) if text.startswith("/goal ")]
            result = {"goal": None if not goals else {
                "threadId": request["params"]["threadId"], "objective":goals[-1], "status":"active",
                "tokensUsed":1, "timeUsedSeconds":1, "createdAt":1, "updatedAt":1, "tokenBudget":None,
            }}
        print(json.dumps({"id":request["id"], "result":result}), flush=True)
    raise SystemExit(0)

def save():
    temporary = state_path + ".tmp"
    with open(temporary, "w", encoding="utf-8") as handle:
        json.dump(state, handle, sort_keys=True)
    os.replace(temporary, state_path)

def envelope(result):
    print(json.dumps({"result": result}, sort_keys=True))

if args[:2] == ["pane", "list"]:
    if state.get("offline"):
        print("server unavailable", file=sys.stderr)
        save()
        raise SystemExit(1)
    panes = [] if state.get("closed") or state.get("missing_pane") else [{
        "pane_id": "w1:p1", "tab_id": state.get("tab_id", "w1:t1"),
        "workspace_id": "w1",
    }]
    if state.get("duplicate_recorded_pane"):
        panes.append({"pane_id":"w1:p1", "tab_id":"w1:duplicate", "workspace_id":"w1"})
    if state.get("extra_pane"):
        panes.append({"pane_id":"w1:p2", "tab_id":"w1:t1", "workspace_id":"w1"})
    envelope({"panes": panes})
elif args[:2] == ["pane", "get"]:
    pane = args[2]
    human = pane == "w1:p2" and state.get("extra_pane")
    if pane != "w1:p1" and not human:
        print("missing pane", file=sys.stderr)
        save()
        raise SystemExit(1)
    envelope({"pane": {
        "pane_id": pane,
        "workspace_id": "w1",
        "cwd": root,
        "agent": None if human or state.get("empty_shell") or (state.get("custom_harness") and not state.get("custom_reported")) else state.get("harness", "codex"),
        "agent_status": "unknown" if human or state.get("empty_shell") or (state.get("custom_harness") and not state.get("custom_reported")) else state.get("status", "idle"),
        "agent_session": None if human or state.get("empty_shell") or state.get("sessionless") or state.get("custom_harness") else {"agent": state.get("harness", "codex"), "value": "session-1"},
    }})
elif args[:2] == ["pane", "process-info"]:
    shell_pid = state["fixture_shell_pid"]
    shell_executable = state["fixture_shell_executable"]
    if state.get("empty_shell"):
        process_pid = shell_pid
        command = shell_executable
        parsed = [shell_executable]
        foreground_processes = [
            {"pid":process_pid, "name":os.path.basename(shell_executable),
             "cmdline":command, "argv":parsed, "executable":shell_executable}
        ]
        foreground_process_group_id = (
            process_pid + 1 if state.get("idle_shell_busy") else process_pid
        )
    else:
        command = ("/tmp/lookalike/muse" if state.get("wrong_custom_process")
                   else state.get("launch_command", "/usr/local/bin/muse"))
        parsed = shlex.split(command)
        executable = parsed[0]
        process_pid = state.get("custom_pid", 200)
        foreground_processes = [] if state.get("custom_identity_hidden") else [
            {"pid":process_pid, "name":os.path.basename(executable),
             "cmdline":command, "argv":parsed, "executable":executable}
        ]
        foreground_process_group_id = (
            process_pid + 1 if state.get("wrong_custom_process_group")
            else process_pid if state.get("custom_pid") else 200
        )
    envelope({"process_info": {
        "pane_id":"w1:p1", "shell_pid":shell_pid,
        "foreground_process_group_id":foreground_process_group_id,
        "foreground_processes":foreground_processes,
    }})
elif args[:2] == ["tab", "create"]:
    state["closed"] = False
    state["tab_environment"] = [
        args[index + 1] for index, argument in enumerate(args[:-1])
        if argument == "--env"
    ]
    envelope({"tab": {"tab_id": "w1:t1"}, "root_pane": {
        "pane_id":"w1:p1", "tab_id":"w1:t1", "workspace_id":"w1",
    }})
elif args[:2] == ["pane", "close"]:
    if args[2] != "w1:p1":
        raise SystemExit("refusing unexpected pane close")
    if state.get("custom_pid"):
        try:
            os.kill(state["custom_pid"], signal.SIGTERM)
        except ProcessLookupError:
            pass
    state["closed"] = True
    state.setdefault("closed_panes", []).append(args[2])
elif args[:2] in (["agent", "focus"], ["tab", "focus"]):
    state["focused"] = args[2]
elif args[:2] == ["agent", "start"]:
    state["name"] = args[2]
    state["harness"] = args[args.index("--kind") + 1]
    state["launch_arguments"] = args[args.index("--") + 1:]
    if state.get("start_failure"):
        print("startup requires attention", file=sys.stderr)
        save()
        raise SystemExit(1)
elif args[:2] == ["pane", "run"]:
    state["launch_command"] = args[3]
    parsed = shlex.split(args[3])
    state["launch_arguments"] = parsed[1:]
    state["custom_harness"] = True
    child_command = (["/bin/sleep", "60"] if state.get("wrong_custom_process_identity")
                     else [parsed[0], "-c", "import time; time.sleep(60)"])
    child = subprocess.Popen(
        child_command, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL, start_new_session=True,
    )
    state["custom_pid"] = child.pid
elif args[:2] == ["pane", "report-agent"]:
    state["harness"] = args[args.index("--agent") + 1]
    state["status"] = args[args.index("--state") + 1]
    state["custom_reported"] = True
elif args[:2] == ["agent", "get"]:
    if state.get("offline") or state.get("name") != args[2]:
        print("named agent unavailable", file=sys.stderr)
        save()
        raise SystemExit(1)
    envelope({"agent": {"name":args[2], "pane_id":"w1:p1"}})
elif args[:2] == ["pane", "report-agent-session"]:
    state["reported_session"] = args[args.index("--agent-session-id") + 1]
elif args[:2] == ["workspace", "get"]:
    envelope({"workspace": {
        "workspace_id": state.get("workspace_response_id", "w1"),
        "label": "project",
    }})
elif args[:2] == ["agent", "prompt"]:
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
elif args[:2] == ["pane", "send-text"]:
    pasted = args[3]
    start, end = "\x1b[200~", "\x1b[201~"
    state["paste_wrapped"] = pasted.startswith(start) and pasted.endswith(end)
    state["custom_draft"] = pasted[len(start):-len(end)] if state["paste_wrapped"] else pasted
    state["custom_submitted"] = False
    if state.get("custom_exit_after_send_text") and state.get("custom_pid"):
        state["retired_custom_pid"] = state["custom_pid"]
        try:
            os.kill(state["custom_pid"], signal.SIGTERM)
        except ProcessLookupError:
            pass
        state["custom_identity_hidden"] = True
elif args[:2] == ["agent", "wait"]:
    if state.get("goal_menu") and not state.get("confirmed_goal"):
        print("replacement menu requires confirmation", file=sys.stderr)
        save()
        raise SystemExit(1)
    if state.get("wait_mode") == "fail":
        print("working transition unavailable", file=sys.stderr)
        save()
        raise SystemExit(1)
    print(json.dumps({"result": {"agent": {"pane_id": args[2], "agent_status": "working"}}}, sort_keys=True))
elif args[:2] == ["pane", "read"]:
    if state.get("extra_pane_on_read"):
        state["extra_pane"] = True
    if state.get("restart_agent_on_read"):
        state["empty_shell"] = False
    if state.get("leave_idle_shell_on_read"):
        state["idle_shell_busy"] = True
    source = args[args.index("--source") + 1]
    if source == "visible" and state.get("screen"):
        sys.stdout.write(state["screen"])
    elif source == "visible" and state.get("custom_harness"):
        prefix = "Muse Code 1.3.0\n\nreasoning effort ultra is not available (gate ultra_reasoning_effort is closed); using xhigh\n"
        divider = "────────────────\n"
        history = "".join("❯ " + text + "\n◆ accepted\n" for text in state.get("submitted", []))
        footer = "watermelon-preview · xhigh · project · Auto-review\n"
        if state.get("custom_submitted"):
            if state.get("custom_post_error"):
                sys.stdout.write(prefix + history + divider + "❯ " + state["custom_draft"] +
                                 "\nError: retry\n" + divider + footer)
            else:
                sys.stdout.write(prefix + history + "Working...\n" + divider + "❯\n" +
                                 divider + footer)
        elif state.get("custom_draft"):
            sys.stdout.write(prefix + history + divider + "❯ " + state["custom_draft"] +
                             "\n" + divider + footer)
        else:
            sys.stdout.write(prefix + history + divider + "❯\n" + divider + footer)
    elif source == "recent-unwrapped":
        sys.stdout.write(state.get("read_unwrapped", "agent transcript\n"))
    else:
        sys.stdout.write(state.get("read_recent", "fallback transcript\n"))
elif args[:2] == ["pane", "send-keys"]:
    if state.get("custom_harness") and state.get("custom_draft") and args[3] == "Enter":
        state["custom_submitted"] = True
        if not state.get("custom_clear_without_submit"):
            state.setdefault("submitted", []).append(state["custom_draft"])
    else:
        state["confirmed_goal"] = args[3] == "Enter"
else:
    print("unsupported fake Herdr call: " + repr(args), file=sys.stderr)
    save()
    raise SystemExit(2)
save()
'''

_FAKE_MUSE_SKILLS = r'''import json, os, pathlib, shutil, sys
args = sys.argv[1:]
if args[:2] != ["skills", "install"] or len(args) < 8:
    print("unsupported fake Muse call", file=sys.stderr)
    raise SystemExit(2)
source = pathlib.Path(args[2])
if args[3:5] != ["--scope", "user"] or args[5] != "--name" or args[7] != "--json":
    print("invalid fake Muse install shape", file=sys.stderr)
    raise SystemExit(2)
name = args[6]
if name != "agentctl" or any(value not in ("--force",) for value in args[8:]):
    print("invalid fake Muse install options", file=sys.stderr)
    raise SystemExit(2)
config = os.environ.get("XDG_CONFIG_HOME")
if not config or not os.path.isabs(config):
    print("missing isolated XDG_CONFIG_HOME", file=sys.stderr)
    raise SystemExit(2)
destination = pathlib.Path(config) / "muse" / "skills" / name / "SKILL.md"
if destination.exists() and "--force" not in args:
    print("skill-already-installed", file=sys.stderr)
    raise SystemExit(1)
destination.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
shutil.copyfile(source / "SKILL.md", destination)
destination.chmod(0o600)
log = pathlib.Path(__file__).with_name("muse-skill-calls.jsonl")
with log.open("a", encoding="utf-8") as stream:
    stream.write(json.dumps(args) + "\n")
print(json.dumps({"installed": {"id": name}}))
'''


class Harness:
    """Create paired fixtures and invoke both implementations."""

    def __init__(self, root: Path, python: Sequence[str], rust: Sequence[str]) -> None:
        self.root = root
        self.python = tuple(python)
        self.rust = tuple(rust)
        self.serial = 0
        shell = os.path.realpath("/bin/bash")
        self.fixture_shell = subprocess.Popen(
            [shell, "--noprofile", "--norc"],
            stdin=subprocess.PIPE,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
        self.replacement_shell = subprocess.Popen(
            [shell, "--noprofile", "--norc"],
            stdin=subprocess.PIPE,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
        self.fixture_shell_executable = shell

    def close(self) -> None:
        """Reap only the two shell generations created by this differential."""
        for process in (self.fixture_shell, self.replacement_shell):
            if process.stdin is not None:
                process.stdin.close()
            if process.poll() is None:
                process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)

    def case(self, label: str, state: Mapping[str, object] | None = None) -> PairCase:
        self.serial += 1
        safe = re.sub(r"[^A-Za-z0-9_.-]+", "-", label).strip("-") or "case"
        base = self.root / f"{self.serial:03d}-{safe}"
        python_root = base / "python" / "project"
        rust_root = base / "rust" / "project"
        initial: dict[str, object] = {
            "status": "idle",
            "calls": [],
            "submitted": [],
            "fixture_shell_pid": self.fixture_shell.pid,
            "fixture_shell_executable": self.fixture_shell_executable,
        }
        if state is not None:
            initial.update(state)
        for root in (python_root, rust_root):
            root.mkdir(parents=True, mode=0o700)
            fixture = root / "fake-herdr"
            fixture.write_text(f"#!{sys.executable}\n{_FAKE_HERDR}", encoding="utf-8")
            fixture.chmod(0o700)
            fake_muse = root / "fake-muse-skills"
            fake_muse.write_text(
                f"#!{sys.executable}\n{_FAKE_MUSE_SKILLS}", encoding="utf-8"
            )
            fake_muse.chmod(0o700)
            fake_muse_runtime = root / "fake-muse-runtime"
            shutil.copyfile(sys.executable, fake_muse_runtime)
            fake_muse_runtime.chmod(0o700)
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
        environment.update({"LANG": "C.UTF-8", "LC_ALL": "C.UTF-8", "NO_COLOR": "1", "HERDR_WORKSPACE_ID": "w1"})
        for harness_name in ("CODEX", "CLAUDE"):
            environment[f"AGENTCTL_{harness_name}_SKILLS_DIR"] = str(
                root / "skill-homes" / harness_name.lower()
            )
        fixture_state = _state(root)
        if fixture_state.get("hostile_path"):
            environment["PATH"] = str(root / "hostile-bin")
        environment["AGENTCTL_MUSE_BIN"] = (
            str(root / "missing-muse")
            if fixture_state.get("missing_custom_executable")
            else str(root / "fake-muse-skills")
            if tuple(arguments[:2]) == ("skill", "install")
            else str(root / "fake-muse-runtime")
        )
        environment["XDG_CONFIG_HOME"] = str(root / "xdg-config")
        empty_environment = fixture_state.get("empty_environment", [])
        if not isinstance(empty_environment, list) or any(
            not isinstance(variable, str) for variable in empty_environment
        ):
            raise TypeError("empty_environment fixture must contain strings")
        for variable in empty_environment:
            environment[variable] = ""
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
    ):
        python, rust = harness.invoke(case, arguments)
        report.require(
            f"cli/{label}",
            python.returncode == rust.returncode == expected_code
            and "traceback" not in (python.stderr + rust.stderr).lower()
            and "panicked" not in (python.stderr + rust.stderr).lower(),
            f"invalid invocation was not a clean usage error: python={python!r} rust={rust!r}",
        )

    python, rust = harness.invoke(case, ("status", "--pane", ""))
    expected = "target needs --pane or a stable session value"
    report.require(
        "cli/empty-exact-pane",
        python.returncode == rust.returncode == 75
        and python.stdout == rust.stdout == ""
        and expected in python.stderr
        and expected in rust.stderr,
        "empty exact pane did not fail target validation before Herdr resolution: "
        f"python={python!r} rust={rust!r}",
    )


def _managed_lifecycle(harness: Harness, report: Report) -> None:
    """Exercise the same named lifecycle and cross-edition durable registry format."""
    def normalized(outcome: Outcome) -> object:
        if outcome.returncode != 0:
            return outcome
        value: object = json.loads(outcome.stdout)
        def clean(item: object) -> object:
            if isinstance(item, list):
                return [clean(entry) for entry in item]
            if isinstance(item, dict):
                return {
                    str(key): ("<TOKEN>" if key == "token" else 0 if key == "created_at"
                               else "<ARCHIVE>" if key == "archive" else clean(child))
                    for key, child in item.items()
                }
            return item
        return clean(value)

    for kind in ("codex", "claude"):
        case = harness.case(f"managed-{kind}")
        common: tuple[str, ...] = ("--herdr-bin", "<HERDR>", "--registry", "<ROOT>/registry", "--goal-command-json", '["<HERDR>","goal-rpc"]')
        start = ("start", "worker", "--cwd", "<ROOT>", "--workspace-id", "w1",
                 "--harness", kind, "--model", "selected-model", "--harness-arg=--extra", *common)
        python, rust = harness.invoke(case, start)
        report.require(f"managed/{kind}/start", python.returncode == rust.returncode == 0 and normalized(python) == normalized(rust),
                       f"start diverged: {python!r} {rust!r}")
        if python.returncode or rust.returncode:
            continue
        for command in (("list",), ("status", "--name", "worker"), ("bind-session", "session-1", "--name", "worker"), ("wait", "--name", "worker", "--ready-timeout", "0"),
                        ("goal", "finish the task", "--name", "worker"), ("goal", "--name", "worker"),
                        ("send", "literal\nmessage", "--name", "worker")):
            python, rust = harness.invoke(case, (*command, *common))
            report.require(f"managed/{kind}/{command[0]}", python.returncode == rust.returncode == 0 and normalized(python) == normalized(rust),
                           f"managed command diverged: {python!r} {rust!r}")
        python, rust = harness.invoke(case, ("read", "--name", "worker", *common))
        report.exact(f"managed/{kind}/read", python, rust, 0)
        python, rust = harness.invoke(case, ("stop", "worker", *common))
        report.require(f"managed/{kind}/stop", python.returncode == rust.returncode == 0 and normalized(python) == normalized(rust),
                       f"stop diverged: {python!r} {rust!r}")
        report.require(f"managed/{kind}/literal-launch", _state(case.python_root).get("launch_arguments") == _state(case.rust_root).get("launch_arguments"),
                       "harness arguments differed")

    compatibility_cases = (
        (
            "claude-unstructured-effort",
            ("--harness", "claude", "--harness-arg=--effort=high"),
            ["--effort=high"],
        ),
        (
            "codex-nonoverriding-config",
            (
                "--harness", "codex", "--model", "selected-model",
                "--harness-arg=-c", "--harness-arg=sandbox_mode=read-only",
            ),
            [
                "--no-alt-screen", "--model", "selected-model",
                "-c", "sandbox_mode=read-only",
            ],
        ),
    )
    for label, launch_policy, expected_arguments in compatibility_cases:
        case = harness.case(f"managed-compatibility-{label}")
        common = (
            "--herdr-bin", "<HERDR>", "--registry", "<ROOT>/registry",
            "--goal-command-json", '["<HERDR>","goal-rpc"]',
        )
        outcomes = harness.invoke(case, (
            "start", "worker", "--cwd", "<ROOT>", "--workspace-id", "w1",
            *launch_policy, *common,
        ))
        report.require(
            f"managed/compatibility/{label}",
            all(outcome.returncode == 0 for outcome in outcomes)
            and all(
                _state(root).get("launch_arguments") == expected_arguments
                for root in (case.python_root, case.rust_root)
            ),
            f"legacy raw launch policy regressed: {outcomes!r}",
        )

    profile_selectors = (
        ("split-config-profile", ("--harness-arg=-c", "--harness-arg=profile=attacker")),
        ("attached-config-profile", ("--harness-arg=--config=profile=attacker",)),
        ("quoted-config-profile", ('--harness-arg=-c"profile"="attacker"',)),
    )
    for label, raw_arguments in profile_selectors:
        case = harness.case(f"managed-profile-selector-refusal-{label}")
        common = ("--herdr-bin", "<HERDR>", "--registry", "<ROOT>/registry")
        outcomes = harness.invoke(case, (
            "start", "worker", "--cwd", "<ROOT>", "--workspace-id", "w1",
            "--harness", "codex", "--model", "structured",
            *raw_arguments, *common,
        ))
        report.require(
            f"managed/profile-selector-refusal/{label}",
            all(
                outcome.returncode == 75 and "raw Codex profile" in outcome.stderr
                for outcome in outcomes
            )
            and all(
                not (root / "registry").exists()
                for root in (case.python_root, case.rust_root)
            ),
            f"indirect Codex profile selector was accepted or allocated state: {outcomes!r}",
        )

    # A record written by Python can be read, messaged and archived by Rust, and vice versa.
    for label, producer, consumer in (("python-rust", harness.python, harness.rust), ("rust-python", harness.rust, harness.python)):
        case = harness.case(f"managed-interop-{label}")
        root = case.python_root
        common = ("--herdr-bin", "<HERDR>", "--registry", "<ROOT>/registry")
        started = harness._invoke_one(producer, root, ("start", "worker", "--cwd", "<ROOT>", "--workspace-id", "w1", *common))
        sent = harness._invoke_one(consumer, root, ("send", "shared registry", "--name", "worker", *common))
        stopped = harness._invoke_one(consumer, root, ("stop", "worker", *common))
        report.require(f"managed/interop/{label}", started.returncode == sent.returncode == stopped.returncode == 0 and _state(root).get("submitted") == ["shared registry"],
                       f"registry interop failed: {started!r} {sent!r} {stopped!r}")

    for label, change in (("offline", {"offline": True}), ("busy", {"status": "working"}), ("extra-pane", {"extra_pane": True})):
        case = harness.case(f"managed-{label}")
        common = ("--herdr-bin", "<HERDR>", "--registry", "<ROOT>/registry")
        harness.invoke(case, ("start", "worker", "--cwd", "<ROOT>", "--workspace-id", "w1", *common))
        for root in (case.python_root, case.rust_root):
            state = _state(root)
            state.update(change)
            (root / "state.json").write_text(json.dumps(state), encoding="utf-8")
        if label == "extra-pane":
            python, rust = harness.invoke(case, ("stop", "worker", *common))
            report.require("managed/stop-extra-pane", python.returncode == rust.returncode == 75
                           and "ownership changed" in python.stderr and "ownership changed" in rust.stderr,
                           f"unsafe tab close was accepted: {python!r} {rust!r}")
        else:
            python, rust = harness.invoke(case, ("send", "retain this prompt", "--name", "worker", "--ready-timeout", "0", *common))
            report.require(f"managed/{label}-pending", python.returncode == rust.returncode == 75
                           and '"outcome": "pending"' in python.stdout and '"outcome": "pending"' in rust.stdout,
                           f"prompt was not durably retained: {python!r} {rust!r}")
            report.require(f"managed/{label}-durable-state", _queue_snapshot(case.python_root, "registry/worker/queue") == _queue_snapshot(case.rust_root, "registry/worker/queue"),
                           "pending managed queue artifacts differed")
        report.require(f"managed/{label}-no-mutation", _state(case.python_root).get("submitted") == []
                       and _state(case.rust_root).get("submitted") == []
                       and not _state(case.python_root).get("closed") and not _state(case.rust_root).get("closed"),
                       "failure injected a prompt or closed a tab")

    case = harness.case("managed-failed-launch", {"start_failure": True})
    common = ("--herdr-bin", "<HERDR>", "--registry", "<ROOT>/registry")
    python, rust = harness.invoke(case, ("start", "worker", "--cwd", "<ROOT>", "--workspace-id", "w1", *common))
    report.require("managed/failed-launch-retained", python.returncode == rust.returncode == 75
                   and (case.python_root / "registry/worker/agent.json").exists()
                   and (case.rust_root / "registry/worker/agent.json").exists()
                   and not _state(case.python_root).get("closed") and not _state(case.rust_root).get("closed"),
                   f"launch failed without inspectable state: {python!r} {rust!r}")
    for root in (case.python_root, case.rust_root):
        state = _state(root)
        state["name"] = "replacement"
        (root / "state.json").write_text(json.dumps(state), encoding="utf-8")
    python, rust = harness.invoke(case, ("stop", "worker", *common))
    report.require("managed/failed-launch-replacement-preserved", python.returncode == rust.returncode == 69
                   and not _state(case.python_root).get("closed") and not _state(case.rust_root).get("closed"),
                   f"cleanup targeted replacement harness: {python!r} {rust!r}")

    for correct in (True, False):
        objective = "finish this task" if correct else "different objective"
        screen = f"Replace goal?\nNew objective: {objective}\n› 1. Replace current goal  Set the new objective and start it now\n2. Cancel  Keep the current goal\nPress enter to confirm or esc to go back"
        case = harness.case(f"goal-menu-{correct}", {"goal_menu": True, "screen": screen})
        common = ("--herdr-bin", "<HERDR>", "--registry", "<ROOT>/registry", "--goal-command-json", '["<HERDR>","goal-rpc"]')
        harness.invoke(case, ("start", "worker", "--cwd", "<ROOT>", "--workspace-id", "w1", *common))
        python, rust = harness.invoke(case, ("goal", "finish this task", "--name", "worker", *common))
        expected = 0 if correct else 76
        report.require(f"managed/goal-replacement-{correct}", python.returncode == rust.returncode == expected
                       and bool(_state(case.python_root).get("confirmed_goal")) == correct
                       and bool(_state(case.rust_root).get("confirmed_goal")) == correct,
                       f"goal menu handling diverged: {python!r} {rust!r}")

    for label, producer, consumer in (("python-rust", harness.python, harness.rust), ("rust-python", harness.rust, harness.python)):
        case = harness.case(f"queued-goal-interop-{label}")
        root = case.python_root
        common = ("--herdr-bin", "<HERDR>", "--registry", "<ROOT>/registry", "--goal-command-json", '["<HERDR>","goal-rpc"]')
        started = harness._invoke_one(producer, root, ("start", "worker", "--cwd", "<ROOT>", "--workspace-id", "w1", *common))
        state = _state(root)
        state["status"] = "working"
        (root / "state.json").write_text(json.dumps(state), encoding="utf-8")
        queued = harness._invoke_one(producer, root, ("goal", "finish this task", "--name", "worker", "--ready-timeout", "0", *common))
        state = _state(root)
        state.update({"status": "idle", "goal_menu": True,
                      "screen": "Replace goal?\nNew objective: finish this task\n› 1. Replace current goal  Set the new objective and start it now\n2. Cancel  Keep the current goal\nPress enter to confirm or esc to go back"})
        (root / "state.json").write_text(json.dumps(state), encoding="utf-8")
        drained = harness._invoke_one(consumer, root, ("drain", "--name", "worker", *common))
        goal = harness._invoke_one(consumer, root, ("goal", "--name", "worker", *common))
        report.require(f"managed/queued-goal/{label}", started.returncode == drained.returncode == goal.returncode == 0
                       and queued.returncode == 75 and bool(_state(root).get("confirmed_goal"))
                       and _state(root).get("submitted") == ["/goal finish this task"]
                       and json.loads(goal.stdout).get("delivery") == "delivered",
                       f"queued goal lost durable operation identity: {started!r} {queued!r} {drained!r} {goal!r}")
        state = _state(root)
        state["confirmed_goal"] = False
        (root / "state.json").write_text(json.dumps(state), encoding="utf-8")
        raw = harness._invoke_one(consumer, root, ("send", "/goal finish this task", "--name", "worker", *common))
        report.require(f"managed/raw-goal-not-authorized/{label}", raw.returncode == 76 and not _state(root).get("confirmed_goal"),
                       f"ordinary text incorrectly inherited goal confirmation: {raw!r}")


def build_report(python_command: Sequence[str], rust_command: Sequence[str]) -> Report:
    """Run the complete black-box differential."""

    report = Report()
    with tempfile.TemporaryDirectory(prefix="herdr-agent-cross-") as temporary:
        harness = Harness(Path(temporary), python_command, rust_command)
        try:
            _bootstrap(harness, report)
            _status_and_read(harness, report)
            _successful_send(harness, report)
            _pending_and_ambiguous(harness, report)
            _adversarial_queue(harness, report)
            _shared_queue_interop(harness, report)
            _cross_process_serialization(harness, report)
            _invalid_cli(harness, report)
            _managed_lifecycle(harness, report)
        finally:
            harness.close()
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
    python = [sys.executable, "-m", "agentctl.legacy_cli"]
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
