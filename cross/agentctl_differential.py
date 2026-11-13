#!/usr/bin/env python3
"""Black-box comparisons of the shared interactive :command:`agentctl` contract.

Each edition runs against its own executable Herdr fixture. Comparisons include
durable identities, queue transitions, human handoff, native goal observation,
and refusal to close an unrelated pane. Optional headless and service adapters
are advertised separately and are not treated as shared capabilities.
"""
from __future__ import annotations

import json
import fcntl
import hashlib
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
from typing import cast
from collections.abc import Mapping, Sequence
from pathlib import Path

from herdr_agent_differential import (
    FIXTURE_HERDR, REPO_ROOT, Harness, Outcome, PairCase, Report, _normalize as _normalize_outcome,
    _fixture_identity, _signal_cleaned_tempdir,
    _option_values, _queue_snapshot, _revive_interop, _state, _submitted,
    SENDER_HEADER,
    _stop_recovery_arguments,
    guarded_path, init_case_repository, with_case_git, without_ambient_executables,
    TIMEOUT_SECONDS, _tree_cpu_seconds, run_tolerant,
)

_COMMON = ("--herdr-bin", "<HERDR>", "--registry", "<ROOT>/registry")
_GOAL_COMMAND = ("--goal-command-json", '["<HERDR>","goal-rpc"]')
_CAPABILITIES = ["send", "status", "read", "wait", "stop", "attach", "pause", "resume",
                 "terminal-snapshot", "drain", "goal", "bind-session"]
_OWNED_CAPABILITIES = [*_CAPABILITIES, "move", "anchor", "rename", "revive"]
_ADOPTED_CAPABILITIES = [*_CAPABILITIES, "anchor", "rename"]
_SHELL_IDENTITY_FIELDS = {
    "version", "boot_id", "pid", "starttime_ticks",
    "executable_device", "executable_inode",
}


def _generated_conversations(value: object) -> set[str]:
    """Only UUIDs selected by the recorded Claude launch, never process boot IDs."""
    if isinstance(value, list):
        return set().union(*(_generated_conversations(item) for item in value))
    if isinstance(value, dict):
        native, arguments = value.get("native_session"), value.get("arguments")
        if (isinstance(native, dict) and native.get("agent") == "claude"
                and isinstance(arguments, list) and len(arguments) >= 2
                and arguments[0] == "--session-id" and arguments[1] == native.get("value")
                and isinstance(arguments[1], str)):
            return {arguments[1]}
    return set()


def _normalize(value: object, conversations: set[str] | None = None) -> object:
    if conversations is None:
        conversations = _generated_conversations(value)
    if isinstance(value, str) and value in conversations:
        return "<CONVERSATION_UUID>"
    if isinstance(value, list):
        return [_normalize(item, conversations) for item in value]
    if isinstance(value, dict):
        return {str(key): ("<TOKEN>" if key == "token" else 0 if key in ("created_at", "captured_at")
                          else "<PROCESS_IDENTITY>" if key == "custom_process_identity"
                          # Each edition's fixture runs its own harness process: only its
                          # pid and start time differ, the rest of the identity is compared.
                          else {**item, "pid": "<PID>", "starttime_ticks": "<TICKS>",
                                **({"executable_inode": "<IMAGE_INODE>"}
                                   if value.get("harness") == "muse"
                                   and value.get("adapter") == "herdr-foreign" else {})}
                          if key == "harness_identity" and isinstance(item, dict)
                          else "<HARNESS_PID>" if key == "harness_pid"
                          else "<JOURNAL_ID>" if key == "journal_id"
                          else 0 if key == "renamed_at"
                          else "<ARCHIVE>" if key == "archive"
                          # Native identity diagnostics use edition-specific quote styles.
                          else re.sub(r'"([a-z][a-z0-9-]*)"', r"'\1'", item)
                          if key == "probe_error" and isinstance(item, str) else _normalize(item, conversations))
                for key, item in value.items()}
    return value


def _valid_foreign_shell_identity(value: object) -> bool:
    """Require the complete cross-edition six-field shell identity schema."""
    if not isinstance(value, dict) or set(value) != _SHELL_IDENTITY_FIELDS:
        return False
    boot_id = value.get("boot_id")
    positive_fields = (
        value.get("pid"), value.get("starttime_ticks"),
        value.get("executable_device"), value.get("executable_inode"),
    )
    return (
        value.get("version") == 1
        and isinstance(boot_id, str)
        and re.fullmatch(
            r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}",
            boot_id,
        ) is not None
        and all(isinstance(item, int) and not isinstance(item, bool) and item > 0
                for item in positive_fields)
    )


def _queue_quiet(root: Path) -> bool:
    """Whether no drain holds this case's worker queue.

    Quiescence is probed from the filesystem: nothing waits in the queue
    inbox or in flight, and the queue delivery lock can be taken without
    blocking. Never a wall-clock judgement.
    """
    queue = root / "registry/agents/worker/queue"
    for stage in ("inbox", "inflight"):
        stage_dir = queue / stage
        if stage_dir.is_dir() and any(stage_dir.iterdir()):
            return False
    delivery_lock = queue / ".delivery.lock"
    if delivery_lock.exists():
        try:
            fd = os.open(delivery_lock, os.O_RDWR)
        except OSError:
            return False
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            return False
        finally:
            os.close(fd)
    return True


def _wait_for_queue_quiescence(case: PairCase, report: Report, label: str) -> None:
    """Wait until both editions' queues are quiet (an event, not a sleep).

    A repeated send issued while the first send's drain still holds the
    queue lock can exhaust the 600 s send deadline and split the editions
    (rc 75 pending vs rc 76 refused) under load; the drain's lock release
    is the event this waits for, bounded by the shared hang guard.
    """
    deadline = time.monotonic() + TIMEOUT_SECONDS
    while time.monotonic() < deadline:
        if all(_queue_quiet(root) for root in (case.python_root, case.rust_root)):
            return
        time.sleep(0.05)
    report.require(label, False, "queue never became quiescent before the repeated send")


def _json(outcome: Outcome) -> object:
    try:
        return _normalize(json.loads(outcome.stdout))
    except (TypeError, ValueError):
        return outcome.stdout


def _with_effort(arguments: Sequence[str]) -> tuple[str, ...]:
    """Declare an effort on a success-expecting `start` (#1216 parity).

    Owner rule 319: effort is always declared, and both editions now
    refuse a start with no profile or --effort effort. Fixture starts
    that expect to launch therefore declare a level every harness
    accepts; refusal-expecting and parse-only starts keep their literal
    arguments (they never reach this helper's callers unchanged).
    """
    args = tuple(arguments)
    if "start" not in args or "--profile" in args:
        return args
    if "--effort" in args or "--reasoning-effort" in args:
        return args
    if "--harness" in args:
        # A Herdr-native kind takes no reasoning effort; declaring one is a
        # refusal in both editions, not a launch.
        harness = args[args.index("--harness") + 1]
        if harness not in ("codex", "claude", "muse"):
            return args
    at = args.index("start")
    return (*args[: at + 2], "--effort", "medium", *args[at + 2:])


def _drift_only_difference(python: Outcome, rust: Outcome) -> bool:
    """Whether two status-shaped outcomes differ only by profile_drift presence.

    Drift is computed from a profile read that shells out to
    `git check-ignore`; under load either edition can fail that read and
    omit the field, so its presence in one snapshot is a timing artifact.
    When both editions report drift, its value is still compared.
    """
    left, right = _json(python), _json(rust)
    if not isinstance(left, dict) or not isinstance(right, dict):
        return False
    if ("profile_drift" in left) == ("profile_drift" in right):
        return False
    left = {key: value for key, value in left.items() if key != "profile_drift"}
    right = {key: value for key, value in right.items() if key != "profile_drift"}
    return left == right


def _pair(harness: Harness, report: Report, case: PairCase, label: str,
          arguments: Sequence[str], expected: int = 0) -> tuple[Outcome, Outcome]:
    python, rust = harness.invoke(case, _with_effort(arguments))
    if (arguments and arguments[0] == "status"
            and python.returncode == rust.returncode == expected
            and _drift_only_difference(python, rust)):
        # A read-only read: wait (as an event, bounded by the hang guard)
        # until both editions could evaluate the profile, then compare.
        deadline = time.monotonic() + TIMEOUT_SECONDS
        while time.monotonic() < deadline and _drift_only_difference(python, rust):
            time.sleep(0.05)
            python, rust = harness.invoke(case, _with_effort(arguments))
    agree = _json(python) == _json(rust) or _drift_only_difference(python, rust)
    report.require(label, python.returncode == rust.returncode == expected
                   and agree
                   and (python.stderr == rust.stderr if expected == 0 else True),
                   f"expected rc {expected}; python={python!r} rust={rust!r}")
    if arguments and arguments[0] == "stop" and expected != 0:
        report.require(label + "/recovery-command",
                       all(_stop_recovery_arguments(outcome) is not None for outcome in (python, rust)),
                       f"stop refusal omitted its reason or copyable command: {python!r} {rust!r}")
    return python, rust


def _change(case: PairCase, values: Mapping[str, object]) -> None:
    for root in (case.python_root, case.rust_root):
        state = _state(root)
        state.update(values)
        (root / "state.json").write_text(json.dumps(state), encoding="utf-8")


def _submission_count(root: Path, text: str) -> int:
    values = _submitted(root)
    return values.count(text) if isinstance(values, list) else 0


def _fixture_calls(root: Path) -> list[list[str]]:
    """Read the protocol calls recorded by one executable fixture."""
    values = _state(root).get("calls", [])
    if not isinstance(values, list) or any(
        not isinstance(call, list) or any(not isinstance(word, str) for word in call)
        for call in values
    ):
        raise TypeError("fixture calls must be argv arrays")
    return cast(list[list[str]], values)


def _fixture_process_alive(process_id: object) -> bool:
    """Whether a fixture's stand-in process `process_id` still runs (a zombie does not)."""
    if not isinstance(process_id, int):
        return False
    try:
        process_stat = Path(f"/proc/{process_id}/stat").read_text(encoding="utf-8")
    except (FileNotFoundError, ProcessLookupError):
        return False
    close = process_stat.rfind(")")
    return close >= 0 and process_stat[close + 2:].split(maxsplit=1)[0] != "Z"


def _retire_fixture_processes(case: PairCase, report: Report | None = None) -> None:
    """Stop only custom children created by this differential's private Herdr fixtures.

    Exit is awaited, bounded by CPU progress (#1067
    cross-differential-load-aware): pane close must make these fixtures
    exit, so success is the observed exit and the bounds only decide
    failure. The old 2 s budget is now spent as CPU - a fixture still
    burning it is progressing, and load delays its exit slice, not the
    verdict - while a fixture that burns the whole CPU budget without
    exiting, or is still alive at the failure deadline, is stuck, not
    starved. Callers with a `Report` get a labelled check. The
    report-less form stays for `agentctl_nested_records`, which imports
    this helper and is outside this change's files; there a stuck
    fixture still raises rather than passing silently.
    """
    process_ids: list[int] = []
    for root in (case.python_root, case.rust_root):
        state = _state(root)
        for field in ("custom_pid", "retired_custom_pid", "harness_pid"):
            process_id = state.get(field)
            if isinstance(process_id, int) and process_id not in process_ids:
                process_ids.append(process_id)
        subprocess.run(
            [str(root / "fake-herdr"), "pane", "close", _fixture_identity(root)[0]],
            check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
    remaining = list(process_ids)
    deadline = time.monotonic() + TIMEOUT_SECONDS
    burned_cpu = 0.0
    last_cpu: float | None = None
    while remaining and time.monotonic() < deadline:
        remaining = [process_id for process_id in remaining
                     if _fixture_process_alive(process_id)]
        if not remaining:
            break
        cpu_now = 0.0
        sampled = False
        for process_id in remaining:
            measured = _tree_cpu_seconds(process_id)
            if measured is not None:
                cpu_now += measured
                sampled = True
        if sampled:
            if last_cpu is not None and cpu_now > last_cpu:
                burned_cpu += cpu_now - last_cpu
            last_cpu = cpu_now
            if burned_cpu > 2.0:
                break
        time.sleep(0.05)
    remaining = [process_id for process_id in remaining
                 if _fixture_process_alive(process_id)]
    if report is not None:
        if process_ids:
            report.require(
                f"retire/{case.python_root.parent.parent.name}",
                not remaining,
                f"fixture custom processes did not exit: {remaining!r}",
            )
    elif remaining:
        raise AssertionError(f"fixture custom processes did not exit: {remaining!r}")


def _start(harness: Harness, report: Report, case: PairCase, label: str,
           *options: str) -> bool:
    outcomes = _pair(harness, report, case, label,
                     ("start", "worker", "--cwd", "<ROOT>", "--workspace-id", "w1", *options, *_COMMON))
    return all(outcome.returncode == 0 for outcome in outcomes)


def _orientation(harness: Harness, report: Report) -> None:
    case = harness.case("primary-orientation")
    _pair(harness, report, case, "primary/version", ("--version",))
    for arguments in ((), ("--help",), ("start", "--help"), ("adopt", "--help"),
                      ("stop", "--help"), ("send", "--help"), ("goal", "--help")):
        for edition, outcome in zip(("python", "rust"), harness.invoke(case, arguments), strict=True):
            report.require(f"primary/help/{arguments}/{edition}",
                           outcome.returncode == 0 and not outcome.stderr and "--registry" in outcome.stdout
                           and ("--message-id" in outcome.stdout if arguments == ("send", "--help") else True)
                           and ("--env" in outcome.stdout if arguments == ("start", "--help") else True)
                           and (all(option in outcome.stdout for option in
                                    ("--pane", "--workspace", "--cwd", "--harness", "--session"))
                                and "muse requires a pinned foreground process" in
                                " ".join(outcome.stdout.lower().split())
                                if arguments == ("adopt", "--help") else True)
                           and (all(option in outcome.stdout for option in (
                               "--recover-legacy-adoption", "--retire-dead-adoption", "--expected-token",
                               "--expected-record-sha256",
                           )) if arguments == ("stop", "--help") else True),
                           f"missing operation help: {outcome!r}")
    for command in ("quickstart", "userguide"):
        for edition, outcome in zip(("python", "rust"), harness.invoke(case, (command,)), strict=True):
            report.require(f"primary/{command}/{edition}", outcome.returncode == 0 and not outcome.stderr
                           and "agentctl start" in outcome.stdout and "agentctl send" in outcome.stdout,
                           f"installed guide has no working lifecycle: {outcome!r}")
    for edition, outcome in zip(("python", "rust"), harness.invoke(case, ("capabilities",)), strict=True):
        value = _json(outcome)
        report.require(f"primary/capabilities/{edition}", outcome.returncode == 0 and isinstance(value, dict)
                       and value.get("interactive") == {"backends": ["herdr"], "harnesses": ["codex", "claude", "muse"]}
                       and value.get("registry") == ".agentctl",
                       f"shared adapter metadata differs: {outcome!r}")
    _pair(harness, report, case, "primary/empty-list", ("list", *_COMMON))
    report.require("primary/empty-list-observational",
                   all(not (root / "registry").exists() for root in (case.python_root, case.rust_root)),
                   "listing an absent registry created persistent state")


def _lifecycle(harness: Harness, report: Report) -> None:
    for kind in ("codex", "claude"):
        case = harness.case(f"primary-{kind}")
        if not _start(harness, report, case, f"primary/{kind}/start", "--harness", kind,
                      "--model", "chosen-model", "--resume", "session-1", "--harness-arg=--extra",
                      "--env", "META_CODEX_AI_GATEWAY=azure-codex-cyber:openai",
                      "--env", "LITERAL= spaces $(unexpanded) = remain "):
            continue
        for command in (("status", "worker"), ("list",), ("wait", "worker", "--timeout", "0")):
            _pair(harness, report, case, f"primary/{kind}/{command[0]}", (*command, *_COMMON))
        expected_args = (["resume", "session-1", "--no-alt-screen"] if kind == "codex"
                         else ["--resume", "session-1"]) + ["--model", "chosen-model"] + (
                             ["--config", "model_reasoning_effort=medium"] if kind == "codex"
                             else ["--effort", "medium"]) + ["--extra"]
        report.require(f"primary/{kind}/native-launch-presets",
                       all(_state(root).get("launch_arguments") == expected_args
                           for root in (case.python_root, case.rust_root)), "native launch argv was changed or split")
        expected_environment = [
            "META_CODEX_AI_GATEWAY=azure-codex-cyber:openai",
            "LITERAL= spaces $(unexpanded) = remain ",
        ]
        report.require(f"primary/{kind}/literal-tab-environment",
                       all(_state(root).get("tab_environment") == expected_environment
                           for root in (case.python_root, case.rust_root)),
                       "tab environment was changed, split, or reordered")
        python, _ = _pair(harness, report, case, f"primary/{kind}/metadata", ("status", "worker", *_COMMON))
        status = _json(python)
        report.require(f"primary/{kind}/metadata-contract", isinstance(status, dict)
                       and all(status.get(key) == value for key, value in {
                           "name": "worker", "adapter": "herdr", "mode": "interactive", "backend": "herdr",
                           "paused": False, "runtime_home": None,
                           "capabilities": _OWNED_CAPABILITIES,
                           "pane_id": "w1:p1", "session_value": "session-1", "lifecycle": "running",
                       }.items()), f"missing canonical identity: {status!r}")
        report.require(f"primary/{kind}/environment-not-in-status",
                       isinstance(status, dict)
                       and all(entry not in json.dumps(status) for entry in expected_environment),
                       f"status exposed launch environment values: {status!r}")
        _pair(harness, report, case, f"primary/{kind}/bind", ("bind-session", "worker", "session-1", *_GOAL_COMMAND, *_COMMON))
        _pair(harness, report, case, f"primary/{kind}/goal-absent", ("goal", "worker", *_COMMON))
        python, _ = _pair(harness, report, case, f"primary/{kind}/set-goal", ("goal", "worker", "finish review", *_COMMON))
        goal = _json(python)
        report.require(f"primary/{kind}/goal-evidence", isinstance(goal, dict)
                       and goal.get("goal") == "finish review" and goal.get("delivery") == "delivered"
                       and goal.get("source") == ("native" if kind == "codex" else "requested")
                       and goal.get("native_status") == ("active" if kind == "codex" else "unverified"),
                       f"requested and native goal state were conflated: {goal!r}")
        _pair(harness, report, case, f"primary/{kind}/read-goal", ("goal", "worker", *_COMMON))
        text = "literal 'quotes' and $(unexpanded)\nsecond line"
        _pair(harness, report, case, f"primary/{kind}/send", ("send", "worker", text, "--message-id", "followup", *_COMMON))
        _pair(harness, report, case, f"primary/{kind}/duplicate-id", ("send", "worker", text, "--message-id", "followup", *_COMMON), 75)
        report.require(f"primary/{kind}/send-exactly-once", all(_submission_count(root, text) == 1
                       for root in (case.python_root, case.rust_root)), "explicit ID was delivered twice")
        typed = [_state(root).get("submitted") for root in (case.python_root, case.rust_root)]
        lasts = [value[-1] if isinstance(value, list) and value and isinstance(value[-1], str) else ""
                 for value in typed]
        report.require(f"primary/{kind}/sender-header",
                       all(SENDER_HEADER.match(last) and last.endswith("\n" + text) for last in lasts)
                       and lasts[0].split("\n", 1)[0] == lasts[1].split("\n", 1)[0],
                       f"send did not open the prompt with the same sender header: {lasts!r}")
        for root in (case.python_root, case.rust_root):
            (root / "prompt.txt").write_text("file instruction\nwith newline\n", encoding="utf-8")
        _pair(harness, report, case, f"primary/{kind}/send-file", ("send", "worker", "--file", "<ROOT>/prompt.txt", *_COMMON))
        _pair(harness, report, case, f"primary/{kind}/read", ("read", "worker", "--lines", "17", *_COMMON))
        report.require(f"primary/{kind}/read-snapshot", all((root / "registry/agents/worker/output.json").is_file()
                       for root in (case.python_root, case.rust_root)), "read did not retain its documented terminal snapshot")
        _pair(harness, report, case, f"primary/{kind}/stop", ("stop", "worker", *_COMMON))
        _pair(harness, report, case, f"primary/{kind}/list-stopped", ("list", *_COMMON))
        report.require(f"primary/{kind}/exact-pane-retirement", all(
            _state(root).get("closed_panes") == [_fixture_identity(root)[0]] and not (root / "registry/agents/worker").exists()
            and len(list((root / "registry/archive").iterdir())) == 1
                       for root in (case.python_root, case.rust_root)), "owned pane or archival contract diverged")

    custom = harness.case("primary-existing-native-kind")
    # A Herdr-native kind has no structured effort adapter, so it cannot
    # declare the effort the launch rule requires: both editions refuse
    # the start (rc 2, no record) rather than launch it undeclared.
    _pair(harness, report, custom, "primary/native-kind/start", (
        "start", "worker", "--cwd", "<ROOT>", "--workspace-id", "w1",
        "--harness", "third-party", *_COMMON,
    ), 2)
    report.require("primary/native-kind/no-record",
                   all(not (root / "registry/agents/worker").exists()
                       for root in (custom.python_root, custom.rust_root)),
                   "refused native-kind start recorded an agent")


def _conversations(harness: Harness, report: Report) -> None:
    case = harness.case("primary-claude-conversation")
    _change(case, {"sessionless": True})
    outcomes = _pair(harness, report, case, "primary/conversation/claude/start", (
        "start", "worker", "--cwd", "<ROOT>", "--workspace-id", "w1", "--harness", "claude", *_COMMON,
    ))
    for edition, root, outcome in zip(("python", "rust"), (case.python_root, case.rust_root), outcomes, strict=True):
        if outcome.returncode:
            continue
        value = json.loads(outcome.stdout)
        native = value.get("native_session")
        arguments = _state(root).get("launch_arguments")
        report.require(f"primary/conversation/claude/provenance/{edition}",
                       isinstance(native, dict) and set(native) == {"schema", "agent", "value", "source"}
                       and native.get("schema") == "agentctl-native-session/v1"
                       and native.get("agent") == "claude" and native.get("source") == "asserted"
                       and isinstance(native.get("value"), str)
                       and re.fullmatch(r"[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}", native["value"]) is not None
                       and arguments == ["--session-id", native["value"], "--effort", "medium"]
                       and value.get("session_value") is None,
                       f"generated conversation not retained separately from routing identity: {value!r}")
    _pair(harness, report, case, "primary/conversation/claude/status", ("status", "worker", *_COMMON))
    _pair(harness, report, case, "primary/conversation/claude/list", ("list", *_COMMON))
    _pair(harness, report, case, "primary/conversation/claude/stop", ("stop", "worker", *_COMMON))
    for selector in ("--session-id=foreign", "--continue", "-rforeign", "-cforeign", "--fork-session"):
        rejected = harness.case(f"primary-claude-conversation-conflict-{selector}")
        _pair(harness, report, rejected, f"primary/conversation/claude/refuse-{selector}", (
            "start", "worker", "--cwd", "<ROOT>", "--harness", "claude", f"--harness-arg={selector}", *_COMMON,
        ), 75)
        report.require(f"primary/conversation/claude/no-allocation-{selector}",
                       all(not (root / "registry/agents/worker").exists() for root in (rejected.python_root, rejected.rust_root)),
                       "conflicting conversation allocated a runtime")
    forked = harness.case("primary-claude-resume-fork")
    _pair(harness, report, forked, "primary/conversation/claude/refuse-resume-fork", (
        "start", "worker", "--cwd", "<ROOT>", "--harness", "claude", "--resume", "chosen-conversation",
        "--harness-arg=--fork-session", *_COMMON,
    ), 75)
    report.require("primary/conversation/claude/no-allocation-resume-fork",
                   all(not (root / "registry/agents/worker").exists() for root in (forked.python_root, forked.rust_root)),
                   "fork selector allocated a replacement conversation")
    mismatch = harness.case("primary-resume-conversation-mismatch")
    _change(mismatch, {"session_value_override": "wrong-conversation"})
    _pair(harness, report, mismatch, "primary/conversation/resume/mismatch", (
        "start", "worker", "--cwd", "<ROOT>", "--workspace-id", "w1", "--harness", "codex",
        "--resume", "chosen-conversation", "--brief", "must never arrive", *_COMMON,
    ), 75)
    report.require("primary/conversation/resume/no-brief",
                   all(not _submitted(root) for root in (mismatch.python_root, mismatch.rust_root)),
                   "brief reached a contradictory conversation")


def _launch_specs(harness: Harness, report: Report) -> None:
    """Both editions write, validate, and refuse the same launch spec and promote states."""
    case = harness.case("primary-launch-spec")
    outcomes = _pair(harness, report, case, "primary/launch-spec/start", (
        "start", "worker", "--cwd", "<ROOT>", "--workspace-id", "w1", "--harness", "claude", *_COMMON,
    ))
    for edition, outcome in zip(("python", "rust"), outcomes, strict=True):
        status = json.loads(outcome.stdout) if outcome.returncode == 0 else {}
        spec = status.get("launch_spec")
        report.require(f"primary/launch-spec/start/{edition}",
                       isinstance(spec, dict) and spec.get("schema") == "agentctl-launch-spec/v1"
                       and spec.get("source") == "start" and spec.get("argv") == status.get("arguments")
                       and spec.get("executable") is None
                       and isinstance(spec.get("native_session"), dict)
                       and spec["native_session"].get("source") == "asserted",
                       f"start did not record its exact launch: {status!r}")
    _pair(harness, report, case, "primary/launch-spec/promote-running", ("promote", "worker", *_COMMON), 75)
    for root in (case.python_root, case.rust_root):
        path = root / "registry/agents/worker/agent.json"
        document = json.loads(path.read_text(encoding="utf-8"))
        document["launch_spec"]["source"] = "guess"
        path.write_text(json.dumps(document), encoding="utf-8")
    _pair(harness, report, case, "primary/launch-spec/malformed-status", ("status", "worker", *_COMMON), 75)
    configured = harness.case("primary-launch-spec-wrkslots")
    for value, expected in (("relative/wrkslots", 75), ("/opt/wrkslots", 0)):
        for root in (configured.python_root, configured.rust_root):
            directory = root / ".agentctl"
            directory.mkdir(mode=0o700, exist_ok=True)
            init_case_repository(root)
            (root / ".gitignore").write_text(".agentctl/\n", encoding="utf-8")
            config = directory / "profiles.json"
            config.write_text(json.dumps({"schema": "agentctl-profiles/v1", "wrkslots": value, "profiles": {}}),
                              encoding="utf-8")
            config.chmod(0o600)
        _pair(harness, report, configured, f"primary/launch-spec/wrkslots-config/{value}",
              ("profiles", "--cwd", "<ROOT>"), expected)
    launch_env = harness.case("primary-launch-env-config")
    launch_env_cases: tuple[tuple[str, dict[str, object], int], ...] = (
        ("relative-env-file", {"env_file": "relative/env.sh"}, 75),
        ("unknown-key", {"path": ["/opt/bin"]}, 75),
        ("duplicate-directory", {"path_prepend": ["/opt/bin", "/opt/bin"]}, 75),
        ("valid", {"env_file": "/opt/team/env.sh", "path_prepend": ["/opt/team/bin"]}, 0),
    )
    for label, launch_value, expected in launch_env_cases:
        for root in (launch_env.python_root, launch_env.rust_root):
            directory = root / ".agentctl"
            directory.mkdir(mode=0o700, exist_ok=True)
            init_case_repository(root)
            (root / ".gitignore").write_text(".agentctl/\n", encoding="utf-8")
            config = directory / "profiles.json"
            config.write_text(json.dumps({"schema": "agentctl-profiles/v1", "launch_env": launch_value,
                                          "profiles": {}}),
                              encoding="utf-8")
            config.chmod(0o600)
        _pair(harness, report, launch_env, f"primary/launch-env-config/{label}",
              ("profiles", "--cwd", "<ROOT>"), expected)
    # A top-level key from a newer agentctl is warned about and ignored, identically in both editions.
    newer = harness.case("primary-unknown-top-level-config")
    for root in (newer.python_root, newer.rust_root):
        directory = root / ".agentctl"
        directory.mkdir(mode=0o700, exist_ok=True)
        init_case_repository(root)
        (root / ".gitignore").write_text(".agentctl/\n", encoding="utf-8")
        config = directory / "profiles.json"
        config.write_text(json.dumps({"schema": "agentctl-profiles/v1", "zz_future": {"on": True},
                                      "a_future": 1, "profiles": {}}),
                          encoding="utf-8")
        config.chmod(0o600)
    _pair(harness, report, newer, "primary/unknown-top-level-config",
          ("profiles", "--cwd", "<ROOT>"), 0)
    # So is a key inside a profile or its agentcloud settings, named by its path (#809
    # chat-bridge-json-forward-compat).
    for root in (newer.python_root, newer.rust_root):
        config = root / ".agentctl" / "profiles.json"
        config.write_text(json.dumps({"schema": "agentctl-profiles/v1", "profiles": {
            "worker": {"harness": "codex", "mode": "interactive", "argv": [], "env": {},
                       "surprise": True},
        }}), encoding="utf-8")
    _pair(harness, report, newer, "primary/unknown-profile-key",
          ("profiles", "--cwd", "<ROOT>"), 0)
    # ... but a duplicate key is refused there as everywhere else in the file.
    for root in (newer.python_root, newer.rust_root):
        config = root / ".agentctl" / "profiles.json"
        config.write_text('{"schema": "agentctl-profiles/v1", "profiles": {}, "x": {"a": 1, "a": 2}}',
                          encoding="utf-8")
    _pair(harness, report, newer, "primary/unknown-top-level-config/duplicate",
          ("profiles", "--cwd", "<ROOT>"), 75)


def _profiles(harness: Harness, report: Report) -> None:
    document = {
        "schema": "agentctl-profiles/v1",
        "profiles": {
            "astra-ultra": {
                "harness": "codex", "mode": "interactive", "model": "gpt-6-astra",
                "reasoning_effort": "ultra",
                "argv": ["--dangerously-enable-internet-mode", "--dangerously-bypass-approvals-and-sandbox"],
                "env": {"META_CODEX_AI_GATEWAY": "azure-codex-cyber:openai"},
            },
            "sol": {
                "harness": "codex", "mode": "interactive", "model": "gpt-5.6-sol",
                "reasoning_effort": "medium",
                "argv": ["--dangerously-enable-internet-mode", "--dangerously-bypass-approvals-and-sandbox"],
                "env": {"META_CODEX_AI_GATEWAY": "azure-codex-cyber:openai"},
            },
            "watermelon": {
                "harness": "muse", "mode": "interactive",
                "model": "kiki_gb300_mxfp8_6p2_840_nwr", "reasoning_effort": "ultra",
                "argv": [], "env": {},
            },
            "watermelon-exec": {
                "harness": "muse", "mode": "headless",
                "model": "kiki_gb300_mxfp8_6p2_840_nwr", "reasoning_effort": "ultra",
                "argv": ["--image=private-fixture.png"], "env": {},
            },
            "muse-literal": {
                "harness": "muse", "mode": "interactive", "reasoning_effort": "medium",
                "argv": [
                    "--trust-workspace",
                    '--meta-tag=literal $(unexpanded) "quotes"',
                    "--meta-tag=repeatable",
                ],
                "env": {},
            },
            "codex-safe-config": {
                "harness": "codex", "mode": "interactive",
                "argv": ["--config=features.web_search=true"], "env": {},
            },
            # An explicit null agentcloud block means absent in both editions.
            "codex-null-cloud": {
                "harness": "codex", "mode": "interactive", "agentcloud": None,
            },
            # Launchable only by the Rust edition; both editions must list it identically and
            # its presence must not break any other profile in the file.
            "agentcloud-worker": {
                "harness": "agentcloud", "mode": "interactive", "model": "provider-model",
                "argv": ["--narration"], "env": {"TAB_SETTING": "private-tab-value"},
                "agentcloud": {"harness": "claude-code", "provision": True,
                               "envspec": "<envspec>", "purpose": "<purpose>",
                               "title": "{name}-{host}"},
            },
        },
    }
    expected = {
        "astra-ultra": ["--no-alt-screen", "--model", "gpt-6-astra", "--config",
                         "model_reasoning_effort=ultra", "--dangerously-enable-internet-mode",
                         "--dangerously-bypass-approvals-and-sandbox"],
        "sol": ["--no-alt-screen", "--model", "gpt-5.6-sol",
                "--config", "model_reasoning_effort=medium",
                "--dangerously-enable-internet-mode", "--dangerously-bypass-approvals-and-sandbox"],
        "watermelon": ["--model", "kiki_gb300_mxfp8_6p2_840_nwr", "--reasoning-effort", "ultra"],
        "muse-literal": ["--reasoning-effort", "medium", "--trust-workspace",
                         '--meta-tag=literal $(unexpanded) "quotes"',
                         "--meta-tag=repeatable"],
        "codex-safe-config": ["--no-alt-screen", "--config=features.web_search=true"],
    }
    for profile in ("astra-ultra", "sol", "watermelon", "muse-literal", "codex-safe-config"):
        case = harness.case(f"primary-profile-{profile}")
        for root in (case.python_root, case.rust_root):
            init_case_repository(root)
            (root / ".gitignore").write_text(".agentctl/\n", encoding="utf-8")
            directory = root / ".agentctl"
            directory.mkdir(mode=0o700)
            config = directory / "profiles.json"
            config.write_text(json.dumps(document), encoding="utf-8")
            config.chmod(0o600)
        listed = _pair(harness, report, case, f"primary/profile/{profile}/list",
                       ("profiles", "--cwd", "<ROOT>"))
        for edition, outcome in zip(("python", "rust"), listed, strict=True):
            public = json.loads(outcome.stdout)
            serialized = json.dumps(public, sort_keys=True)
            report.require(
                f"primary/profile/{profile}/list-redaction/{edition}",
                "azure-codex-cyber:openai" not in serialized
                and "--dangerously-enable-internet-mode" not in serialized
                and "private-fixture.png" not in serialized
                and "private-tab-value" not in serialized
                and any(
                    item.get("name") == "codex-null-cloud" and "agentcloud" not in item
                    for item in public.get("profiles", [])
                )
                and any(
                    item.get("name") == "agentcloud-worker"
                    and item.get("requires_edition") == "rust"
                    and item.get("argv_count") == 1
                    and item.get("agentcloud") == {
                        "harness": "claude-code", "provision": True, "envspec": "<envspec>",
                        "purpose": "<purpose>", "workspace": None, "node_id": None,
                        "title": "{name}-{host}",
                    }
                    for item in public.get("profiles", [])
                )
                and any(
                    item.get("name") == "watermelon-exec"
                    and item.get("harness") == "muse"
                    and item.get("mode") == "headless"
                    and item.get("argv_count") == 1
                    and item.get("environment") == []
                    for item in public.get("profiles", [])
                ),
                f"safe profile metadata disclosed private values or omitted headless Muse: {public!r}",
            )
        python, rust = _pair(harness, report, case, f"primary/profile/{profile}/start", (
            "start", "worker", "--cwd", "<ROOT>", "--workspace-id", "w1",
            "--profile", profile, *_COMMON,
        ), 2 if profile == "codex-safe-config" else 0)
        if profile == "codex-safe-config":
            # A raw --config profile cannot declare structured effort (the
            # loader refuses that combination), so its start is the shared
            # no-effort refusal and records nothing.
            report.require("primary/profile/codex-safe-config/no-record", all(
                not (root / "registry/agents/worker").exists()
                for root in (case.python_root, case.rust_root)),
                "refused safe-config start recorded an agent")
            continue
        status = _json(python)
        report.require(f"primary/profile/{profile}/recorded-policy", isinstance(status, dict)
                       and status.get("profile") == profile,
                       f"launch profile was not retained: {status!r}")
        report.require(f"primary/profile/{profile}/argv", all(
            _state(root).get("launch_arguments") == expected[profile]
            for root in (case.python_root, case.rust_root)
        ), "profile argv changed, split, or reordered")
        if profile == "sol":
            # The Rust start would contact a real agentcloud service, so only the Python
            # refusal is exercised here; the Rust launcher is covered by its own tests.
            python_refusal = harness._invoke_one(harness.python, case.python_root, (
                "start", "sub-cloud", "--cwd", "<ROOT>", "--workspace-id", "w1",
                "--profile", "agentcloud-worker", *_COMMON,
            ))
            report.require(
                "primary/profile/agentcloud/python-start-refusal",
                python_refusal.returncode == 2
                and "only the Rust edition of agentctl implements" in python_refusal.stderr
                and not (case.python_root / "registry" / "agents" / "sub-cloud").exists(),
                f"Python started or mis-refused a Rust-only agentcloud profile: {python_refusal!r}",
            )
        adapter = "herdr-pane" if profile in ("watermelon", "muse-literal") else "herdr"
        report.require(f"primary/profile/{profile}/adapter",
                       isinstance(status, dict) and status.get("adapter") == adapter,
                       f"profile selected wrong adapter: {status!r}")
        if profile == "watermelon":
            for edition, outcome in zip(("python", "rust"), (python, rust), strict=True):
                raw_status = json.loads(outcome.stdout)
                identity = raw_status.get("custom_process_identity")
                report.require(
                    f"primary/profile/watermelon/process-identity/{edition}",
                    isinstance(identity, dict)
                    and set(identity) == {
                        "version", "boot_id", "pid", "starttime_ticks",
                        "executable_device", "executable_inode",
                    }
                    and identity.get("version") == 1
                    and isinstance(identity.get("boot_id"), str)
                    and all(
                        isinstance(identity.get(field), int) and identity[field] > 0
                        for field in (
                            "pid", "starttime_ticks", "executable_device", "executable_inode",
                        )
                    ),
                    f"{edition} omitted the pinned custom process identity: {raw_status!r}",
                )
            report.require("primary/profile/watermelon/effective-effort",
                           isinstance(status, dict)
                           and status.get("effective_reasoning_effort") == "xhigh"
                           and status.get("startup_warning") == (
                               "reasoning effort ultra is not available "
                               "(gate ultra_reasoning_effort is closed); using xhigh"
                           ), f"Muse downgrade was hidden or misreported: {status!r}")
            for root in (case.python_root, case.rust_root):
                executable = root / "fake-muse-runtime"
                replacement = root / "fake-muse-runtime.new"
                shutil.copyfile(sys.executable, replacement)
                replacement.chmod(0o700)
                os.replace(replacement, executable)
                process_id = _state(root).get("custom_pid")
                report.require(
                    f"primary/profile/watermelon/deleted-image/{root.parent.name}",
                    isinstance(process_id, int)
                    and os.readlink(f"/proc/{process_id}/exe").endswith(" (deleted)"),
                    "atomic replacement did not retain the old running executable inode",
                )
            _pair(harness, report, case, "primary/profile/watermelon/status-after-upgrade", (
                "status", "worker", *_COMMON,
            ))
            original_records = {
                root: json.loads(
                    (root / "registry/agents/worker/agent.json").read_text(encoding="utf-8")
                )
                for root in (case.python_root, case.rust_root)
            }
            mismatch_values: tuple[tuple[str, str, object], ...] = (
                ("pid", "pid", 1),
                ("starttime", "starttime_ticks", 1),
                ("boot", "boot_id", "ffffffff-ffff-ffff-ffff-ffffffffffff"),
                ("device", "executable_device", 1),
                ("inode", "executable_inode", 1),
            )
            for label, field, adjustment in mismatch_values:
                for root, original in original_records.items():
                    changed = json.loads(json.dumps(original))
                    if field == "boot_id":
                        changed["custom_process_identity"][field] = adjustment
                    else:
                        changed["custom_process_identity"][field] += adjustment
                    (root / "registry/agents/worker/agent.json").write_text(
                        json.dumps(changed), encoding="utf-8"
                    )
                _pair(
                    harness, report, case,
                    f"primary/profile/watermelon/refuse-{label}-status",
                    ("status", "worker", *_COMMON),
                )
                refused = harness.invoke(case, ("stop", "worker", *_COMMON))
                report.require(
                    f"primary/profile/watermelon/refuse-{label}-stop",
                    all(outcome.returncode == 69 for outcome in refused)
                    and all(
                        (root / "registry/agents/worker/agent.json").is_file()
                        and not _state(root).get("closed")
                        for root in (case.python_root, case.rust_root)
                    ),
                    f"{label} identity mismatch allowed pane retirement: {refused!r}",
                )
                for root, original in original_records.items():
                    (root / "registry/agents/worker/agent.json").write_text(
                        json.dumps(original), encoding="utf-8"
                    )
            _change(case, {"wrong_custom_process_group": True})
            _pair(
                harness, report, case,
                "primary/profile/watermelon/refuse-process-group-status",
                ("status", "worker", *_COMMON),
            )
            refused = harness.invoke(case, ("stop", "worker", *_COMMON))
            report.require(
                "primary/profile/watermelon/refuse-process-group-stop",
                all(outcome.returncode == 69 for outcome in refused)
                and all(
                    (root / "registry/agents/worker/agent.json").is_file()
                    and not _state(root).get("closed")
                    for root in (case.python_root, case.rust_root)
                ),
                f"foreground process-group mismatch allowed pane retirement: {refused!r}",
            )
            _change(case, {"wrong_custom_process_group": False})

            malformed_variants: tuple[tuple[str, str, object], ...] = (
                ("bool", "pid", True),
                ("overflow", "starttime_ticks", 1 << 64),
                ("zero-device", "executable_device", 0),
            )
            for label, field, value in malformed_variants:
                for root, original in original_records.items():
                    changed = json.loads(json.dumps(original))
                    changed["custom_process_identity"][field] = value
                    (root / "registry/agents/worker/agent.json").write_text(
                        json.dumps(changed), encoding="utf-8"
                    )
                _pair(
                    harness, report, case,
                    f"primary/profile/watermelon/malformed-{label}",
                    ("status", "worker", *_COMMON), 75,
                )
                for root, original in original_records.items():
                    (root / "registry/agents/worker/agent.json").write_text(
                        json.dumps(original), encoding="utf-8"
                    )
            # A key a newer agentctl added to the identity is ignored by both editions (#809
            # chat-bridge-json-forward-compat).
            for root, original in original_records.items():
                changed = json.loads(json.dumps(original))
                changed["custom_process_identity"]["unexpected"] = 1
                (root / "registry/agents/worker/agent.json").write_text(
                    json.dumps(changed), encoding="utf-8"
                )
            _pair(harness, report, case, "primary/profile/watermelon/identity-newer-key",
                  ("status", "worker", *_COMMON), 0)
            for root, original in original_records.items():
                (root / "registry/agents/worker/agent.json").write_text(
                    json.dumps(original), encoding="utf-8"
                )
            prompt = "Muse literal $(unexpanded) delivery\nsecond line"
            _pair(harness, report, case, "primary/profile/watermelon/send", (
                "send", "worker", prompt, "--message-id", "muse-first", *_COMMON,
            ))
            report.require("primary/profile/watermelon/pane-delivery",
                           all(_submitted(root) == [prompt]
                               and _state(root).get("paste_wrapped") is True
                               and (root / "registry/agents/worker/queue/processed/muse-first.json").is_file()
                               and not list((root / "registry/agents/worker/queue/inflight").glob("*.json"))
                               and not list((root / "registry/agents/worker/queue/failed").glob("*.json"))
                               for root in (case.python_root, case.rust_root)),
                           "Muse pane delivery lacked an exact post-Enter receipt")
            _change(case, {
                "custom_post_error": True, "custom_submitted": False, "custom_draft": "",
            })
            # No exact stdout comparison here (#1067
            # cross-differential-load-aware): the failure JSON's reason
            # depends on whether a redraw read lands before the 0.05
            # working expiry, so under load the editions can split the
            # reason while agreeing on the refusal. What is compared is
            # rc 76 from both, plus the quarantine asserted just below;
            # the 0.05 budget is meant to expire and keeps its literal.
            redraw_outcomes = harness.invoke(case, (
                "send", "worker", "must remain uncertain", "--message-id", "muse-error",
                "--working-timeout", "0.05", *_COMMON,
            ))
            report.require("primary/profile/watermelon/redraw-refusal",
                           all(outcome.returncode == 76 for outcome in redraw_outcomes),
                           f"expected rc 76; python={redraw_outcomes[0]!r} rust={redraw_outcomes[1]!r}")
            report.require("primary/profile/watermelon/redraw-quarantine", all(
                (root / "registry/agents/worker/queue/failed/muse-error.json").is_file()
                for root in (case.python_root, case.rust_root)
            ), "Muse draft/error redraw was incorrectly accepted as delivery")
            _change(case, {
                "custom_post_error": False, "custom_clear_without_submit": True,
                "custom_submitted": False, "custom_draft": "",
            })
            # Same split as redraw-refusal just above: rc 76 from both
            # editions, no exact failure-stdout comparison (#1067).
            _wait_for_queue_quiescence(case, report,
                                       "primary/profile/watermelon/repeated-quiescence")
            repeated_outcomes = harness.invoke(case, (
                "send", "worker", prompt, "--message-id", "muse-repeat",
                "--working-timeout", "0.05", *_COMMON,
            ))
            report.require("primary/profile/watermelon/repeated-redraw",
                           all(outcome.returncode == 76 for outcome in repeated_outcomes),
                           f"expected rc 76; python={repeated_outcomes[0]!r} rust={repeated_outcomes[1]!r}")
            report.require("primary/profile/watermelon/repeated-quarantine", all(
                (root / "registry/agents/worker/queue/failed/muse-repeat.json").is_file()
                for root in (case.python_root, case.rust_root)
            ), "an old identical transcript entry was mistaken for a new submission")
        if profile not in ("watermelon", "muse-literal"):
            expected_environment = (
                ["META_CODEX_AI_GATEWAY=azure-codex-cyber:openai"]
                if profile in ("astra-ultra", "sol") else []
            )
            report.require(f"primary/profile/{profile}/environment", all(
                _state(root).get("tab_environment") == expected_environment
                for root in (case.python_root, case.rust_root)
            ), "profile environment changed or leaked")
        _pair(harness, report, case, f"primary/profile/{profile}/stop", (
            "stop", "worker", *_COMMON,
        ))


def _profile_resume(harness: Harness, report: Report) -> None:
    """A profile resumes the asserted conversation without changing its launch policy."""
    conversation = "11111111-1111-4111-8111-111111111111"
    literal_argument = '--meta-tag=literal $(unexpanded) "quotes"'
    literal_environment = 'PROFILE_SELECTOR=literal $(unexpanded) "quotes"'
    expected_arguments = {
        "claude": ["--resume", conversation, "--model", "selected-model", "--effort", "high", literal_argument],
        "codex": ["resume", conversation, "--no-alt-screen", "--model", "selected-model", "--config",
                  "model_reasoning_effort=high", literal_argument],
        "muse": ["--model", "selected-model", "--reasoning-effort", "high", literal_argument,
                 "resume", conversation],
    }

    def configured_case(kind: str, scenario: str, *, raw: Sequence[str] | None = None) -> PairCase:
        case = harness.case(f"primary-profile-resume-{kind}-{scenario}", {
            "report_custom_session": True,
            **({"session_value_override": "wrong-conversation"} if scenario == "mismatch" else {}),
        })
        document = {
            "schema": "agentctl-profiles/v1",
            "profiles": {"local-policy": {
                "harness": kind, "mode": "interactive", "model": "selected-model",
                "reasoning_effort": "high", "argv": [literal_argument] if raw is None else list(raw),
                "env": {"PROFILE_SELECTOR": literal_environment.partition("=")[2]},
            }},
        }
        for root in (case.python_root, case.rust_root):
            init_case_repository(root)
            (root / ".gitignore").write_text(".agentctl/\n", encoding="utf-8")
            directory = root / ".agentctl"
            directory.mkdir(mode=0o700)
            config = directory / "profiles.json"
            config.write_text(json.dumps(document), encoding="utf-8")
            config.chmod(0o600)
        return case

    for kind in ("claude", "codex", "muse"):
        selected = configured_case(kind, "selected")
        outcomes = _pair(harness, report, selected, f"primary/profile/resume/{kind}/start", (
            "start", "worker", "--cwd", "<ROOT>", "--workspace-id", "w1", "--profile", "local-policy",
            "--resume", conversation, "--brief", "continue the saved conversation", *_COMMON,
        ))
        for edition, root, outcome in zip(
            ("python", "rust"), (selected.python_root, selected.rust_root), outcomes, strict=True,
        ):
            if outcome.returncode:
                continue
            value = _json(outcome)
            native = {
                "schema": "agentctl-native-session/v1", "agent": kind,
                "value": conversation, "source": "asserted",
            }
            report.require(f"primary/profile/resume/{kind}/policy/{edition}",
                           isinstance(value, dict) and value.get("profile") == "local-policy"
                           and value.get("model") == "selected-model"
                           and value.get("reasoning_effort") == "high"
                           and value.get("cwd") == "<ROOT>" and value.get("mode") == "interactive"
                           and value.get("arguments") == expected_arguments[kind]
                           and value.get("native_session") == native and value.get("resume") == conversation
                           and value.get("session_agent") == kind and value.get("session_value") == conversation
                           and value.get("environment_names") == ["PROFILE_SELECTOR"],
                           f"profile resume did not retain its conversation and policy: {value!r}")
            state = _state(root)
            report.require(f"primary/profile/resume/{kind}/launch/{edition}",
                           state.get("launch_arguments") == expected_arguments[kind]
                           and state.get("tab_environment") == [literal_environment]
                           and state.get("submitted") == ["continue the saved conversation"],
                           "the literal launch policy or startup brief changed")
        _pair(harness, report, selected, f"primary/profile/resume/{kind}/status", (
            "status", "worker", *_COMMON,
        ))
        _pair(harness, report, selected, f"primary/profile/resume/{kind}/list", ("list", *_COMMON))
        roots = (selected.python_root, selected.rust_root)
        before = {
            root: ((_state(root).get("calls")), (root / "registry/agents/worker/agent.json").read_bytes())
            for root in roots if (root / "registry/agents/worker/agent.json").exists()
        }
        _pair(harness, report, selected, f"primary/profile/resume/{kind}/occupied", (
            "start", "other-worker", "--cwd", "<ROOT>", "--workspace-id", "w1",
            "--profile", "local-policy", "--resume", conversation, *_COMMON,
        ), 75)
        report.require(f"primary/profile/resume/{kind}/occupied-atomic",
                       len(before) == 2 and all(
                           not (root / "registry/agents/other-worker").exists()
                           and _state(root).get("calls") == calls
                           and (root / "registry/agents/worker/agent.json").read_bytes() == record
                           for root, (calls, record) in before.items()
                       ), "an occupied conversation changed its owner or allocated a new tab")
        _pair(harness, report, selected, f"primary/profile/resume/{kind}/stop", (
            "stop", "worker", *_COMMON,
        ))

        mismatch = configured_case(kind, "mismatch")
        _pair(harness, report, mismatch, f"primary/profile/resume/{kind}/mismatch", (
            "start", "worker", "--cwd", "<ROOT>", "--workspace-id", "w1",
            "--profile", "local-policy", "--resume", conversation, "--brief", "must never arrive", *_COMMON,
        ), 75)
        report.require(f"primary/profile/resume/{kind}/mismatch-no-brief",
                       all(not _submitted(root)
                           for root in (mismatch.python_root, mismatch.rust_root)),
                       "a startup brief reached a contradictory conversation")
        _retire_fixture_processes(mismatch, report)

        raw_selector = ("--resume", "other-conversation") if kind == "claude" else ("resume", "other-conversation")
        duplicate = configured_case(kind, "duplicate", raw=raw_selector)
        _pair(harness, report, duplicate, f"primary/profile/resume/{kind}/duplicate", (
            "start", "worker", "--cwd", "<ROOT>", "--workspace-id", "w1",
            "--profile", "local-policy", "--resume", conversation, *_COMMON,
        ), 75)
        report.require(f"primary/profile/resume/{kind}/duplicate-atomic",
                       all(not (root / "registry").exists() and not _state(root).get("calls")
                           for root in (duplicate.python_root, duplicate.rust_root)),
                       "duplicate resume selectors allocated a registry or contacted Herdr")


def _skill_install(harness: Harness, report: Report) -> None:
    case = harness.case("primary-skill-install")
    first_python, first_rust = harness.invoke(case, ("skill", "install"))
    report.require("primary/skill/install", first_python.returncode == first_rust.returncode == 0
                   and _json(first_python) == _json(first_rust),
                   f"skill install diverged: {first_python!r} {first_rust!r}")
    for root in (case.python_root, case.rust_root):
        paths = [
            root / "skill-homes/codex/agentctl/SKILL.md",
            root / "skill-homes/claude/agentctl/SKILL.md",
            root / "xdg-config/muse/skills/agentctl/SKILL.md",
        ]
        report.require(f"primary/skill/content/{root.parent.name}",
                       all(path.is_file() for path in paths)
                       and len({path.read_bytes() for path in paths}) == 1,
                       "not every harness received the same skill")
        calls = (root / "muse-skill-calls.jsonl").read_text(encoding="utf-8").splitlines()
        report.require(f"primary/skill/muse-native/{root.parent.name}", len(calls) == 1,
                       "Muse skill did not use its native managed installer exactly once")
    second_python, second_rust = harness.invoke(case, ("skill", "install"))
    report.require("primary/skill/idempotent", second_python.returncode == second_rust.returncode == 0
                   and _json(second_python) == _json(second_rust)
                   and _json(second_python) == {"installed": [], "unchanged": ["codex", "claude", "muse"]},
                   f"second install was not idempotent: {second_python!r} {second_rust!r}")
    report.require("primary/skill/muse-idempotent-native", all(
        len((root / "muse-skill-calls.jsonl").read_text(encoding="utf-8").splitlines()) == 1
        for root in (case.python_root, case.rust_root)
    ), "idempotent Muse install needlessly invoked the native installer")
    for root in (case.python_root, case.rust_root):
        (root / "skill-homes/codex/agentctl/SKILL.md").write_text("owner customization\n", encoding="utf-8")
    refused = harness.invoke(case, ("skill", "install", "--harness", "codex"))
    report.require("primary/skill/divergent-refusal",
                   all(outcome.returncode == 75 for outcome in refused),
                   f"divergent skill was overwritten without force: {refused!r}")
    forced_python, forced_rust = harness.invoke(
        case, ("skill", "install", "--harness", "codex", "--force")
    )
    report.require("primary/skill/explicit-force",
                   forced_python.returncode == forced_rust.returncode == 0
                   and _json(forced_python) == _json(forced_rust),
                   f"explicit skill replacement diverged: {forced_python!r} {forced_rust!r}")

    for root in (case.python_root, case.rust_root):
        (root / "xdg-config/muse/skills/agentctl/SKILL.md").write_text(
            "owner customization\n", encoding="utf-8"
        )
    refused = harness.invoke(case, ("skill", "install", "--harness", "muse"))
    report.require("primary/skill/muse-divergent-refusal",
                   all(outcome.returncode == 75 for outcome in refused)
                   and all(
                       (root / "xdg-config/muse/skills/agentctl/SKILL.md").read_text(
                           encoding="utf-8"
                       ) == "owner customization\n"
                       for root in (case.python_root, case.rust_root)
                   ), f"divergent managed Muse skill was overwritten: {refused!r}")
    forced_python, forced_rust = harness.invoke(
        case, ("skill", "install", "--harness", "muse", "--force")
    )
    report.require("primary/skill/muse-explicit-force",
                   forced_python.returncode == forced_rust.returncode == 0
                   and _json(forced_python) == _json(forced_rust)
                   and all(
                       len((root / "muse-skill-calls.jsonl").read_text(
                           encoding="utf-8"
                       ).splitlines()) == 2
                       for root in (case.python_root, case.rust_root)
                   ), f"explicit managed Muse replacement diverged: {forced_python!r} {forced_rust!r}")

    symlink = harness.case("primary-skill-symlink")
    for root in (symlink.python_root, symlink.rust_root):
        destination = root / "skill-homes/codex/agentctl"
        destination.mkdir(parents=True, mode=0o700)
        (root / "outside-skill").write_text("outside\n", encoding="utf-8")
        (destination / "SKILL.md").symlink_to(root / "outside-skill")
    outcomes = harness.invoke(symlink, ("skill", "install", "--harness", "codex", "--force"))
    report.require("primary/skill/symlink-refusal",
                   all(outcome.returncode == 75 for outcome in outcomes)
                   and all((root / "outside-skill").read_text(encoding="utf-8") == "outside\n"
                           for root in (symlink.python_root, symlink.rust_root)),
                   f"skill symlink was followed: {outcomes!r}")

    muse_symlink = harness.case("primary-skill-muse-symlink")
    for root in (muse_symlink.python_root, muse_symlink.rust_root):
        outside = root / "outside-muse-skills"
        outside.mkdir(mode=0o700)
        destination = root / "xdg-config/muse"
        destination.mkdir(parents=True, mode=0o700)
        (destination / "skills").symlink_to(outside)
    outcomes = harness.invoke(
        muse_symlink, ("skill", "install", "--harness", "muse", "--force")
    )
    report.require("primary/skill/muse-symlink-refusal",
                   all(outcome.returncode == 75 for outcome in outcomes)
                   and all(not list((root / "outside-muse-skills").iterdir())
                           for root in (muse_symlink.python_root, muse_symlink.rust_root)),
                   f"Muse skill destination symlink was followed: {outcomes!r}")

    missing_muse = harness.case("primary-skill-muse-missing-executable")
    for root in (missing_muse.python_root, missing_muse.rust_root):
        (root / "fake-muse-skills").unlink()
    outcomes = harness.invoke(missing_muse, ("skill", "install", "--harness", "muse"))
    report.require("primary/skill/muse-missing-executable",
                   all(outcome.returncode == 75 for outcome in outcomes)
                   and all(not (root / "xdg-config/muse/skills").exists()
                           for root in (missing_muse.python_root, missing_muse.rust_root)),
                   f"missing Muse installer mutated configuration: {outcomes!r}")

    for variable, selected in (
        ("AGENTCTL_CODEX_SKILLS_DIR", "codex"),
        ("AGENTCTL_CLAUDE_SKILLS_DIR", "claude"),
        ("AGENTCTL_MUSE_BIN", "muse"),
        ("XDG_CONFIG_HOME", "muse"),
    ):
        empty = harness.case(
            f"primary-skill-empty-{variable.lower()}",
            {"empty_environment": [variable]},
        )
        outcomes = harness.invoke(empty, (
            "skill", "install", "--harness", selected,
        ))
        report.require(f"primary/skill/empty/{variable}",
                       all(outcome.returncode == 75 for outcome in outcomes),
                       f"empty environment override diverged or was accepted: {outcomes!r}")


def _profile_refusals(harness: Harness, report: Report) -> None:
    base = {"schema": "agentctl-profiles/v1", "profiles": {
        "worker": {"harness": "codex", "mode": "interactive", "argv": [], "env": {}}
    }}
    for label in ("nonignored", "permissions", "symlink"):
        case = harness.case(f"primary-profile-refusal-{label}")
        for root in (case.python_root, case.rust_root):
            init_case_repository(root)
            if label != "nonignored":
                (root / ".gitignore").write_text(".agentctl/\n", encoding="utf-8")
            directory = root / ".agentctl"
            directory.mkdir(mode=0o700)
            document = json.loads(json.dumps(base))
            target = directory / "profiles.json"
            if label == "symlink":
                real = directory / "real.json"
                real.write_text(json.dumps(document), encoding="utf-8")
                real.chmod(0o600)
                target.symlink_to(real.name)
            else:
                target.write_text(json.dumps(document), encoding="utf-8")
                target.chmod(0o644 if label == "permissions" else 0o600)
        outcomes = harness.invoke(case, ("profiles", "--cwd", "<ROOT>"))
        report.require(f"primary/profile/refusal/{label}",
                       all(outcome.returncode == 75 and "traceback" not in outcome.stderr.lower()
                           and "panicked" not in outcome.stderr.lower() for outcome in outcomes),
                       f"unsafe profile config was accepted or crashed: {outcomes!r}")

    overlap = harness.case("primary-profile-refusal-overlap")
    for root in (overlap.python_root, overlap.rust_root):
        init_case_repository(root)
        (root / ".gitignore").write_text(".agentctl/\n", encoding="utf-8")
        directory = root / ".agentctl"
        directory.mkdir(mode=0o700)
        config = directory / "profiles.json"
        config.write_text(json.dumps(base), encoding="utf-8")
        config.chmod(0o600)
    outcomes = harness.invoke(overlap, (
        "start", "worker", "--cwd", "<ROOT>", "--profile", "worker", "--model", "override",
        *FIXTURE_HERDR,
    ))
    report.require("primary/profile/refusal/precedence",
                   all(outcome.returncode == 2 for outcome in outcomes),
                   f"profile precedence was ambiguous: {outcomes!r}")
    direct_precedence = (
        ("model-short-attached", "codex", ("--model", "structured"), ("-mother-model",)),
        ("effort-config-attached", "codex", ("--reasoning-effort", "ultra"),
         ("--config=model_reasoning_effort=low",)),
        ("model-quoted-config-key", "codex", ("--model", "structured"),
         ('--config="model"="attacker"',)),
        ("effort-quoted-config-key", "codex", ("--reasoning-effort", "ultra"),
         ('--config="model_reasoning_effort"="low"',)),
        ("model-opaque-config", "codex", ("--model", "structured"),
         ("--config=sandbox_mode=read-only",)),
        ("codex-resume", "codex", ("--resume", "session-1"), ("resume",)),
        ("muse-resume", "muse", ("--resume", "session-1"), ("resume",)),
        ("claude-resume", "claude", ("--resume", "session-1"), ("--continue",)),
    )
    for label, kind, structured, raw in direct_precedence:
        case = harness.case(f"primary-direct-precedence-{label}")
        arguments = [
            "start", "worker", "--cwd", "<ROOT>", "--workspace-id", "w1",
            "--harness", kind, *structured,
        ]
        for item in raw:
            arguments.append(f"--harness-arg={item}")
        outcomes = harness.invoke(case, (*arguments, *_COMMON))
        report.require(f"primary/profile/refusal/direct-{label}",
                       all(outcome.returncode == 75 for outcome in outcomes),
                       f"direct launch precedence was ambiguous: {outcomes!r}")
        report.require(
            f"primary/profile/refusal/direct-{label}-atomic",
            all(not (root / "registry").exists()
                for root in (case.python_root, case.rust_root)),
            "rejected direct launch allocated registry state",
        )

    adopt_muse = harness.case("primary-adopt-muse-mismatched-harness")
    outcomes = harness.invoke(adopt_muse, (
        "adopt", "worker", "--pane", "w1:p1", "--workspace", "project",
        "--cwd", "<ROOT>", "--harness", "muse", *_COMMON,
    ))
    report.require(
        "primary/adopt/muse-mismatched-harness",
        all(outcome.returncode == 75 for outcome in outcomes)
        and all(not (root / "registry/agents/worker").exists()
                for root in (adopt_muse.python_root, adopt_muse.rust_root)),
        f"a different harness was adopted as Muse: {outcomes!r}",
    )

    hostile_path = harness.case("primary-profile-hostile-path", {"hostile_path": True})
    for root in (hostile_path.python_root, hostile_path.rust_root):
        init_case_repository(root)
        (root / ".gitignore").write_text(".agentctl/\n", encoding="utf-8")
        directory = root / ".agentctl"
        directory.mkdir(mode=0o700)
        config = directory / "profiles.json"
        config.write_text(json.dumps(base), encoding="utf-8")
        config.chmod(0o600)
        hostile = root / "hostile-bin"
        hostile.mkdir(mode=0o700)
        fake_git = hostile / "git"
        fake_git.write_text(
            f"#!/bin/sh\ntouch {root / 'hostile-git-executed'}\nexit 0\n",
            encoding="utf-8",
        )
        fake_git.chmod(0o700)
    _pair(harness, report, hostile_path, "primary/profile/hostile-path", (
        "profiles", "--cwd", "<ROOT>",
    ))
    report.require("primary/profile/hostile-path-not-executed", all(
        not (root / "hostile-git-executed").exists()
        for root in (hostile_path.python_root, hostile_path.rust_root)
    ), "profile discovery executed Git from caller PATH")


def _workspace_retirement(harness: Harness, report: Report) -> None:
    """A record started before its project gained a mismatched workspace key stays manageable.

    The goal query names the fixture's goal transport. Without it both editions fall back to the
    default ``codex app-server proxy`` on PATH, which is the developer's real Codex CLI.
    """
    retirement = harness.case("primary-workspace-retirement")
    for root in (retirement.python_root, retirement.rust_root):
        init_case_repository(root)
        (root / ".gitignore").write_text(".agentctl/\n", encoding="utf-8")
        (root / ".agentctl").mkdir(mode=0o700)
    common = ("--herdr-bin", "<HERDR>", "--registry", "<ROOT>/.agentctl")
    _pair(harness, report, retirement, "primary/workspace/retirement/start", (
        "start", "worker", "--cwd", "<ROOT>", "--workspace-id", "w1", *common,
    ))
    for root in (retirement.python_root, retirement.rust_root):
        config = root / ".agentctl/profiles.json"
        config.write_text(json.dumps({
            "schema": "agentctl-profiles/v1",
            "profiles": {"broken": {"harness": "codex"}},
            "workspace": "required-label",
        }), encoding="utf-8")
        config.chmod(0o600)
    _pair(harness, report, retirement, "primary/workspace/retirement/status", (
        "status", "worker", *common,
    ))
    _pair(harness, report, retirement, "primary/workspace/retirement/attach", (
        "attach", "worker", *common,
    ))
    python, _ = _pair(harness, report, retirement, "primary/workspace/retirement/goal-query", (
        "goal", "worker", *_GOAL_COMMAND, *common,
    ))
    goal = _json(python)
    report.require(
        "primary/workspace/retirement/goal-query-native",
        isinstance(goal, dict) and goal.get("source") == "native"
        and goal.get("native_status") == "absent" and "native_error" not in goal,
        f"the goal query did not reach the fixture's native goal transport: {goal!r}",
    )
    _pair(harness, report, retirement, "primary/workspace/retirement/wait", (
        "wait", "worker", "--timeout", "0", *common,
    ))
    _pair(harness, report, retirement, "primary/workspace/retirement/stop", (
        "stop", "worker", *common,
    ))


def _workspace_policy(harness: Harness, report: Report) -> None:
    """Pin the paired CLI surface for the optional project workspace key."""
    base = {"schema": "agentctl-profiles/v1", "profiles": {
        "worker": {"harness": "codex", "mode": "interactive", "argv": [], "env": {}}
    }}
    documents: dict[str, object] = {
        "valid": {"schema": "agentctl-profiles/v1", "profiles": {}, "workspace": "project"},
        "null": {"schema": "agentctl-profiles/v1", "profiles": {}, "workspace": None},
        "invalid": {"schema": "agentctl-profiles/v1", "profiles": {}, "workspace": " bad "},
    }
    for label, document in documents.items():
        case = harness.case(f"primary-workspace-{label}")
        for root in (case.python_root, case.rust_root):
            init_case_repository(root)
            (root / ".gitignore").write_text(".agentctl/\n", encoding="utf-8")
            directory = root / ".agentctl"
            directory.mkdir(mode=0o700)
            config = directory / "profiles.json"
            config.write_text(json.dumps(document), encoding="utf-8")
            config.chmod(0o600)
        _pair(
            harness, report, case, f"primary/workspace/{label}/profiles",
            ("profiles", "--cwd", "<ROOT>"), 75 if label == "invalid" else 0,
        )

    malformed = harness.case("primary-workspace-malformed-unrelated-profile")
    for root in (malformed.python_root, malformed.rust_root):
        init_case_repository(root)
        (root / ".gitignore").write_text(".agentctl/\n", encoding="utf-8")
        directory = root / ".agentctl"
        directory.mkdir(mode=0o700)
        config = directory / "profiles.json"
        config.write_text(json.dumps({
            "schema": "agentctl-profiles/v1", "workspace": "project",
            "profiles": {"broken": {"harness": "codex"}},
        }), encoding="utf-8")
        config.chmod(0o600)
    _pair(harness, report, malformed, "primary/workspace/malformed/list", (
        "list", "--herdr-bin", "<HERDR>", "--registry", "<ROOT>/.agentctl",
    ))

    duplicate_workspace = harness.case("primary-workspace-duplicate-key")
    for root in (duplicate_workspace.python_root, duplicate_workspace.rust_root):
        init_case_repository(root)
        (root / ".gitignore").write_text(".agentctl/\n", encoding="utf-8")
        directory = root / ".agentctl"
        directory.mkdir(mode=0o700)
        config = directory / "profiles.json"
        config.write_text(
            '{"schema":"agentctl-profiles/v1","workspace":"first",'
            '"workspace":"second","profiles":{"broken":{"harness":"codex"}}}',
            encoding="utf-8",
        )
        config.chmod(0o600)
    _pair(
        harness, report, duplicate_workspace,
        "primary/workspace/duplicate-workspace-key",
        ("profiles", "--cwd", "<ROOT>"),
        75,
    )

    mismatch = harness.case("primary-workspace-explicit-mismatch")
    for root in (mismatch.python_root, mismatch.rust_root):
        init_case_repository(root)
        (root / ".gitignore").write_text(".agentctl/\n", encoding="utf-8")
        directory = root / ".agentctl"
        directory.mkdir(mode=0o700)
        config = directory / "profiles.json"
        config.write_text(json.dumps({
            "schema": "agentctl-profiles/v1",
            "profiles": {"broken": {"harness": "codex"}},
            "workspace": "required-label",
        }), encoding="utf-8")
        config.chmod(0o600)
    outcomes = harness.invoke(mismatch, _with_effort((
        "start", "worker", "--cwd", "<ROOT>", "--workspace-id", "w1",
        "--herdr-bin", "<HERDR>", "--registry", "<ROOT>/.agentctl",
    )))
    report.require(
        "primary/workspace/explicit-mismatch",
        all(outcome.returncode == 75 and "project configuration requires" in outcome.stderr
            for outcome in outcomes)
        and all(not (root / ".agentctl/agents/worker").exists()
                for root in (mismatch.python_root, mismatch.rust_root)),
        f"workspace mismatch diverged or allocated a record: {outcomes!r}",
    )

    _workspace_retirement(harness, report)

    move = harness.case("primary-workspace-move")
    for root in (move.python_root, move.rust_root):
        init_case_repository(root)
        (root / ".gitignore").write_text(".agentctl/\n", encoding="utf-8")
        (root / ".agentctl").mkdir(mode=0o700)
    move_common = ("--herdr-bin", "<HERDR>", "--registry", "<ROOT>/.agentctl")
    _pair(harness, report, move, "primary/workspace/move/start", (
        "start", "worker", "--cwd", "<ROOT>", "--workspace-id", "w1",
        *move_common,
    ))
    for root in (move.python_root, move.rust_root):
        config = root / ".agentctl/profiles.json"
        config.write_text(json.dumps({
            "schema": "agentctl-profiles/v1",
            "workspace": "project-agents",
            "profiles": {},
        }), encoding="utf-8")
        config.chmod(0o600)
    _pair(harness, report, move, "primary/workspace/move/command", (
        "move", "worker", *move_common,
    ))
    _pair(harness, report, move, "primary/workspace/move/status", (
        "status", "worker", *move_common,
    ))
    report.require(
        "primary/workspace/move/identity",
        all(_state(root).get("pane_id") == "w-project:p1"
            and _state(root).get("workspace_id") == "w-project"
            for root in (move.python_root, move.rust_root)),
        "move did not commit the destination Herdr identity",
    )

    strict_documents = {
        "duplicate-json": (
            '{"schema":"agentctl-profiles/v1","profiles":{'
            '"worker":{"harness":"codex","mode":"interactive"},'
            '"worker":{"harness":"muse","mode":"interactive"}}}'
        ),
        "reserved-headless": json.dumps({"schema": "agentctl-profiles/v1", "profiles": {
            "worker": {"harness": "muse", "mode": "headless",
                       "argv": ["--session-id=attacker"], "env": {}}
        }}),
        "headless-option-terminator": json.dumps({"schema": "agentctl-profiles/v1", "profiles": {
            "worker": {"harness": "muse", "mode": "headless", "argv": ["--"], "env": {}}
        }}),
        "headless-secret-stdin": json.dumps({"schema": "agentctl-profiles/v1", "profiles": {
            "worker": {"harness": "muse", "mode": "headless",
                       "argv": ["--api-key-stdin"], "env": {}}
        }}),
        "unsupported-combination": json.dumps({"schema": "agentctl-profiles/v1", "profiles": {
            "worker": {"harness": "claude", "mode": "headless", "argv": [], "env": {}}
        }}),
        "unsupported-interactive-agy": json.dumps({"schema": "agentctl-profiles/v1", "profiles": {
            "worker": {"harness": "agy", "mode": "interactive", "argv": [], "env": {}}
        }}),
        "unsupported-headless-codex-argv": json.dumps({"schema": "agentctl-profiles/v1", "profiles": {
            "worker": {"harness": "codex", "mode": "headless",
                       "argv": ["--search"], "env": {}}
        }}),
        "unsupported-headless-codex-effort": json.dumps({"schema": "agentctl-profiles/v1", "profiles": {
            "worker": {"harness": "codex", "mode": "headless",
                       "reasoning_effort": "high", "argv": [], "env": {}}
        }}),
        "unsupported-headless-agy-effort": json.dumps({"schema": "agentctl-profiles/v1", "profiles": {
            "worker": {"harness": "agy", "mode": "headless",
                       "reasoning_effort": "high", "argv": [], "env": {}}
        }}),
        "unsupported-headless-environment": json.dumps({"schema": "agentctl-profiles/v1", "profiles": {
            "worker": {"harness": "muse", "mode": "headless", "argv": [],
                       "env": {"ROUTING_SELECTOR": "route"}}
        }}),
        "codex-model-short-attached": json.dumps({"schema": "agentctl-profiles/v1", "profiles": {
            "worker": {"harness": "codex", "mode": "interactive", "model": "structured",
                       "argv": ["-mother-model"], "env": {}}
        }}),
        "codex-model-config-attached": json.dumps({"schema": "agentctl-profiles/v1", "profiles": {
            "worker": {"harness": "codex", "mode": "interactive", "model": "structured",
                       "argv": ['--config= model = "other"'], "env": {}}
        }}),
        "codex-effort-config-short": json.dumps({"schema": "agentctl-profiles/v1", "profiles": {
            "worker": {"harness": "codex", "mode": "interactive", "reasoning_effort": "ultra",
                       "argv": ['-c=model_reasoning_effort = "low"'], "env": {}}
        }}),
        "codex-effort-config-split": json.dumps({"schema": "agentctl-profiles/v1", "profiles": {
            "worker": {"harness": "codex", "mode": "interactive", "reasoning_effort": "ultra",
                       "argv": ["--config", ' model_reasoning_effort = "low"'], "env": {}}
        }}),
        "codex-model-config-quoted": json.dumps({"schema": "agentctl-profiles/v1", "profiles": {
            "worker": {"harness": "codex", "mode": "interactive", "model": "structured",
                       "argv": ["--config", '\"model\"=\"other\"'], "env": {}}
        }}),
        "codex-effort-config-quoted": json.dumps({"schema": "agentctl-profiles/v1", "profiles": {
            "worker": {"harness": "codex", "mode": "interactive", "reasoning_effort": "ultra",
                       "argv": ['-c"model_reasoning_effort"="low"'], "env": {}}
        }}),
        "codex-opaque-config-profile": json.dumps({"schema": "agentctl-profiles/v1", "profiles": {
            "worker": {"harness": "codex", "mode": "interactive", "model": "structured",
                       "argv": ["-powner-defaults"], "env": {}}
        }}),
        "oversized": json.dumps(base) + (" " * (256 * 1024)),
    }
    for label, content in strict_documents.items():
        case = harness.case(f"primary-profile-strict-{label}")
        for root in (case.python_root, case.rust_root):
            init_case_repository(root)
            (root / ".gitignore").write_text(".agentctl/\n", encoding="utf-8")
            directory = root / ".agentctl"
            directory.mkdir(mode=0o700)
            config = directory / "profiles.json"
            config.write_text(content, encoding="utf-8")
            config.chmod(0o600)
        outcomes = harness.invoke(case, ("profiles", "--cwd", "<ROOT>"))
        report.require(f"primary/profile/strict/{label}",
                       all(outcome.returncode == 75 for outcome in outcomes),
                       f"ambiguous or unbounded profile was accepted: {outcomes!r}")

    missing = harness.case(
        "primary-custom-missing",
        {"missing_custom_executable": True, "empty_shell": True},
    )
    outcomes = harness.invoke(missing, _with_effort((
        "start", "worker", "--cwd", "<ROOT>", "--workspace-id", "w1",
        "--harness", "muse", *_COMMON,
    )))
    report.require("primary/custom/missing-executable",
                   all(outcome.returncode == 75
                   and "cannot inspect muse executable" in outcome.stderr
                       for outcome in outcomes),
                   f"missing custom executable was not refused: {outcomes!r}")
    stopped = harness.invoke(missing, ("stop", "worker", *_COMMON))
    report.require("primary/custom/missing-executable-retained",
                   all(outcome.returncode == 69 for outcome in stopped)
                   and all((root / "registry/agents/worker/agent.json").is_file()
                           and not _state(root).get("closed")
                           for root in (missing.python_root, missing.rust_root)),
                   f"unobserved missing-executable launch was incorrectly retired: {stopped!r}")

    wrong_process = harness.case(
        "primary-custom-wrong-process", {"wrong_custom_process": True}
    )
    outcomes = harness.invoke(wrong_process, _with_effort((
        "start", "worker", "--cwd", "<ROOT>", "--workspace-id", "w1",
        "--harness", "muse", "--startup-timeout", "0.05", *_COMMON,
    )))
    report.require("primary/custom/exact-process",
                   all(outcome.returncode == 75 and "foreground process" in outcome.stderr
                       for outcome in outcomes),
                   f"lookalike custom process was accepted: {outcomes!r}")
    stopped = harness.invoke(wrong_process, ("stop", "worker", *_COMMON))
    report.require("primary/custom/exact-process-retained",
                   all(outcome.returncode == 69 for outcome in stopped)
                   and all((root / "registry/agents/worker/agent.json").is_file()
                           and not _state(root).get("closed")
                           for root in (wrong_process.python_root, wrong_process.rust_root)),
                   f"unobserved wrong-process launch was incorrectly retired: {stopped!r}")
    _retire_fixture_processes(wrong_process, report)

    wrong_identity = harness.case(
        "primary-custom-wrong-kernel-identity", {"wrong_custom_process_identity": True}
    )
    outcomes = harness.invoke(wrong_identity, _with_effort((
        "start", "worker", "--cwd", "<ROOT>", "--workspace-id", "w1",
        "--harness", "muse", "--startup-timeout", "0.05", *_COMMON,
    )))
    report.require("primary/custom/kernel-process-identity",
                   all(outcome.returncode == 75 and "foreground process" in outcome.stderr
                       for outcome in outcomes),
                   f"self-reported executable bypassed kernel identity: {outcomes!r}")
    stopped = harness.invoke(wrong_identity, ("stop", "worker", *_COMMON))
    report.require("primary/custom/kernel-process-retained",
                   all(outcome.returncode == 69 for outcome in stopped)
                   and all((root / "registry/agents/worker/agent.json").is_file()
                           and not _state(root).get("closed")
                           for root in (wrong_identity.python_root, wrong_identity.rust_root)),
                   f"unobserved wrong-kernel launch was incorrectly retired: {stopped!r}")
    _retire_fixture_processes(wrong_identity, report)

    trust = harness.case("primary-muse-trust", {"screen": "Do you trust this workspace? yes / no"})
    outcomes = harness.invoke(trust, _with_effort((
        "start", "worker", "--cwd", "<ROOT>", "--workspace-id", "w1",
        "--harness", "muse", *_COMMON,
    )))
    report.require("primary/muse/trust-refusal",
                   all(outcome.returncode == 75 and "trust prompt" in outcome.stderr
                       and _submitted(root) == []
                       for outcome, root in zip(outcomes, (trust.python_root, trust.rust_root), strict=True)),
                   f"Muse trust prompt was accepted or mutated: {outcomes!r}")
    stopped = harness.invoke(trust, ("stop", "worker", *_COMMON))
    report.require("primary/muse/failed-start-cleanup",
                   all(outcome.returncode == 0 for outcome in stopped)
                   and all(_state(root).get("closed_panes") == [_fixture_identity(root)[0]]
                           and not (root / "registry/agents/worker").exists()
                           and len(list((root / "registry/archive").iterdir())) == 1
                           for root in (trust.python_root, trust.rust_root)),
                   f"retained failed Muse launch was not safely stoppable: {stopped!r}")

    exited = harness.case("primary-muse-exit-after-send-text")
    if _start(
        harness, report, exited, "primary/muse/exit-after-send-text/start",
        "--harness", "muse",
    ):
        _change(exited, {"custom_exit_after_send_text": True})
        outcomes = harness.invoke(exited, (
            "send", "worker", "must never reach a replacement shell",
            "--message-id", "exit-race", *_COMMON,
        ))
        report.require(
            "primary/muse/exit-after-send-text/no-enter",
            all(outcome.returncode == 76 for outcome in outcomes)
            and all(
                _state(root).get("custom_submitted") is False
                and _submitted(root) == []
                and (root / "registry/agents/worker/queue/failed/exit-race.json").is_file()
                for root in (exited.python_root, exited.rust_root)
            ),
            f"Muse exit between text and Enter submitted to a replacement: {outcomes!r}",
        )
        _retire_fixture_processes(exited, report)


def _herdr_093_compatibility(harness: Harness, report: Report) -> None:
    """Exercise the 0.9.3 startup envelope and missing optional process argv."""
    unknown: dict[str, object] = {
        "status": "unknown", "startup_timeout": True, "empty_shell": True,
        "codex_footer": "  model default · /work/project",
        "foreground_argv_mode": "missing",
    }
    case = harness.case("primary-herdr-093-codex", unknown)
    # Success-expecting start (#1249 cross-differentials-robust): no
    # harness budget is passed, so the product default (30 s) bounds the
    # wait on fixture state. The removed calibration scaled a 0.1 s
    # nominal by measured spawn latency; verdicts now depend only on
    # the startup proof outcome, never on host spawn speed.
    started, _ = _pair(harness, report, case, "primary/herdr-093/codex/start", (
        "start", "worker", "--cwd", "<ROOT>", "--workspace-id", "w1",
        "--brief", "compatibility brief", *_COMMON,
    ))
    status = _json(started)
    report.require("primary/herdr-093/codex/raw-status-and-once",
                   isinstance(status, dict) and status.get("lifecycle") == "running"
                   and status.get("agent_status") == "unknown"
                   and all(_submission_count(root, "compatibility brief") == 1
                           and sum(call[:2] == ["agent", "start"]
                                   for call in _fixture_calls(root)) == 1
                           for root in (case.python_root, case.rust_root)),
                   f"unknown Codex was relabeled, relaunched, or lost its brief: {status!r}")
    if started.returncode == 0:
        _change(case, {"composer_busy": True})
        _pair(harness, report, case, "primary/herdr-093/codex/busy-pending", (
            "send", "worker", "compatibility follow-up", "--message-id", "compat-next",
            "--ready-timeout", "0", *_COMMON,
        ), 75)
        _pair(harness, report, case, "primary/herdr-093/codex/busy-drain", (
            "drain", "worker", "--ready-timeout", "0", *_COMMON,
        ), 75)
        report.require("primary/herdr-093/codex/busy-no-input", all(
            _submission_count(root, "compatibility follow-up") == 0
            and (root / "registry/agents/worker/queue/inbox/compat-next.json").is_file()
            for root in (case.python_root, case.rust_root)),
            "an unknown busy composer received a queued follow-up")
        _change(case, {"composer_busy": False})
        _pair(harness, report, case, "primary/herdr-093/codex/idle-drain", (
            "drain", "worker", "--ready-timeout", "0", *_COMMON,
        ))
        report.require("primary/herdr-093/codex/drain-once", all(
            _submission_count(root, "compatibility follow-up") == 1
            and (root / "registry/agents/worker/queue/processed/compat-next.json").is_file()
            for root in (case.python_root, case.rust_root)),
            "idle unknown Codex did not acknowledge the queued follow-up exactly once")
        _pair(harness, report, case, "primary/herdr-093/codex/stop", (
            "stop", "worker", *_COMMON,
        ))
    else:
        _retire_fixture_processes(case, report)

    refused: tuple[tuple[str, Mapping[str, object]], ...] = (
        ("busy", {"composer_busy": True}),
        ("hidden-interrupt", {"composer_busy": True, "codex_activity": "• Exploring (7s)"}),
        ("padded-hidden-interrupt", {"composer_busy": True,
                                    "codex_activity": "Working (7s)   "}),
        ("annotated-hidden-interrupt", {"composer_busy": True,
                                       "codex_activity": "Processing inputs (7s) · command (background)"}),
        ("remapped-interrupt", {"composer_busy": True,
                               "codex_activity": "• Exploring (1m 2s • Ctrl+c to interrupt)"}),
        ("queued-turn", {"composer_busy": True,
                         "codex_activity": "• Messages to be submitted at end of turn"}),
        ("grouped-menu", {"composer_busy": True,
                          "codex_activity": "All Results   Filesystem Only   Plugins"}),
        ("draft", {"composer_draft": "operator draft"}),
        ("unrecognized-footer", {"codex_footer": "unrecognized screen"}),
        ("terminal-swap", {"terminal_on_visible_read": "term-replacement"}),
        ("other-error", {"startup_error": {
            "id": "cli:agent:start", "error": {
                "code": "agent_not_ready", "message": "timed out waiting for agent startup",
            },
        }}),
        ("wrong-id", {"startup_error": {
            "id": "cli:request", "error": {
                "code": "timeout", "message": "timed out waiting for agent startup",
            },
        }}),
    )
    for label, overrides in refused:
        failed = harness.case(f"primary-herdr-093-codex-{label}", {**unknown, **overrides})
        _pair(harness, report, failed, f"primary/herdr-093/codex/refuse-{label}", (
            "start", "worker", "--cwd", "<ROOT>", "--workspace-id", "w1",
            "--startup-timeout", "0.1", "--brief", "must remain unsent", *_COMMON,
        ), 75)
        report.require(f"primary/herdr-093/codex/refuse-{label}-atomic", all(
            _submitted(root) == []
            and sum(call[:2] == ["agent", "start"]
                    for call in _fixture_calls(root)) == 1
            and json.loads((root / "registry/agents/worker/agent.json").read_text(
                encoding="utf-8")).get("lifecycle") == "launch_failed"
            for root in (failed.python_root, failed.rust_root)),
            f"{label} caused a startup retry, input, or a running record")
        _retire_fixture_processes(failed, report)

    for mode in ("missing", "null", "empty"):
        muse = harness.case(f"primary-herdr-093-muse-{mode}", {
            "foreground_argv_mode": mode,
        })
        if _start(harness, report, muse, f"primary/herdr-093/muse/{mode}/start",
                  "--harness", "muse"):
            report.require(f"primary/herdr-093/muse/{mode}/kernel-pin", all(
                _valid_foreign_shell_identity(json.loads(
                    (root / "registry/agents/worker/agent.json").read_text(encoding="utf-8")
                ).get("custom_process_identity"))
                for root in (muse.python_root, muse.rust_root)),
                "missing argv discarded the custom harness kernel identity")
            _pair(harness, report, muse, f"primary/herdr-093/muse/{mode}/send", (
                "send", "worker", "optional argv delivery", "--message-id", "optional-argv",
                *_COMMON,
            ))
            report.require(f"primary/herdr-093/muse/{mode}/receipt", all(
                _submission_count(root, "optional argv delivery") == 1
                and _state(root).get("paste_wrapped") is True
                and (root / "registry/agents/worker/queue/processed/optional-argv.json").is_file()
                for root in (muse.python_root, muse.rust_root)),
                "missing argv bypassed verified Muse delivery")
            _pair(harness, report, muse, f"primary/herdr-093/muse/{mode}/stop", (
                "stop", "worker", *_COMMON,
            ))
        else:
            _retire_fixture_processes(muse, report)

    malformed: tuple[tuple[str, Mapping[str, object]], ...] = (
        ("wrong-kernel", {"foreground_argv_mode": "missing",
                          "wrong_custom_process_identity": True}),
        ("argv-object", {"foreground_argv_override": {"0": "muse"}}),
        ("argv-nonstring", {"foreground_argv_override": ["muse", 42]}),
        ("argv-empty-first", {"foreground_argv_override": [""]}),
    )
    for label, state in malformed:
        failed = harness.case(f"primary-herdr-093-muse-{label}", state)
        outcomes = harness.invoke(failed, _with_effort((
            "start", "worker", "--cwd", "<ROOT>", "--workspace-id", "w1",
            "--harness", "muse", "--startup-timeout", "0.05", *_COMMON,
        )))
        # No cross-edition rc equality here (#1067
        # cross-differential-load-aware): whether the malformed argv is
        # read before the 0.05 startup expiry decides 69 vs 75, so load
        # splits the failure class per edition without splitting the
        # refusal - each edition must still refuse with launch_failed.
        report.require(f"primary/herdr-093/muse/refuse-{label}",
                       all(outcome.returncode in (69, 75) for outcome in outcomes)
                       and all(_submitted(root) == []
                               and json.loads((root / "registry/agents/worker/agent.json").read_text(
                                   encoding="utf-8")).get("lifecycle") == "launch_failed"
                               for root in (failed.python_root, failed.rust_root)),
                       f"{label} became a verified Muse launch: {outcomes!r}")
        _retire_fixture_processes(failed, report)


def _muse_adoption(harness: Harness, report: Report) -> None:
    """A foreign Muse keeps its runtime and shares verified composer delivery."""
    failures: tuple[tuple[str, Mapping[str, object]], ...] = (
        ("missing-process", {"custom_identity_hidden": True}),
        ("wrong-process-group", {"wrong_custom_process_group": True}),
        ("missing-terminal", {"terminal_id": ""}),
    )
    for label, overrides in failures:
        refused = harness.case(f"primary-muse-adoption-{label}", {
            "harness": "muse", "custom_reported": True, "report_custom_session": True,
            "foreground_argv_mode": "missing", **overrides,
        })
        for root in (refused.python_root, refused.rust_root):
            subprocess.run([
                str(root / "fake-herdr"), "pane", "run", _fixture_identity(root)[0],
                shlex.join([str(root / "fake-muse-runtime")]),
            ], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        _pair(harness, report, refused, f"primary/adopt/muse/refuse-{label}", (
            "adopt", "foreign", "--pane", "w1:p1", "--workspace", "project",
            "--cwd", "<ROOT>", "--harness", "muse", "--session", "session-1", *_COMMON,
        ), 75)
        report.require(f"primary/adopt/muse/refuse-{label}-atomic", all(
            not (root / "registry/agents/foreign").exists()
            and not _state(root).get("closed")
            and not any(call[:2] in (["agent", "prompt"], ["pane", "send-text"],
                                     ["pane", "send-keys"], ["pane", "report-agent"],
                                     ["agent", "rename"], ["tab", "rename"])
                        for call in _fixture_calls(root))
            for root in (refused.python_root, refused.rust_root)),
            "unanchored Muse adoption saved a record or changed the foreign runtime")
        _retire_fixture_processes(refused, report)
    for label, with_session in (("native-session", True), ("sessionless", False)):
        case = harness.case(f"primary-muse-adoption-{label}", {
            "harness": "muse", "custom_reported": True,
            "report_custom_session": with_session, "foreground_argv_mode": "missing",
        })
        for root in (case.python_root, case.rust_root):
            subprocess.run([
                str(root / "fake-herdr"), "pane", "run", _fixture_identity(root)[0],
                shlex.join([str(root / "fake-muse-runtime")]),
            ], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        session = ("--session", "session-1") if with_session else ()
        adopted, _ = _pair(harness, report, case, f"primary/adopt/muse/{label}/register", (
            "adopt", "foreign", "--pane", "w1:p1", "--workspace", "project",
            "--cwd", "<ROOT>", "--harness", "muse", *session, *_COMMON,
        ))
        record = _json(adopted)
        report.require(f"primary/adopt/muse/{label}/anchors", isinstance(record, dict)
                       and record.get("adapter") == "herdr-foreign"
                       and isinstance(record.get("harness_identity"), dict)
                       and _valid_foreign_shell_identity(record.get("foreign_shell_identity"))
                       and all(json.loads((root / "registry/agents/foreign/agent.json").read_text(
                           encoding="utf-8")).get("custom_process_identity") is None
                           for root in (case.python_root, case.rust_root)),
                       f"adoption changed ownership or omitted mandatory Muse pins: {record!r}")
        if adopted.returncode != 0:
            _retire_fixture_processes(case, report)
            continue
        _pair(harness, report, case, f"primary/adopt/muse/{label}/send", (
            "send", "foreign", "foreign Muse literal mentions Auto-review\nsecond line",
            "--message-id", "foreign-muse",
            *_COMMON,
        ))
        report.require(f"primary/adopt/muse/{label}/guarded-receipt", all(
            _submission_count(root, "foreign Muse literal mentions Auto-review\nsecond line") == 1
            and _state(root).get("paste_wrapped") is True
            and (root / "registry/agents/foreign/queue/processed/foreign-muse.json").is_file()
            and not any(call[:2] in (["agent", "prompt"], ["pane", "report-agent"],
                                     ["agent", "rename"], ["tab", "rename"])
                        for call in _fixture_calls(root))
            for root in (case.python_root, case.rust_root)),
            "foreign Muse bypassed staged input or changed runtime ownership")
        _change(case, {"custom_permission": "YOLO", "custom_submitted": False,
                       "custom_draft": ""})
        # Owner-directed: an adopted Muse pane takes input whatever its footer's approval mode,
        # so an adopted coordinator in YOLO, the owner's normal launch mode, still receives
        # messages, through the same staged-editor and on-screen submission proofs.
        _pair(harness, report, case, f"primary/adopt/muse/{label}/yolo-delivery", (
            "send", "foreign", "an adopted YOLO pane takes input", "--message-id",
            "foreign-yolo", "--ready-timeout", "0", *_COMMON,
        ))
        report.require(f"primary/adopt/muse/{label}/yolo-input", all(
            _submission_count(root, "an adopted YOLO pane takes input") == 1
            and (root / "registry/agents/foreign/queue/processed/foreign-yolo.json").is_file()
            for root in (case.python_root, case.rust_root)),
            "foreign Muse delivery refused a YOLO footer")
        _change(case, {"custom_permission": "Auto-review", "custom_submitted": False,
                       "custom_draft": "", "fixture_shell_pid": harness.replacement_shell.pid})
        outcomes = harness.invoke(case, ("send", "foreign", "must not cross shell generation",
                                         "--message-id", "foreign-replacement", *_COMMON))
        report.require(f"primary/adopt/muse/{label}/shell-replacement-refusal",
                       all(outcome.returncode == 75 for outcome in outcomes)
                       and all(_submission_count(root, "must not cross shell generation") == 0
                               for root in (case.python_root, case.rust_root)),
                       f"foreign Muse input crossed a shell generation: {outcomes!r}")
        _change(case, {"fixture_shell_pid": harness.fixture_shell.pid})
        stopped, _ = _pair(harness, report, case, f"primary/adopt/muse/{label}/unregister", (
            "stop", "foreign", *_COMMON,
        ))
        result = _json(stopped)
        report.require(f"primary/adopt/muse/{label}/runtime-preserved",
                       isinstance(result, dict) and result.get("runtime_preserved") is True
                       and all(not _state(root).get("closed")
                               and not (root / "registry/agents/foreign").exists()
                               and len(list((root / "registry/archive").glob("*/agent.json"))) == 1
                               for root in (case.python_root, case.rust_root)),
                       f"foreign Muse unregister closed or discarded its runtime: {result!r}")
        _retire_fixture_processes(case, report)


def _muse14_status(harness: Harness, report: Report) -> None:
    """Muse 1.4 frames read as idle, staged or working alike; a paused goal reads idle and takes input."""
    case = harness.case("primary-muse14-status", {
        "harness": "muse", "custom_reported": True, "report_custom_session": True,
        "foreground_argv_mode": "missing",
    })
    for root in (case.python_root, case.rust_root):
        subprocess.run([
            str(root / "fake-herdr"), "pane", "run", _fixture_identity(root)[0],
            shlex.join([str(root / "fake-muse-runtime")]),
        ], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    adopted, _ = _pair(harness, report, case, "primary/muse14/adopt", (
        "adopt", "foreign", "--pane", "w1:p1", "--workspace", "project",
        "--cwd", "<ROOT>", "--harness", "muse", "--session", "session-1", *_COMMON,
    ))
    if adopted.returncode != 0:
        _retire_fixture_processes(case, report)
        return
    divider, footer = "─" * 40 + "\n", "kiki · xhigh · /work · YOLO\n"
    idle = f"old transcript\n{divider}❯\n{divider}{footer}"
    staged, paused = idle.replace("❯\n", "❯ owner draft\n"), idle.replace(footer, "Goal (paused)\n" + footer)
    headed = "Muse Code 1.4.0\n" + paused.replace("YOLO", "Auto-review")
    headed_staged = "Muse Code 1.4.0\n" + staged.replace("YOLO", "Auto-review")
    frames = (("idle", "done", idle, "idle"), ("staged", "done", staged, "staged"),
              ("paused", "done", paused, "idle"), ("echo", "done", "Goal (paused)\n" + idle, "idle"),
              ("busy-headerless", "working", paused, "working"), ("busy-headed", "working", headed, "idle"),
              ("headed-paused", "done", headed, "idle"), ("headed-staged", "idle", headed_staged, "staged"))
    for label, status, screen, expected in frames:
        _change(case, {"status": status, "screen": screen})
        outcomes = _pair(harness, report, case, f"primary/muse14/{label}/status",
                         ("status", "foreign", *_COMMON))
        report.require(f"primary/muse14/{label}/agent-status", all(
            isinstance(value := _json(outcome), dict) and value.get("agent_status") == expected
            for outcome in outcomes), f"expected {expected}: {outcomes!r}")
    # Only a staged editor is held here: a paused goal reads idle (above) and takes
    # input, which the unit fakes that reflect typing prove (#734
    # chat-status-disagreement); this fixture's static frame cannot show a paste.
    _change(case, {"status": "done", "screen": headed_staged})
    _pair(harness, report, case, "primary/muse14/staged/held", (
        "send", "foreign", "held while staged", "--message-id", "0-muse14-staged",
        "--ready-timeout", "0", *_COMMON,
    ), 75)
    for root in (case.python_root, case.rust_root):
        path = root / "registry/agents/foreign/queue/inbox" / "0-muse14-staged.json"
        document = json.loads(path.read_text())
        document["queued_at"] = 101.0
        path.write_text(json.dumps(document) + "\n")
    report.require("primary/muse14/held-without-input", all(
        len(list((root / "registry/agents/foreign/queue/inbox").glob("*.json"))) == 1
        for root in (case.python_root, case.rust_root)), "a staged Muse editor took input")
    _change(case, {"status": "idle", "screen": ""})
    # The drain's recipient is the stand-in Muse this case launched. One that is gone is a
    # fixture failure, not a difference between the editions, and says so (#1002).
    gone = [edition for edition, root in (("Python", case.python_root), ("Rust", case.rust_root))
            if not _fixture_process_alive(_state(root).get("custom_pid"))]
    report.require("primary/muse14/fixture-alive-before-drain", not gone,
                   f"fixture error in primary/muse14: the stand-in Muse of the {' and '.join(gone)} "
                   "edition exited before the drain, so the drain compared nothing")
    if gone:
        _retire_fixture_processes(case, report)
        return
    _pair(harness, report, case, "primary/muse14/resumed-drain", ("drain", "foreign", *_COMMON))
    report.require("primary/muse14/delivered-once-in-order", all(
        _submitted(root) == ["held while staged"]
        for root in (case.python_root, case.rust_root)), "held Muse input was lost, repeated or reordered")
    _retire_fixture_processes(case, report)


def _muse_busy_followup(harness: Harness, report: Report) -> None:
    """#505 agentctl-muse-followup: an EXPECTED divergence, checked per edition.

    For a Muse that is running a turn, the Rust edition pastes an ordinary prompt into the empty
    editor and steers the turn with Enter, proved by the new user turn. By the owner's 2026-10-25
    decision this is not ported to Python, which keeps the prompt pending until the editor is
    idle and then delivers it. Neither edition may type it twice.
    """
    case = harness.case("primary-muse-busy-followup", {
        "harness": "muse", "custom_reported": True, "report_custom_session": True,
        "foreground_argv_mode": "missing",
    })
    for root in (case.python_root, case.rust_root):
        subprocess.run([
            str(root / "fake-herdr"), "pane", "run", _fixture_identity(root)[0],
            shlex.join([str(root / "fake-muse-runtime")]),
        ], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    adopted, _ = _pair(harness, report, case, "primary/muse-busy/adopt", (
        "adopt", "foreign", "--pane", "w1:p1", "--workspace", "project",
        "--cwd", "<ROOT>", "--harness", "muse", "--session", "session-1", *_COMMON,
    ))
    if adopted.returncode != 0:
        _retire_fixture_processes(case, report)
        return
    _change(case, {"status": "working", "custom_busy": True})
    prompt = "hold: stop after the current step"
    python, rust = harness.invoke(case, (
        "send", "foreign", prompt, "--message-id", "busy", "--ready-timeout", "0", *_COMMON,
    ))
    report.require("primary/muse-busy/python-waits-for-idle",
                   python.returncode == 75
                   and _submission_count(case.python_root, prompt) == 0
                   and (case.python_root / "registry/agents/foreign/queue/inbox/busy.json").is_file(),
                   f"Python typed into a busy Muse or lost the prompt: {python!r}")
    report.require("primary/muse-busy/rust-steers-once",
                   rust.returncode == 0
                   and _submission_count(case.rust_root, prompt) == 1
                   and _state(case.rust_root).get("paste_wrapped") is True
                   and (case.rust_root / "registry/agents/foreign/queue/processed/busy.json").is_file(),
                   f"Rust did not steer the busy Muse exactly once: {rust!r}")
    _change(case, {"status": "idle", "custom_busy": False, "custom_submitted": False,
                   "custom_draft": ""})
    harness.invoke(case, ("drain", "foreign", *_COMMON))
    report.require("primary/muse-busy/delivered-once-each",
                   all(_submission_count(root, prompt) == 1
                       and (root / "registry/agents/foreign/queue/processed/busy.json").is_file()
                       for root in (case.python_root, case.rust_root)),
                   "the idle drain lost or repeated the busy-time prompt")
    _retire_fixture_processes(case, report)


def _handoff_and_pending(harness: Harness, report: Report) -> None:
    case = harness.case("primary-handoff")
    if not _start(harness, report, case, "primary/handoff/start"):
        return
    _change(case, {"status": "working", "screen": "unrecognized terminal layout\n"})
    _pair(harness, report, case, "primary/handoff/busy-pending",
          ("send", "worker", "queued before handoff", "--message-id", "waiting", "--ready-timeout", "0", *_COMMON), 75)
    _pair(harness, report, case, "primary/handoff/pause", ("pause", "worker", *_COMMON))
    for command in (("send", "worker", "refuse while paused"), ("drain", "worker"), ("goal", "worker", "refuse new goal")):
        _pair(harness, report, case, f"primary/handoff/paused-{command[0]}", (*command, *_COMMON), 75)
    _pair(harness, report, case, "primary/handoff/attach", ("attach", "worker", *_COMMON))
    _pair(harness, report, case, "primary/handoff/read", ("read", "worker", *_COMMON))
    report.require("primary/handoff/preserved-input", all(_submitted(root) == []
                   and _state(root).get("focused") == _fixture_identity(root)[0] for root in (case.python_root, case.rust_root)),
                   "handoff injected input or failed to focus the owned pane")
    _change(case, {"status": "idle", "screen": ""})
    _pair(harness, report, case, "primary/handoff/resume", ("resume", "worker", *_COMMON))
    _pair(harness, report, case, "primary/handoff/drain", ("drain", "worker", *_COMMON))
    report.require("primary/handoff/pending-once", all(_submitted(root) == ["queued before handoff"]
                   for root in (case.python_root, case.rust_root)) and
                   _queue_snapshot(case.python_root, "registry/agents/worker/queue") == _queue_snapshot(case.rust_root, "registry/agents/worker/queue"),
                   "resume lost, duplicated, or changed the pending request")
    _change(case, {"run_mode": "fail"})
    _pair(harness, report, case, "primary/handoff/ambiguous",
          ("send", "worker", "uncertain request", "--message-id", "uncertain", *_COMMON), 76)
    _change(case, {"run_mode": "normal"})
    _pair(harness, report, case, "primary/handoff/no-replay", ("drain", "worker", *_COMMON))
    report.require("primary/handoff/uncertain-retained", all(
        _submitted(root) == ["queued before handoff", "uncertain request"]
        and (root / "registry/agents/worker/queue/failed/uncertain.json").exists()
        for root in (case.python_root, case.rust_root)), "uncertain work was replayed or discarded")


def _queue_lanes(harness: Harness, report: Report) -> None:
    """The Rust edition's expiring and cancellable prompts, and the other edition's refusal.

    A prompt sent with --ttl goes to inbox.v2/, which the Python edition must refuse to drain or
    add to rather than type without honouring the expiry; the Rust edition types it, and sets a
    cancelled one aside untyped. Both projects run the same edition at each step, so the paired
    outcomes must agree.
    """
    case = harness.case("primary-queue-lanes")
    if not _start(harness, report, case, "primary/lanes/start"):
        return
    rust, python = harness.rust, harness.python
    roots = (case.python_root, case.rust_root)

    def both(command: Sequence[str], label: str, arguments: Sequence[str], expected: int) -> tuple[Outcome, Outcome]:
        # The same edition in both projects, so the two outcomes must agree with each other.
        first = harness._invoke_one(command, case.python_root, arguments)
        second = harness._invoke_one(command, case.rust_root, arguments)
        report.require(label, first.returncode == second.returncode == expected,
                       f"expected rc {expected}; first={first!r} second={second!r}")
        return first, second

    _change(case, {"status": "working", "screen": "unrecognized terminal layout\n"})
    both(rust, "primary/lanes/send-ttl", ("send", "worker", "expiring prompt", "--ttl", "3600",
                                           "--message-id", "lane", "--ready-timeout", "0", *_COMMON), 75)
    both(rust, "primary/lanes/send-cancelled", ("send", "worker", "cancelled prompt", "--ttl", "3600",
                                                 "--message-id", "gone", "--ready-timeout", "0", *_COMMON), 75)
    report.require("primary/lanes/second-inbox", all(
        (root / "registry/agents/worker/queue/inbox.v2/lane.json").is_file()
        and not (root / "registry/agents/worker/queue/inbox/lane.json").exists() for root in roots),
        "a prompt with --ttl did not go to inbox.v2/")
    outcomes = both(rust, "primary/lanes/cancel", ("cancel", "worker", "gone", "--reason", "superseded", *_COMMON), 0)
    report.require("primary/lanes/cancel-set-aside", all(
        isinstance(record := _json(outcome), dict) and record.get("outcome") == "set_aside" for outcome in outcomes),
        f"cancel of a waiting prompt in an idle queue did not set it aside: {outcomes!r}")
    _change(case, {"status": "idle", "screen": ""})
    for label, arguments in (("drain", ("drain", "worker", *_COMMON)),
                             ("send", ("send", "worker", "python prompt", *_COMMON))):
        refused = both(python, f"primary/lanes/python-refuses-{label}", arguments, 75)
        report.require(f"primary/lanes/python-refusal-reason-{label}", all(
            "only the Rust edition of agentctl honours" in outcome.stderr for outcome in refused),
            f"the other edition did not name the Rust edition: {refused!r}")
    report.require("primary/lanes/python-typed-nothing", all(
        _submitted(root) == [] for root in roots),
        "the other edition typed into a queue with Rust-only lanes")
    both(rust, "primary/lanes/rust-drain", ("drain", "worker", *_COMMON), 0)
    report.require("primary/lanes/rust-typed-once", all(
        _submitted(root) == ["expiring prompt"]
        and (root / "registry/agents/worker/queue/processed/lane.json").is_file()
        and (root / "registry/agents/worker/queue/cancelled/gone.json").is_file() for root in roots),
        "the Rust drain did not type the expiring prompt once, or the cancelled one left cancelled/")


def _queue_supersede(harness: Harness, report: Report) -> None:
    """#1156 queue-supersede-key: only the newest pending copy per key is typed.

    Run once per edition (the same edition in both projects, as _queue_lanes):
    two sends under one key while the pane cannot take them, then a drain.
    The editions agree on the outcome and on the cancelled/ record; they
    differ only in lane placement - Rust writes keyed prompts to inbox.v2/,
    Python to the classic inbox it can also supersede within.
    """
    for edition, command, lane in (
        ("rust", harness.rust, "inbox.v2"),
        ("python", harness.python, "inbox"),
    ):
        case = harness.case(f"primary-queue-supersede-{edition}")
        if not _start(harness, report, case, f"primary/supersede-{edition}/start"):
            continue
        roots = (case.python_root, case.rust_root)

        def both(label: str, arguments: Sequence[str], expected: int) -> tuple[Outcome, Outcome]:
            first = harness._invoke_one(command, case.python_root, arguments)
            second = harness._invoke_one(command, case.rust_root, arguments)
            report.require(label, first.returncode == second.returncode == expected,
                           f"expected rc {expected}; first={first!r} second={second!r}")
            return first, second

        _change(case, {"status": "working", "screen": "unrecognized terminal layout\n"})
        both(f"primary/supersede-{edition}/send-first",
             ("send", "worker", "notice v1", "--supersede-key", "notice:worker",
              "--message-id", "sup-1", "--queue-only", *_COMMON), 0)
        outcomes = both(f"primary/supersede-{edition}/send-second",
                        ("send", "worker", "notice v2", "--supersede-key", "notice:worker",
                         "--message-id", "sup-2", "--queue-only", *_COMMON), 0)
        report.require(f"primary/supersede-{edition}/reported", all(
            isinstance(record := _json(outcome), dict)
            and record.get("superseded") == ["sup-1"] for outcome in outcomes),
            f"the replacing send did not name what it superseded: {outcomes!r}")

        def replaced_ok(root: Path) -> bool:
            path = root / "registry/agents/worker/queue/cancelled/sup-1.json"
            if not path.is_file():
                return False
            document = cast(dict[str, object], json.loads(path.read_text(encoding="utf-8")))
            return bool(document.get("cancel_reason") == "superseded"
                        and document.get("superseded_by") == "sup-2"
                        and document.get("supersede_key") == "notice:worker")

        report.require(f"primary/supersede-{edition}/set-aside", all(
            replaced_ok(root)
            and (root / f"registry/agents/worker/queue/{lane}/sup-2.json").is_file()
            and not (root / f"registry/agents/worker/queue/{lane}/sup-1.json").exists()
            for root in roots),
            "the older copy was not set aside as superseded, or the newest is not pending")
        _change(case, {"status": "idle", "screen": ""})
        both(f"primary/supersede-{edition}/drain", ("drain", "worker", *_COMMON), 0)
        report.require(f"primary/supersede-{edition}/typed-newest-only", all(
            _submitted(root) == ["notice v2"] for root in roots),
            "the drain did not type only the newest copy")


def _registry_and_interop(harness: Harness, report: Report) -> None:
    defaults = harness.case("primary-default-registry")
    _pair(harness, report, defaults, "primary/default-registry/start", ("start", "worker", "--herdr-bin", "<HERDR>"))
    report.require("primary/default-registry/location", all((root / ".agentctl/agents/worker/agent.json").is_file()
                   for root in (defaults.python_root, defaults.rust_root)), "default registry was not .agentctl")
    older = harness.case("primary-record-defaults")
    if _start(harness, report, older, "primary/old-record/start"):
        for root in (older.python_root, older.rust_root):
            path = root / "registry/agents/worker/agent.json"
            record = json.loads(path.read_text(encoding="utf-8"))
            for key in ("adapter", "mode", "backend", "paused", "runtime_home"):
                record.pop(key, None)
            path.write_text(json.dumps(record), encoding="utf-8")
        python, _ = _pair(harness, report, older, "primary/old-record/defaults", ("status", "worker", *_COMMON))
        record = _json(python)
        report.require("primary/old-record/compatible", isinstance(record, dict) and all(record.get(key) == value
                       for key, value in {"adapter": "herdr", "mode": "interactive", "backend": "herdr",
                                          "paused": False, "runtime_home": None}.items()),
                       f"older records lost their interactive defaults: {record!r}")
    for option in ("--registry", "--state"):
        for before in (True, False):
            label = f"primary/global/{option}/{'before' if before else 'after'}"
            case = harness.case(label)
            shared = (option, "<ROOT>/chosen", "--herdr-bin", "<HERDR>")
            def command(*args: str) -> tuple[str, ...]:
                return (*shared, *args) if before else (*args, *shared)
            _pair(harness, report, case, label + "/start", command("start", "worker"))
            _pair(harness, report, case, label + "/status", command("status", "worker"))
            report.require(label + "/default-record", all(
                (root / "chosen/agents/worker/agent.json").is_file() and not (root / ".agentctl").exists()
                and _state(root).get("launch_arguments") == [
                    "--no-alt-screen", "--config", "model_reasoning_effort=medium"]
                for root in (case.python_root, case.rust_root)), "registry alias/placement or native defaults changed")
    for label, producer, consumer in (("python-rust", harness.python, harness.rust), ("rust-python", harness.rust, harness.python)):
        case = harness.case(f"primary/interop/{label}")
        root = case.python_root
        started = harness._invoke_one(producer, root, _with_effort(("start", "worker", *_COMMON)))
        path = root / "registry/agents/worker/agent.json"
        if path.exists():
            record = json.loads(path.read_text(encoding="utf-8"))
            record["extension_metadata"] = {"purpose": "review", "labels": ["persistent"]}
            path.write_text(json.dumps(record), encoding="utf-8")
        paused = harness._invoke_one(consumer, root, ("pause", "worker", *_COMMON))
        refused = harness._invoke_one(producer, root, ("send", "worker", "human owns input", *_COMMON))
        resumed = harness._invoke_one(consumer, root, ("resume", "worker", *_COMMON))
        _wait_for_queue_quiescence(case, report, f"primary/interop/{label}/quiescence")
        sent = harness._invoke_one(consumer, root, ("send", "worker", "shared primary registry", *_COMMON))
        stopped = harness._invoke_one(producer, root, ("stop", "worker", *_COMMON))
        outcomes = (started, paused, resumed, sent, stopped)
        report.require(f"primary/interop/{label}", all(outcome.returncode == 0 for outcome in outcomes)
                       and refused.returncode == 75 and _submitted(root) == ["shared primary registry"],
                       f"canonical registry/handoff incompatible: {outcomes!r}; refused={refused!r}")
        archives = list((root / "registry/archive").glob("*/agent.json"))
        report.require(f"primary/interop/{label}/extension-metadata", len(archives) == 1
                       and json.loads(archives[0].read_text(encoding="utf-8")).get("extension_metadata")
                       == {"purpose": "review", "labels": ["persistent"]},
                       "another edition discarded unknown record metadata during handoff/retirement")


def _legacy_adopted_registry_and_queue(harness: Harness, report: Report) -> None:
    """Read and drain one literal pre-profile adopted record and queue artifact.

    The record sits in registry layout 1 (``registry/foreign``), as every build before #593
    registry-agents-subdir wrote it. ``status`` reads it in place; ``drain`` writes, so both
    editions first move it to layout 2 (``registry/agents/foreign``), identically.
    """

    case = harness.case("primary-legacy-adopted-registry")
    # The field set is derived from AgentRecord in pre-profile revision
    # 475c2498c2c54767f70e3056cf6fda668738205a. These are constructed test
    # artifacts rather than captured command output; each root has distinct
    # expected_cwd bytes, which are frozen before either edition reads them.
    for root in (case.python_root, case.rust_root):
        agent_root = root / "registry/foreign"
        queue = agent_root / "queue"
        for directory in (
            root / "registry", agent_root, queue,
            queue / "inbox", queue / "inflight", queue / "processed", queue / "failed",
        ):
            directory.mkdir(mode=0o700, exist_ok=True)
            directory.chmod(0o700)
        record = {
            "name": "foreign",
            "token": "legacy-token",
            "harness": "codex",
            "cwd": str(root),
            "created_at": 1.0,
            "schema": 1,
            "lifecycle": "running",
            "workspace_id": "w1",
            "tab_id": _fixture_identity(root)[0].replace(":p", ":t", 1),
            "pane_id": _fixture_identity(root)[0],
            "session_agent": "codex",
            "session_value": "session-1",
            "model": None,
            "resume": None,
            "arguments": [],
            "error": None,
            "goal": None,
            "goal_delivery": None,
            "goal_session_id": None,
            "goal_command": None,
            "goal_messages": {},
            "goal_message_id": None,
            "adapter": "herdr-foreign",
            "mode": "interactive",
            "backend": "herdr",
            "paused": False,
            "runtime_home": None,
        }
        binding = {
            "kind": "session",
            "agent": "codex",
            "value": "session-1",
            "expected_agent": "codex",
            "expected_workspace": None,
            "expected_cwd": str(root),
        }
        artifacts = {
            agent_root / "agent.json": json.dumps(record, sort_keys=True) + "\n",
            queue / "target.json": json.dumps(binding, sort_keys=True) + "\n",
            queue / "inbox/000000000007.json": (
                '{"seq":7,"text":"legacy fifo","tui_delivery_attempts":0}\n'
            ),
        }
        for path, content in artifacts.items():
            path.write_text(content, encoding="utf-8")
            path.chmod(0o600)

    immutable_paths = ("agent.json", "queue/target.json")
    immutable_before = [
        {
            relative: (root / "registry/foreign" / relative).read_bytes()
            for relative in immutable_paths
        }
        for root in (case.python_root, case.rust_root)
    ]
    queue_before_status = [
        _queue_snapshot(root, "registry/foreign/queue")
        for root in (case.python_root, case.rust_root)
    ]

    python, _ = _pair(
        harness, report, case, "primary/legacy-adopted/status",
        ("status", "foreign", *_COMMON),
    )
    status = _json(python)
    report.require(
        "primary/legacy-adopted/status-shape",
        isinstance(status, dict)
        and status.get("adapter") == "herdr-foreign"
        and status.get("agent_status") == "idle"
        and status.get("pending") == ["000000000007"],
        f"pre-profile adopted record or pending queue was not readable: {status!r}",
    )
    immutable_after_status = [
        {
            relative: (root / "registry/foreign" / relative).read_bytes()
            for relative in immutable_paths
        }
        for root in (case.python_root, case.rust_root)
    ]
    queue_after_status = [
        _queue_snapshot(root, "registry/foreign/queue")
        for root in (case.python_root, case.rust_root)
    ]
    report.require(
        "primary/legacy-adopted/status-nonmutation",
        immutable_after_status == immutable_before
        and queue_after_status == queue_before_status,
        "status rewrote the pre-profile record, binding, or queue",
    )
    drained = harness.invoke(case, ("drain", "foreign", *_COMMON))
    # The migration notice names each edition's own registry path.
    notices = [
        outcome.stderr.replace(str(root), "<ROOT>")
        for outcome, root in zip(drained, (case.python_root, case.rust_root), strict=True)
    ]
    report.require(
        "primary/legacy-adopted/drain",
        drained[0].returncode == drained[1].returncode == 0
        and _json(drained[0]) == _json(drained[1])
        and notices[0] == notices[1],
        f"editions disagree on draining a layout-1 agent: {drained!r}",
    )
    tripwires = [root / "registry/agents/agent.json" for root in (case.python_root, case.rust_root)]
    report.require(
        "primary/legacy-adopted/layout-migrated",
        all(
            not (root / "registry/foreign").exists()
            and (root / "registry/agents/foreign/agent.json").is_file()
            and not (root / "registry/.foreign.lock").exists()
            and not (root / "registry/.foreign.rank.lock").exists()
            for root in (case.python_root, case.rust_root)
        )
        and all(path.is_file() for path in tripwires)
        and tripwires[0].read_bytes() == tripwires[1].read_bytes()
        and isinstance(json.loads(tripwires[0].read_text(encoding="utf-8")), str)
        and all("registry layout 2" in outcome.stderr for outcome in drained),
        "a write did not move the layout-1 agent, or the editions moved it differently: "
        f"{drained!r}",
    )
    snapshots = [
        _queue_snapshot(root, "registry/agents/foreign/queue")
        for root in (case.python_root, case.rust_root)
    ]
    immutable_after_drain = [
        {
            relative: (root / "registry/agents/foreign" / relative).read_bytes()
            for relative in immutable_paths
        }
        for root in (case.python_root, case.rust_root)
    ]
    processed = snapshots[0].get("processed/000000000007.json")
    report.require(
        "primary/legacy-adopted/drained-once",
        # Queued before the sender header existed, so drained exactly as written.
        all(_state(root).get("submitted") == ["legacy fifo"]
            and not list((root / "registry/agents/foreign/queue/inbox").iterdir())
            and (root / "registry/agents/foreign/queue/processed/000000000007.json").is_file()
            for root in (case.python_root, case.rust_root))
        and immutable_after_drain == immutable_before
        and snapshots[0] == snapshots[1]
        and set(snapshots[0]) == {"processed/000000000007.json"}
        and isinstance(processed, dict)
        and processed.get("seq") == 7
        and processed.get("text") == "legacy fifo"
        and processed.get("tui_delivery_attempts") == 0,
        f"pre-profile pending artifact diverged or was lost: {snapshots!r}",
    )
    _change(case, {"empty_shell": True})
    _pair(
        harness,
        report,
        case,
        "primary/legacy-adopted/dead-stop-refusal",
        ("stop", "foreign", *_COMMON),
        75,
    )
    report.require(
        "primary/legacy-adopted/dead-stop-preserved",
        all(
            (root / "registry/agents/foreign/agent.json").is_file()
            and not _state(root).get("closed")
            and not _state(root).get("closed_panes")
            for root in (case.python_root, case.rust_root)
        ),
        "legacy adopted record without shell identity was retired or mutated",
    )

    _change(case, {"fail_read": True})
    capture_failures = []
    for command, root in (
        (harness.python, case.python_root), (harness.rust, case.rust_root),
    ):
        record_path = root / "registry/agents/foreign/agent.json"
        digest = hashlib.sha256(record_path.read_bytes()).hexdigest()
        capture_failures.append(harness._invoke_one(command, root, (
            "stop", "foreign", "--recover-legacy-adoption",
            "--expected-token", "legacy-token",
            "--expected-record-sha256", digest,
            *_COMMON,
        )))
    report.require(
        "primary/legacy-adopted/capture-failure-is-busy",
        all(outcome.returncode == 75 for outcome in capture_failures)
        and all((root / "registry/agents/foreign/agent.json").is_file()
                for root in (case.python_root, case.rust_root)),
        f"legacy recovery capture failures did not preserve delivery classification: {capture_failures!r}",
    )
    _change(case, {"fail_read": False})

    outcomes: list[Outcome] = []
    recovery_values: list[dict[str, object]] = []
    for command, root in (
        (harness.python, case.python_root), (harness.rust, case.rust_root),
    ):
        record_path = root / "registry/agents/foreign/agent.json"
        digest = hashlib.sha256(record_path.read_bytes()).hexdigest()
        outcome = harness._invoke_one(command, root, (
            "stop", "foreign", "--recover-legacy-adoption",
            "--expected-token", "legacy-token",
            "--expected-record-sha256", digest,
            *_COMMON,
        ))
        outcomes.append(outcome)
        value = json.loads(outcome.stdout) if outcome.returncode == 0 else {}
        if isinstance(value, dict):
            report.require(
                f"primary/legacy-adopted/recovery-hash-{root.parent.name}",
                value.get("record_sha256") == digest,
                f"recovery did not report its exact record digest: {outcome!r}",
            )
            value["record_sha256"] = "<ROOT-SPECIFIC-HASH>"
        recovery_values.append(value)
    report.require(
        "primary/legacy-adopted/recovery-parity",
        all(outcome.returncode == 0 and not outcome.stderr for outcome in outcomes)
        and _normalize(recovery_values[0]) == _normalize(recovery_values[1]),
        f"legacy recovery diverged: {outcomes!r}",
    )
    report.require(
        "primary/legacy-adopted/recovery-preserves-bytes-and-runtime",
        all(
            not (root / "registry/agents/foreign").exists()
            and not _state(root).get("closed")
            and not _state(root).get("closed_panes")
            and len(list((root / "registry/archive").glob("*/agent.json"))) == 1
            and list((root / "registry/archive").glob("*/agent.json"))[0].read_bytes()
                == immutable_after_drain[index]["agent.json"]
            and _queue_snapshot(
                list((root / "registry/archive").glob("*"))[0], "queue"
            ) == snapshots[index]
            for index, root in enumerate((case.python_root, case.rust_root))
        ),
        "explicit recovery changed legacy record/queue bytes or mutated its runtime",
    )


def _ownership(harness: Harness, report: Report) -> None:
    for label, change in (("unavailable", {"offline": True}), ("replacement", {"name": "different"}),
                          ("late-human-pane", {"extra_pane_on_read": True})):
        case = harness.case(f"primary/ownership/{label}")
        if not _start(harness, report, case, f"primary/ownership/{label}/start"):
            continue
        _change(case, change)
        if label == "late-human-pane":
            python, _ = _pair(harness, report, case, "primary/ownership/late-pane-stop", ("stop", "worker", *_COMMON))
            result = _json(python)
            report.require("primary/ownership/human-pane-preserved", isinstance(result, dict)
                           and result.get("pane_closed") is True and result.get("tab_closed") is False
                           and all(_state(root).get("closed_panes") == [_fixture_identity(root)[0]] and _state(root).get("extra_pane")
                                   for root in (case.python_root, case.rust_root)),
                           f"closing the owned pane also closed the human pane: {result!r}")
        else:
            _pair(harness, report, case, f"primary/ownership/{label}/status", ("status", "worker", *_COMMON))
            outcomes = harness.invoke(case, ("stop", "worker", *_COMMON))
            report.require(f"primary/ownership/{label}/stop-refusal", all(outcome.returncode == 69 for outcome in outcomes)
                           and all((root / "registry/agents/worker/agent.json").exists() and not _state(root).get("closed")
                           for root in (case.python_root, case.rust_root)), f"uncertain identity was retired: {outcomes!r}")


def _adopted_generation(harness: Harness, report: Report, label: str) -> PairCase | None:
    """Produce foreign records through the CLI with real foreground generations."""
    case = harness.case(label, {"empty_shell": True, "sessionless": True})
    for root in (case.python_root, case.rust_root):
        subprocess.run([
            str(root / "fake-herdr"), "agent", "start", "external-runtime",
            "--kind", "codex", "--pane", "w1:p1", "--timeout", "1", "--",
        ], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    outcomes = _pair(harness, report, case, label + "/adopt", (
        "adopt", "foreign", "--pane", "w1:p1", "--workspace", "project",
        "--cwd", "<ROOT>", "--harness", "codex", *_COMMON,
    ))
    if any(outcome.returncode != 0 for outcome in outcomes):
        _retire_fixture_processes(case, report)
        return None
    return case


def _record_document(path: Path) -> dict[str, object]:
    value: object = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise TypeError("fixture record must be an object")
    return {str(key): item for key, item in value.items()}


def _artifact_bytes(directory: Path) -> dict[str, bytes]:
    """Capture original artifacts, including arbitrary forensic file formats."""
    return {path.relative_to(directory).as_posix(): path.read_bytes()
            for path in directory.rglob("*") if path.is_file()}


def _dead_adopted_retirement(harness: Harness, report: Report) -> None:
    """Cross-consume raw adopted records without consulting the restarted server."""
    case = _adopted_generation(harness, report, "primary/dead-adopted")
    if case is None:
        return
    originals: list[dict[str, bytes]] = []
    tokens: list[str] = []
    digests: list[str] = []
    for root in (case.python_root, case.rust_root):
        directory = root / "registry/agents/foreign"
        path = directory / "agent.json"
        document = _record_document(path)
        token = document["token"]
        if not isinstance(token, str):
            raise TypeError("fixture record token must be a string")
        tokens.append(token)
        document["future_metadata"] = {"preserve": "☃", "opaque": [1, None, True]}
        path.write_bytes((json.dumps(document, ensure_ascii=False, indent=1) + "\n \n").encode())
        artifacts = {
            "output.json": b'{"text":"saved output","captured_at":7}\n',
            "operator-note.bin": b"\x00\xffpreserve\r\n",
            "queue/.delivery.lock": b"",
            "queue/inbox/pending.json": b'{"id":"pending","text":"never replay","queued_at":1}\n',
            "queue/inflight/uncertain.json": b'{"id":"uncertain","text":"maybe submitted"}\n',
            "queue/failed/quarantined.json": b'{"id":"quarantined","text":"do not retry"}\n',
            "queue/failed/quarantined.json.error": b'{"outcome":"possibly_submitted"}\n',
            "queue/processed/done.json": b'{"id":"done","text":"already submitted"}\n',
        }
        for relative, content in artifacts.items():
            target = directory / relative
            target.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
            target.write_bytes(content)
            target.chmod(0o600)
        originals.append(_artifact_bytes(directory))
        digests.append(hashlib.sha256(path.read_bytes()).hexdigest())
    _retire_fixture_processes(case, report)
    _change(case, {
        "closed": False, "closed_panes": [], "empty_shell": True,
        "fixture_shell_pid": harness.replacement_shell.pid,
        "fixture_shell_executable": harness.fixture_shell_executable,
    })
    # Each consumer reads the other edition's producer record in its own fixture root.
    consumers = ((harness.rust, case.python_root), (harness.python, case.rust_root))
    refused = [harness._invoke_one(command, root, ("stop", "foreign", *_COMMON))
               for command, root in consumers]
    actions = [_stop_recovery_arguments(outcome) for outcome in refused]
    report.require("primary/dead-adopted/restart-advice", all(
        outcome.returncode == 75 and "recorded pane shell generation changed" in outcome.stderr
        and arguments is not None and "--retire-dead-adoption" in arguments
        and "--recover-legacy-adoption" not in arguments
        and _option_values(arguments, "--expected-token") == [token]
        and _option_values(arguments, "--expected-record-sha256") == [digest]
        for outcome, arguments, token, digest in zip(refused, actions, tokens, digests, strict=True)
    ), f"restart refused without exact dead-adoption advice: {refused!r}")
    _change(case, {"offline": True})
    offline_refused = [harness._invoke_one(command, root, ("stop", "foreign", *_COMMON))
                       for command, root in consumers]
    report.require("primary/dead-adopted/offline-advice", all(
        outcome.returncode == 69 and (arguments := _stop_recovery_arguments(outcome)) is not None
        and "--retire-dead-adoption" in arguments
        and _option_values(arguments, "--expected-token") == [token]
        and _option_values(arguments, "--expected-record-sha256") == [digest]
        for outcome, token, digest in zip(offline_refused, tokens, digests, strict=True)
    ), f"offline failure lost previously captured authority: {offline_refused!r}")
    calls = [_fixture_calls(root) for _command, root in consumers]
    for label, extra in (
        ("missing-assertions", ()),
        ("missing-digest", ("--expected-token", "stale-token")),
        ("both-selectors", ("--recover-legacy-adoption",)),
    ):
        outcomes = [harness._invoke_one(command, root, (
            "stop", "foreign", "--retire-dead-adoption", *extra, *_COMMON,
        )) for command, root in consumers]
        report.require("primary/dead-adopted/cli/" + label, all(
            outcome.returncode == 2
            and (arguments := _stop_recovery_arguments(outcome)) is not None
            and arguments[-1] == "doctor" for outcome in outcomes
        ), f"invalid retirement syntax did not refuse with advice: {outcomes!r}")
    for label, change in (("token", "token"), ("same-token-raw-bytes", "format")):
        for index, (_command, root) in enumerate(consumers):
            path = root / "registry/agents/foreign/agent.json"
            content = originals[index]["agent.json"]
            if change == "token":
                document = _record_document(path)
                document["token"] = "replacement-token"
                path.write_text(json.dumps(document), encoding="utf-8")
            else:
                path.write_bytes(content + b"\n")
        outcomes = [
            harness._invoke_one(command, root, arguments[1:]) if arguments is not None
            else Outcome(1, "", "missing captured command")
            for (command, root), arguments in zip(consumers, actions, strict=True)
        ]
        report.require("primary/dead-adopted/stale-advice/" + label, all(
            outcome.returncode == 75
            and (arguments := _stop_recovery_arguments(outcome)) is not None
            and arguments[-1] == "doctor" for outcome in outcomes
        ) and all((root / "registry/agents/foreign/agent.json").is_file()
                  and _fixture_calls(root) == calls[index]
                  for index, (_command, root) in enumerate(consumers)),
                       f"stale command acquired new record authority: {outcomes!r}")
        for index, (_command, root) in enumerate(consumers):
            (root / "registry/agents/foreign/agent.json").write_bytes(originals[index]["agent.json"])
    retired = [
        harness._invoke_one(command, root, arguments[1:]) if arguments is not None
        else Outcome(1, "", "missing captured command")
        for (command, root), arguments in zip(consumers, actions, strict=True)
    ]
    values: list[object] = []
    for index, ((_command, root), outcome) in enumerate(zip(consumers, retired, strict=True)):
        value = _json(outcome)
        report.require(f"primary/dead-adopted/interop/{index}", outcome.returncode == 0
                       and not outcome.stderr and isinstance(value, dict)
                       and value.get("record_sha256") == digests[index]
                       and value.get("retired_dead_adoption") is True
                       and value.get("runtime_preserved") is True
                       and value.get("pane_closed") is False and value.get("tab_closed") is False,
                       f"dead adoption did not retire through the other edition: {outcome!r}")
        if isinstance(value, dict):
            value["record_sha256"] = "<RAW-RECORD-HASH>"
        values.append(value)
    report.require("primary/dead-adopted/result-parity", values[0] == values[1],
                   f"retirement result schema differs: {values!r}")
    report.require("primary/dead-adopted/whole-original-preserved", all(
        not (root / "registry/agents/foreign").exists()
        and len(archived := list((root / "registry/archive").glob("*/agent.json"))) == 1
        and all((archived[0].parent / relative).read_bytes() == content
                for relative, content in originals[index].items())
        and _fixture_calls(root) == calls[index]
        and _submitted(root) == []
        and _state(root).get("closed_panes") == []
        for index, (_command, root) in enumerate(consumers)
    ) and harness.replacement_shell.poll() is None,
                   "retirement changed original bytes, replayed a prompt or touched the replacement shell")

    for label in ("live", "unknown-image", "missing-pin", "old-anchor", "nonrunning", "owned"):
        guarded = _adopted_generation(harness, report, "primary/dead-adopted/refuse-" + label)
        if guarded is None:
            continue
        assertions: list[tuple[str, str]] = []
        for root in (guarded.python_root, guarded.rust_root):
            path = root / "registry/agents/foreign/agent.json"
            document = _record_document(path)
            if label == "unknown-image":
                identity = document.get("harness_identity")
                if not isinstance(identity, dict) or not isinstance(identity.get("executable_inode"), int):
                    raise TypeError("fixture harness identity is missing")
                identity["executable_inode"] += 1
            elif label == "missing-pin":
                document["harness_identity"] = None
            elif label == "old-anchor":
                document["anchor_rule"] = 1
            elif label == "nonrunning":
                document["lifecycle"] = "stopped"
            elif label == "owned":
                document["adapter"] = "herdr"
            path.write_text(json.dumps(document), encoding="utf-8")
            token = document["token"]
            if not isinstance(token, str):
                raise TypeError("fixture record token must be a string")
            assertions.append((token, hashlib.sha256(path.read_bytes()).hexdigest()))
        _change(guarded, {"offline": True})
        before = [_fixture_calls(root) for root in (guarded.python_root, guarded.rust_root)]
        outcomes = [harness._invoke_one(command, root, (
            "stop", "foreign", "--retire-dead-adoption", "--expected-token", token,
            "--expected-record-sha256", digest, *_COMMON,
        )) for command, root, (token, digest) in zip(
            (harness.python, harness.rust), (guarded.python_root, guarded.rust_root), assertions, strict=True,
        )]
        report.require("primary/dead-adopted/guard/" + label, all(
            outcome.returncode == 75
            and (arguments := _stop_recovery_arguments(outcome)) is not None
            and arguments[-1] == "doctor" for outcome in outcomes
        ) and all((root / "registry/agents/foreign/agent.json").is_file()
                  and _fixture_calls(root) == before[index]
                  for index, root in enumerate((guarded.python_root, guarded.rust_root))),
                       f"{label} record was retired or queried Herdr: {outcomes!r}")
        _change(guarded, {"offline": False})
        _retire_fixture_processes(guarded, report)


def _managed_dead_recovery(harness: Harness, report: Report) -> None:
    case = harness.case("primary-managed-dead", {"empty_shell": False})
    if not _start(harness, report, case, "primary/managed-dead/start"):
        return
    _change(case, {"empty_shell": True, "sessionless": True})
    refused = harness.invoke(case, ("stop", "worker", *_COMMON))
    report.require(
        "primary/managed-dead/token-required",
        all(outcome.returncode == 75 for outcome in refused)
        and all((root / "registry/agents/worker/agent.json").is_file()
                and not _state(root).get("closed_panes")
                for root in (case.python_root, case.rust_root)),
        f"managed-dead retirement did not require a token: {refused!r}",
    )
    tokens = [
        json.loads((root / "registry/agents/worker/agent.json").read_text(encoding="utf-8"))["token"]
        for root in (case.python_root, case.rust_root)
    ]
    recovery_arguments = [_stop_recovery_arguments(outcome) for outcome in refused]
    report.require(
        "primary/managed-dead/exact-recovery-command",
        all(arguments is not None and "stop" in arguments and "worker" in arguments
            for arguments in recovery_arguments)
        and all(
            _option_values(arguments or (), "--registry") == ["<ROOT>/registry"]
            and _option_values(arguments or (), "--herdr-bin") == ["<ROOT>/fake-herdr"]
            and _option_values(arguments or (), "--expected-token") == [token]
            for arguments, token in zip(recovery_arguments, tokens, strict=True)
        ),
        f"recovery command lost its trusted token or selected globals: {refused!r}",
    )
    original_records: list[tuple[Path, bytes]] = []
    for root in (case.python_root, case.rust_root):
        path = root / "registry/agents/worker/agent.json"
        original = path.read_bytes()
        original_records.append((path, original))
        document = json.loads(original)
        document["token"] += "-replacement"
        path.write_text(json.dumps(document), encoding="utf-8")
    stale_retries = [
        harness._invoke_one(command, root, arguments[1:])
        if arguments is not None else Outcome(1, "", "missing recovery command")
        for command, root, arguments in zip(
            (harness.python, harness.rust),
            (case.python_root, case.rust_root), recovery_arguments, strict=True,
        )
    ]
    report.require(
        "primary/managed-dead/stale-advice-preserves-replacement",
        all(outcome.returncode == 75 for outcome in stale_retries)
        and all(
            (arguments := _stop_recovery_arguments(outcome)) is not None
            and arguments[-1] == "doctor"
            and not _option_values(arguments, "--expected-token")
            for outcome in stale_retries
        )
        and all((root / "registry/agents/worker/agent.json").is_file()
                and not _state(root).get("closed_panes")
                for root in (case.python_root, case.rust_root)),
        f"stale advice acquired a replacement generation: {stale_retries!r}",
    )
    for path, original in original_records:
        path.write_bytes(original)
    _change(case, {"fail_read": True})
    capture_failures = [
        harness._invoke_one(command, root, (
            "stop", "worker", "--expected-token", token, *_COMMON,
        ))
        for command, root, token in zip(
            (harness.python, harness.rust),
            (case.python_root, case.rust_root),
            tokens,
            strict=True,
        )
    ]
    report.require(
        "primary/managed-dead/capture-failure-is-busy",
        all(outcome.returncode == 75 for outcome in capture_failures)
        and all((root / "registry/agents/worker/agent.json").is_file()
                for root in (case.python_root, case.rust_root)),
        f"managed-dead capture failures did not preserve delivery classification: {capture_failures!r}",
    )
    _change(case, {"fail_read": False, "fixture_shell_pid": 2_147_483_000})
    proof_failures = [
        harness._invoke_one(command, root, (
            "stop", "worker", "--expected-token", token, *_COMMON,
        ))
        for command, root, token in zip(
            (harness.python, harness.rust),
            (case.python_root, case.rust_root),
            tokens,
            strict=True,
        )
    ]
    report.require(
        "primary/managed-dead/procfs-failure-is-busy",
        all(outcome.returncode == 75 for outcome in proof_failures)
        and all((root / "registry/agents/worker/agent.json").is_file()
                for root in (case.python_root, case.rust_root)),
        f"managed-dead procfs failures diverged: {proof_failures!r}",
    )
    _change(case, {
        "fixture_shell_pid": harness.fixture_shell.pid,
        "fail_process_info": True,
    })
    control_failures = [
        harness._invoke_one(command, root, (
            "stop", "worker", "--expected-token", token, *_COMMON,
        ))
        for command, root, token in zip(
            (harness.python, harness.rust),
            (case.python_root, case.rust_root),
            tokens,
            strict=True,
        )
    ]
    report.require(
        "primary/managed-dead/process-info-failure-is-busy",
        all(outcome.returncode == 75 for outcome in control_failures)
        and all((root / "registry/agents/worker/agent.json").is_file()
                for root in (case.python_root, case.rust_root)),
        f"managed-dead process-info failures diverged: {control_failures!r}",
    )
    _change(case, {"fail_process_info": False})
    for field, value in (("mode", "headless"), ("backend", "tmux")):
        original_documents: list[tuple[Path, dict[str, object]]] = []
        for root in (case.python_root, case.rust_root):
            path = root / "registry/agents/worker/agent.json"
            document = json.loads(path.read_text(encoding="utf-8"))
            original_documents.append((path, document.copy()))
            document[field] = value
            path.write_text(json.dumps(document), encoding="utf-8")
        malformed = [
            harness._invoke_one(command, root, (
                "stop", "worker", "--expected-token", token, *_COMMON,
            ))
            for command, root, token in zip(
                (harness.python, harness.rust),
                (case.python_root, case.rust_root),
                tokens,
                strict=True,
            )
        ]
        report.require(
            f"primary/managed-dead/malformed-{field}-refusal",
            all(outcome.returncode == 75 for outcome in malformed)
            and all(not _state(root).get("closed_panes")
                    for root in (case.python_root, case.rust_root)),
            f"malformed managed record was not refused consistently: {malformed!r}",
        )
        for path, document in original_documents:
            path.write_text(json.dumps(document), encoding="utf-8")
    for root in (case.python_root, case.rust_root):
        path = root / "registry/agents/worker/agent.json"
        document = json.loads(path.read_text(encoding="utf-8"))
        document["lifecycle"] = "stopping"
        path.write_text(json.dumps(document), encoding="utf-8")
    stopping = [
        harness._invoke_one(command, root, (
            "stop", "worker", "--expected-token", token, *_COMMON,
        ))
        for command, root, token in zip(
            (harness.python, harness.rust),
            (case.python_root, case.rust_root),
            tokens,
            strict=True,
        )
    ]
    report.require(
        "primary/managed-dead/stopping-without-proof-refusal",
        all(outcome.returncode == 75 for outcome in stopping)
        and all(not _state(root).get("closed_panes")
                for root in (case.python_root, case.rust_root)),
        f"unproved stopping generation was retired: {stopping!r}",
    )
    for root in (case.python_root, case.rust_root):
        path = root / "registry/agents/worker/agent.json"
        document = json.loads(path.read_text(encoding="utf-8"))
        document["lifecycle"] = "running"
        path.write_text(json.dumps(document), encoding="utf-8")
    outcomes = []
    for command, root, arguments in zip(
        (harness.python, harness.rust), (case.python_root, case.rust_root),
        recovery_arguments, strict=True,
    ):
        outcomes.append(harness._invoke_one(command, root, arguments[1:])
                        if arguments is not None else Outcome(1, "", "missing recovery command"))
    report.require(
        "primary/managed-dead/recovery-parity",
        all(outcome.returncode == 0 for outcome in outcomes)
        and _json(outcomes[0]) == _json(outcomes[1])
        and all(
            _state(root).get("closed_panes") == [_fixture_identity(root)[0]]
            and not (root / "registry/agents/worker").exists()
            and len(list((root / "registry/archive").glob("*/output.json"))) == 1
            for root in (case.python_root, case.rust_root)
        ),
        f"managed-dead retirement diverged: {outcomes!r}",
    )


def _stop_recovery_quoted_globals(harness: Harness, report: Report) -> None:
    """Execute refusal advice with literal spaces, quotes, and shell characters."""
    case = harness.case("primary-stop-quoted-globals")
    registry = "private state's $(literal); registry"
    executable = "fake herdr's $(literal); executable"
    common = (f"--registry=<ROOT>/{registry}", f"--herdr-bin=<ROOT>/{executable}")
    for root in (case.python_root, case.rust_root):
        shutil.copyfile(root / "fake-herdr", root / executable)
        (root / executable).chmod(0o700)
    outcomes = harness.invoke(case, (
        "start", "worker", "--effort", "medium", "--cwd", "<ROOT>", "--workspace-id", "w1", *common,
    ))
    report.require("primary/stop/quoted-globals/start", all(outcome.returncode == 0 for outcome in outcomes),
                   f"fixture could not start with literal globals: {outcomes!r}")
    if any(outcome.returncode != 0 for outcome in outcomes):
        return
    _change(case, {"empty_shell": True, "sessionless": True})
    refused = harness.invoke(case, ("stop", "worker", *common))
    commands = [_stop_recovery_arguments(outcome) for outcome in refused]
    tokens = [
        json.loads((root / registry / "agents/worker/agent.json").read_text(encoding="utf-8"))["token"]
        for root in (case.python_root, case.rust_root)
    ]
    report.require(
        "primary/stop/quoted-globals/advice",
        all(outcome.returncode == 75 for outcome in refused)
        and all(
            arguments is not None
            and _option_values(arguments, "--registry") == [f"<ROOT>/{registry}"]
            and _option_values(arguments, "--herdr-bin") == [f"<ROOT>/{executable}"]
            and _option_values(arguments, "--expected-token") == [token]
            for arguments, token in zip(commands, tokens, strict=True)
        ),
        f"quoted recovery advice changed a literal value: {refused!r}",
    )
    recovered = [
        harness._invoke_one(command, root, arguments[1:])
        if arguments is not None else Outcome(1, "", "missing recovery command")
        for command, root, arguments in zip(
            (harness.python, harness.rust), (case.python_root, case.rust_root), commands, strict=True,
        )
    ]
    report.require(
        "primary/stop/quoted-globals/execute",
        all(outcome.returncode == 0 for outcome in recovered)
        and all(not (root / registry / "agents/worker").exists()
                and _state(root).get("closed_panes") == [_fixture_identity(root)[0]]
                and len(list((root / registry / "archive").glob("*/agent.json"))) == 1
                for root in (case.python_root, case.rust_root)),
        f"printed advice could not recover the selected registry: {recovered!r}",
    )


def _adoption(harness: Harness, report: Report) -> None:
    case = harness.case("primary-adoption")
    common = ("--herdr-bin", "<HERDR>", "--registry", "<ROOT>/registry")
    python, _ = _pair(harness, report, case, "primary/adopt/register", (
        "adopt", "foreign", "--pane", "w1:p1", "--workspace", "project",
        "--cwd", "<ROOT>", "--harness", "codex", "--session", "session-1", *common,
    ))
    adopted = _json(python)
    report.require("primary/adopt/metadata", isinstance(adopted, dict)
                   and all(adopted.get(key) == value for key, value in {
                       "name": "foreign", "adapter": "herdr-foreign", "mode": "interactive",
                       "backend": "herdr", "pane_id": "w1:p1", "tab_id": "w1:t1",
                       "workspace_id": "w1", "session_agent": "codex",
                       "session_value": "session-1", "capabilities": _ADOPTED_CAPABILITIES,
                   }.items()), f"adoption lost exact live identity: {adopted!r}")
    report.require(
        "primary/adopt/shell-identity-schema",
        isinstance(adopted, dict)
        and _valid_foreign_shell_identity(adopted.get("foreign_shell_identity")),
        f"adoption did not retain the exact six-field shell identity: {adopted!r}",
    )
    for command in (("status", "foreign"), ("list",),
                    ("wait", "foreign", "--timeout", "0")):
        _pair(harness, report, case, f"primary/adopt/{command[0]}", (*command, *common))
    _pair(harness, report, case, "primary/adopt/send",
          ("send", "foreign", "retained request", "--message-id", "adopted-1", *common))
    _pair(harness, report, case, "primary/adopt/read", ("read", "foreign", *common))
    _pair(harness, report, case, "primary/adopt/bind",
          ("bind-session", "foreign", "session-1", *_GOAL_COMMAND, *common))
    _pair(harness, report, case, "primary/adopt/pause", ("pause", "foreign", *common))
    _pair(harness, report, case, "primary/adopt/resume", ("resume", "foreign", *common))
    _pair(harness, report, case, "primary/adopt/attach", ("attach", "foreign", *common))
    python, _ = _pair(harness, report, case, "primary/adopt/unregister",
                      ("stop", "foreign", *common))
    stopped = _json(python)
    report.require("primary/adopt/runtime-preserved", isinstance(stopped, dict)
                   and stopped.get("runtime_preserved") is True
                   and stopped.get("pane_closed") is False
                   and stopped.get("tab_closed") is False
                   and all(not _state(root).get("closed")
                           and not _state(root).get("closed_panes")
                           and not (root / "registry/agents/foreign").exists()
                           and len(list((root / "registry/archive").glob("*/agent.json"))) == 1
                           and len(list((root / "registry/archive").glob(
                               "*/queue/processed/adopted-1.json"))) == 1
                           for root in (case.python_root, case.rust_root)),
                   f"unregister mutated a foreign runtime or lost durable state: {stopped!r}")

    retirement_cases: tuple[tuple[str, Mapping[str, object], int], ...] = (
        ("absent-idle", {"empty_shell": True}, 0),
        (
            "absent-restart-during-capture",
            {"empty_shell": True, "restart_agent_on_read": True},
            75,
        ),
        (
            "absent-leaves-idle-during-capture",
            {"empty_shell": True, "leave_idle_shell_on_read": True},
            75,
        ),
        ("live-moved-tab", {"tab_id": "w1:moved"}, 0),
        ("absent-moved-tab", {"empty_shell": True, "tab_id": "w1:moved"}, 75),
    )
    for label, changed_state, expected in retirement_cases:
        retirement = harness.case(f"primary-adoption-retire-{label}")
        _pair(harness, report, retirement, f"primary/adopt/{label}/register", (
            "adopt", "foreign", "--pane", "w1:p1", "--workspace", "project",
            "--cwd", "<ROOT>", "--harness", "codex", "--session", "session-1",
            *common,
        ))
        _change(retirement, changed_state)
        _pair(
            harness,
            report,
            retirement,
            f"primary/adopt/{label}/stop",
            ("stop", "foreign", *common),
            expected,
        )
        report.require(
            f"primary/adopt/{label}/state",
            all(
                not _state(root).get("closed")
                and not _state(root).get("closed_panes")
                and (root / "registry/agents/foreign").exists() == (expected != 0)
                for root in (retirement.python_root, retirement.rust_root)
            ),
            f"foreign runtime mutation or registry outcome diverged for {label}",
        )

    replacement_shell = {
        "fixture_shell_pid": harness.replacement_shell.pid,
        "fixture_shell_executable": harness.fixture_shell_executable,
    }
    for label, initial_state, session_arguments in (
        ("live-replaced-shell", {}, ("--session", "session-1")),
        ("sessionless-live-replaced-shell", {"sessionless": True}, ()),
    ):
        replacement_case = harness.case(f"primary-adoption-retire-{label}", initial_state)
        _pair(harness, report, replacement_case, f"primary/adopt/{label}/register", (
            "adopt", "foreign", "--pane", "w1:p1", "--workspace", "project",
            "--cwd", "<ROOT>", "--harness", "codex", *session_arguments, *common,
        ))
        _change(replacement_case, replacement_shell)
        _pair(
            harness,
            report,
            replacement_case,
            f"primary/adopt/{label}/stop",
            ("stop", "foreign", *common),
            75,
        )
        report.require(
            f"primary/adopt/{label}/preserved",
            all(
                (root / "registry/agents/foreign/agent.json").is_file()
                and not _state(root).get("closed")
                and not _state(root).get("closed_panes")
                for root in (replacement_case.python_root, replacement_case.rust_root)
            ),
            f"live replacement was archived or its runtime was mutated for {label}",
        )

    legacy_live = harness.case("primary-adoption-retire-live-legacy")
    _pair(harness, report, legacy_live, "primary/adopt/live-legacy/register", (
        "adopt", "foreign", "--pane", "w1:p1", "--workspace", "project",
        "--cwd", "<ROOT>", "--harness", "codex", "--session", "session-1", *common,
    ))
    for root in (legacy_live.python_root, legacy_live.rust_root):
        path = root / "registry/agents/foreign/agent.json"
        document = json.loads(path.read_text(encoding="utf-8"))
        del document["foreign_shell_identity"]
        path.write_text(json.dumps(document), encoding="utf-8")
    _pair(
        harness,
        report,
        legacy_live,
        "primary/adopt/live-legacy/stop",
        ("stop", "foreign", *common),
        75,
    )
    report.require(
        "primary/adopt/live-legacy/preserved",
        all(
            (root / "registry/agents/foreign/agent.json").is_file()
            and not _state(root).get("closed")
            and not _state(root).get("closed_panes")
            for root in (legacy_live.python_root, legacy_live.rust_root)
        ),
        "live legacy record was automatically retired or its runtime was mutated",
    )

    mismatch_cases: tuple[tuple[str, Mapping[str, object], tuple[str, ...]], ...] = (
        ("non-agent", {"harness": None}, ()),
        ("harness", {}, ("--harness", "claude")),
        ("workspace", {}, ("--workspace", "wrong")),
        ("session", {}, ("--session", "wrong-session")),
    )
    for label, state, replacement in mismatch_cases:
        mismatch = harness.case(f"primary-adoption-{label}", state)
        arguments = [
            "adopt", "foreign", "--pane", "w1:p1", "--workspace", "project",
            "--cwd", "<ROOT>", "--harness", "codex", *common,
        ]
        option = replacement[0] if replacement else None
        if option is not None:
            index = arguments.index(option) if option in arguments else -1
            if index >= 0:
                arguments[index:index + 2] = replacement
            else:
                arguments.extend(replacement)
        outcomes = harness.invoke(mismatch, arguments)
        report.require(f"primary/adopt/refuse-{label}", all(
            outcome.returncode == 75 for outcome in outcomes
        ) and all(not (root / "registry/agents/foreign").exists()
                  and not _state(root).get("closed")
                  for root in (mismatch.python_root, mismatch.rust_root)),
            f"identity mismatch was adopted or mutated: {outcomes!r}")

    for label, producer, consumer in (
        ("python-rust", harness.python, harness.rust),
        ("rust-python", harness.rust, harness.python),
    ):
        interop = harness.case(f"primary-adoption-interop-{label}")
        root = interop.python_root
        adopted = harness._invoke_one(producer, root, (
            "adopt", "foreign", "--pane", "w1:p1", "--workspace", "project",
            "--cwd", "<ROOT>", "--harness", "codex", *common,
        ))
        status = harness._invoke_one(consumer, root, ("status", "foreign", *common))
        sent = harness._invoke_one(consumer, root, (
            "send", "foreign", "cross-engine adopted request", *common,
        ))
        _change(interop, {"empty_shell": True})
        stopped = harness._invoke_one(consumer, root, ("stop", "foreign", *common))
        report.require(f"primary/adopt/interop-{label}", all(
            outcome.returncode == 0 for outcome in (adopted, status, sent, stopped)
        ) and _submitted(root) == ["cross-engine adopted request"]
            and not _state(root).get("closed") and not _state(root).get("closed_panes")
            and len(list((root / "registry/archive").glob("*/agent.json"))) == 1,
            f"adopted record was not safely portable between editions: "
            f"{(adopted, status, sent, stopped)!r}")


def _outcome_field(outcome: Outcome) -> object:
    value = _json(outcome)
    return value.get("outcome") if isinstance(value, dict) else None


def _identity_and_rename(harness: Harness, report: Report) -> None:
    """Anchors, per-effect identity checks, rename, doctor, and cross-edition rename recovery."""
    common = ("--herdr-bin", "<HERDR>", "--registry", "<ROOT>/registry")
    case = harness.case("identity-rename")
    if not _start(harness, report, case, "identity/start", "--harness", "claude"):
        return
    python, _ = _pair(harness, report, case, "identity/status", ("status", "worker", *common))
    status = _json(python)
    report.require("identity/anchored", isinstance(status, dict)
                   and status.get("terminal_id") == "term-1"
                   and isinstance(status.get("harness_identity"), dict)
                   and status["harness_identity"].get("pid") == "<PID>"
                   and status["harness_identity"].get("version") == 1,
                   f"start did not pin terminal and harness: {status!r}")
    _pair(harness, report, case, "identity/doctor-clean", ("doctor", *common))
    _pair(harness, report, case, "identity/rename", ("rename", "worker", "reviewer", *common))
    report.require("identity/rename-presentation", all(
        _state(root).get("name") == "reviewer" and _state(root).get("tab_label") == "reviewer"
        and (root / "registry" / "agents" / "reviewer" / "agent.json").exists()
        and not (root / "registry" / "agents" / "worker").exists()
        for root in (case.python_root, case.rust_root)), "rename left a stale name or label")
    _pair(harness, report, case, "identity/send-renamed", ("send", "reviewer", "after rename", *common))
    report.require("identity/send-renamed-delivered", all(
        _submission_count(root, "after rename") == 1 for root in (case.python_root, case.rust_root)),
        "send after rename was not delivered exactly once")
    _change(case, {"input_expect": True})
    _pair(harness, report, case, "identity/send-expect", ("send", "reviewer", "with expect", *common))
    pastes = [[call for call in cast(list[list[str]], _state(root)["calls"])
               if call[:2] == ["pane", "send-text"]] for root in (case.python_root, case.rust_root)]
    report.require("identity/expect-flag-used", all(
        calls and calls[-1][:4] == ["pane", "send-text", "--expect-terminal",
                                    _fixture_identity(root)[1]]
        for calls, root in zip(pastes, (case.python_root, case.rust_root), strict=True)),
        f"an advertised input-expect capability was not used: {pastes!r}")
    _change(case, {"input_expect": False, "tab_label": "retitled"})
    outcomes = harness.invoke(case, ("send", "reviewer", "after drift", *common))
    report.require("identity/label-drift-refused", all(
        outcome.returncode == 75 and _outcome_field(outcome) == "pending" for outcome in outcomes)
        and all(_submission_count(root, "after drift") == 0
                for root in (case.python_root, case.rust_root)),
        f"label drift did not keep the message pending: {outcomes!r}")
    outcomes = harness.invoke(case, ("doctor", *common))
    findings = [_json(outcome) for outcome in outcomes]
    report.require("identity/doctor-drift", all(outcome.returncode == 1 for outcome in outcomes)
                   and findings[0] == findings[1], f"doctor disagreed on drift: {outcomes!r}")
    _pair(harness, report, case, "identity/doctor-repair", ("doctor", "--repair-labels", *common))
    report.require("identity/repaired-label", all(
        _state(root).get("tab_label") == "reviewer" for root in (case.python_root, case.rust_root)),
        "label repair did not restore the label")
    # An interrupted rename by one edition is finished by the other, on the same registry.
    for first, second, root in ((harness.python, harness.rust, case.python_root),
                                (harness.rust, harness.python, case.rust_root)):
        state = _state(root)
        state["fail_tab_rename_once"] = True
        (root / "state.json").write_text(json.dumps(state), encoding="utf-8")
        crashed = harness._invoke_one(first, root, ("rename", "reviewer", "lead", *common))
        blocked = harness._invoke_one(second, root, ("send", "lead", "blocked", *common))
        finished = harness._invoke_one(second, root, ("rename", "reviewer", "lead", *common))
        record = json.loads((root / "registry" / "agents" / "lead" / "agent.json").read_text(encoding="utf-8")) \
            if (root / "registry" / "agents" / "lead" / "agent.json").exists() else {}
        history = record.get("name_history", []) if isinstance(record, dict) else []
        report.require(f"identity/cross-recovery/{root.parent.name}",
                       crashed.returncode != 0 and blocked.returncode != 0
                       and finished.returncode == 0
                       and [entry.get("name") for entry in history] == ["worker", "reviewer"]
                       and not list((root / "registry" / ".renames").glob("*.json"))
                       and _state(root).get("tab_label") == "lead",
                       f"crashed={crashed!r} blocked={blocked!r} finished={finished!r} history={history!r}")
    _retire_fixture_processes(case, report)


def _invalid_cli(harness: Harness, report: Report) -> None:
    case = harness.case("primary-invalid-cli")
    for label, arguments in (
        ("unknown", ("unknown", *FIXTURE_HERDR)), ("missing-name", ("status", *FIXTURE_HERDR)),
        ("missing-message", ("send", "worker", *FIXTURE_HERDR)),
        ("empty-message", ("send", "worker", "", *FIXTURE_HERDR)),
        ("blank-message", ("send", "worker", " \n\t", *FIXTURE_HERDR)),
        ("empty-goal", ("goal", "worker", "", *FIXTURE_HERDR)),
        ("abbreviation", ("start", "worker", "--work", "w1", *FIXTURE_HERDR)),
        ("nonfinite", ("send", "worker", "text", "--ready-timeout", "nan", *FIXTURE_HERDR)),
        ("zero-working", ("send", "worker", "text", "--working-timeout", "0", *FIXTURE_HERDR)),
        ("zero-startup", ("start", "worker", "--startup-timeout", "0", *FIXTURE_HERDR)),
        ("environment-missing-equals", ("start", "worker", "--env", "MISSING_EQUALS", *FIXTURE_HERDR)),
        ("environment-empty-name", ("start", "worker", "--env", "=value", *FIXTURE_HERDR)),
        ("environment-invalid-name", ("start", "worker", "--env", "BAD-NAME=value", *FIXTURE_HERDR)),
        ("zero-lines", ("read", "worker", "--lines", "0", *FIXTURE_HERDR)),
        ("underscore-count", ("read", "worker", "--lines", "1_0", *FIXTURE_HERDR)),
        ("unicode-time", ("wait", "worker", "--timeout", "١.0", *FIXTURE_HERDR)),
        ("prompt-conflict", ("send", "worker", "text", "--file", "file.txt", *FIXTURE_HERDR)),
        ("bad-goal-command", ("goal", "worker", "--goal-command-json", "[]", *FIXTURE_HERDR)),
        ("missing-global-value", ("--registry", "--help")),
        ("wrong-command-option", ("status", "worker", "--brief", "text", *FIXTURE_HERDR)),
        ("legacy-recovery-missing-token", (
            "stop", "worker", "--recover-legacy-adoption",
            "--expected-record-sha256", "0" * 64, *FIXTURE_HERDR,
        )),
        ("legacy-recovery-missing-hash", (
            "stop", "worker", "--recover-legacy-adoption",
            "--expected-token", "generation", *FIXTURE_HERDR,
        )),
        ("legacy-hash-without-recovery", (
            "stop", "worker", "--expected-record-sha256", "0" * 64, *FIXTURE_HERDR,
        )),
        ("stop-missing-name", (*_COMMON, "stop")),
        ("stop-unknown-option", (*_COMMON, "stop", "worker", "--unrecognized")),
        ("stop-missing-option-value", (*_COMMON, "stop", "worker", "--expected-token")),
    ):
        outcomes = harness.invoke(case, arguments)
        report.require(f"primary/cli/{label}", all(outcome.returncode == 2
                       and "traceback" not in outcome.stderr.lower() and "panicked" not in outcome.stderr.lower()
                       for outcome in outcomes), f"invalid invocation was not a clean usage error: {outcomes!r}")
        if arguments and (arguments[0] == "stop" or label.startswith("stop-")):
            report.require(f"primary/cli/{label}/recovery-command",
                           all(_stop_recovery_arguments(outcome) is not None for outcome in outcomes),
                           f"stop usage refusal omitted recovery advice: {outcomes!r}")
        if label.startswith("stop-"):
            report.require(
                f"primary/cli/{label}/recovery-globals",
                all(
                    (recovery := _stop_recovery_arguments(outcome)) is not None
                    and recovery[-1] == "doctor"
                    and _option_values(recovery, "--registry") == ["<ROOT>/registry"]
                    and _option_values(recovery, "--herdr-bin") == ["<ROOT>/fake-herdr"]
                    for outcome in outcomes
                ),
                f"stop syntax advice lost parsed globals or inferred a target: {outcomes!r}",
            )
    report.require("primary/cli/no-registry-side-effect", all(not (root / ".agentctl").exists()
                   for root in (case.python_root, case.rust_root)), "invalid syntax created persistent state")


def _seed_layout_one(root: Path, names: Sequence[str]) -> None:
    """A registry in layout 1, as builds before #593 registry-agents-subdir wrote it: one
    directory per agent beside the fixed files, and two lock files per agent."""
    registry = root / "registry"
    registry.mkdir(mode=0o700, exist_ok=True)
    registry.chmod(0o700)
    for name in names:
        directory = registry / name
        directory.mkdir(mode=0o700)
        # The pre-profile record shape of _legacy_adopted_registry_and_queue.
        record = {
            "name": name, "token": f"{name}-token", "harness": "codex", "cwd": str(root),
            "created_at": 1.0, "schema": 1, "lifecycle": "running", "workspace_id": "w1",
            "tab_id": "w1:t1", "pane_id": f"w1:p-{name}", "session_agent": "codex",
            "session_value": f"session-{name}", "model": None, "resume": None, "arguments": [],
            "error": None, "goal": None, "goal_delivery": None, "goal_session_id": None,
            "goal_command": None, "goal_messages": {}, "goal_message_id": None,
            "adapter": "herdr-foreign", "mode": "interactive", "backend": "herdr",
            "paused": False, "runtime_home": None,
        }
        (directory / "agent.json").write_text(json.dumps(record, sort_keys=True) + "\n", encoding="utf-8")
        (directory / "agent.json").chmod(0o600)
        for lock in (f".{name}.lock", f".{name}.rank.lock"):
            (registry / lock).write_bytes(b"")
            (registry / lock).chmod(0o600)
    for fixed in (".identity.lock", ".coordinator.lock", ".gone.lock"):
        (registry / fixed).write_bytes(b"")
        (registry / fixed).chmod(0o600)


def _layout_report(outcome: Outcome, root: Path) -> object:
    """A migrate-layout report with the edition's own registry path normalized."""
    value = _json(outcome)
    if isinstance(value, dict) and isinstance(value.get("registry"), str):
        value = {**value, "registry": value["registry"].replace(str(root), "<ROOT>")}
    return value


def _top_level(root: Path) -> list[str]:
    return sorted(path.name for path in (root / "registry").iterdir())


def _registry_layout_migration(harness: Harness, report: Report) -> None:
    """Both editions migrate a layout-1 registry identically, idempotently, and read each
    other's result (#593 registry-agents-subdir)."""
    case = harness.case("primary-registry-layout-migration")
    roots = (case.python_root, case.rust_root)
    for root in roots:
        _seed_layout_one(root, ("alpha", "beta"))
    dry = harness.invoke(case, ("migrate-layout", *_COMMON))
    dry_reports = [_layout_report(outcome, root) for outcome, root in zip(dry, roots, strict=True)]
    report.require(
        "primary/layout/dry-run",
        all(outcome.returncode == 0 for outcome in dry)
        and dry_reports[0] == dry_reports[1]
        and isinstance(dry_reports[0], dict)
        and dry_reports[0].get("would_move") == ["alpha", "beta"]
        and dry_reports[0].get("would_remove_lock_files") == 5
        and dry_reports[0].get("layout_before") == "1"
        and all((root / "registry/alpha/agent.json").is_file() and not (root / "registry/agents").exists()
                for root in roots),
        f"the dry runs differ or changed the registry: {dry!r}",
    )
    applied = harness.invoke(case, ("migrate-layout", "--apply", *_COMMON))
    applied_reports = [_layout_report(outcome, root) for outcome, root in zip(applied, roots, strict=True)]
    report.require(
        "primary/layout/apply",
        all(outcome.returncode == 0 for outcome in applied)
        and applied_reports[0] == applied_reports[1]
        and isinstance(applied_reports[0], dict)
        and applied_reports[0].get("moved") == ["alpha", "beta"]
        and applied_reports[0].get("removed_lock_files") == 5
        and applied_reports[0].get("layout") == "2"
        and all(_top_level(root) == [".coordinator.lock", ".identity.lock", ".renames", "agents", "locks"]
                for root in roots)
        and (case.python_root / "registry/.renames/upgrade-agentctl-registry-layout-2").read_bytes()
        == (case.rust_root / "registry/.renames/upgrade-agentctl-registry-layout-2").read_bytes()
        and (case.python_root / "registry/agents/agent.json").read_bytes()
        == (case.rust_root / "registry/agents/agent.json").read_bytes()
        and all(
            (root / "registry/agents/alpha/agent.json").read_bytes()
            == json.dumps(json.loads((root / "registry/agents/alpha/agent.json").read_text(encoding="utf-8")),
                          sort_keys=True).encode() + b"\n"
            for root in roots
        ),
        f"the editions migrated differently: {applied!r}",
    )
    again = harness.invoke(case, ("migrate-layout", "--apply", *_COMMON))
    again_reports = [_layout_report(outcome, root) for outcome, root in zip(again, roots, strict=True)]
    report.require(
        "primary/layout/idempotent",
        all(outcome.returncode == 0 for outcome in again)
        and again_reports[0] == again_reports[1]
        and isinstance(again_reports[0], dict)
        and again_reports[0].get("moved") == []
        and again_reports[0].get("removed_lock_files") == 0,
        f"a second migration changed something: {again!r}",
    )
    # Each edition reads the registry the other one migrated, and both read layout 1 unmoved.
    crossed = (
        harness._invoke_one(harness.rust, case.python_root, ("status", "alpha", *_COMMON)),
        harness._invoke_one(harness.python, case.rust_root, ("status", "alpha", *_COMMON)),
    )
    statuses = [_json(outcome) for outcome in crossed]
    report.require(
        "primary/layout/interop",
        all(isinstance(status, dict) and status.get("name") == "alpha" for status in statuses),
        f"an edition could not read the registry the other migrated: {crossed!r}",
    )
    # A name in both layouts, as an older build creating an agent after the move would leave,
    # is refused by both editions and never touched by a migration.
    for root in roots:
        _seed_layout_one(root, ("alpha",))
    conflicted = harness.invoke(case, ("migrate-layout", "--apply", *_COMMON))
    conflict_reports = [_layout_report(outcome, root) for outcome, root in zip(conflicted, roots, strict=True)]
    refused = harness.invoke(case, ("status", "alpha", *_COMMON))
    report.require(
        "primary/layout/conflict",
        all(outcome.returncode == 1 for outcome in conflicted)
        and conflict_reports[0] == conflict_reports[1]
        and isinstance(conflict_reports[0], dict)
        and conflict_reports[0].get("conflicts") == ["alpha"]
        and all((root / "registry/alpha/agent.json").is_file()
                and (root / "registry/agents/alpha/agent.json").is_file() for root in roots)
        and all(outcome.returncode != 0 and "both registry layouts" in outcome.stderr for outcome in refused),
        f"a name in both layouts was not refused alike: {conflicted!r} {refused!r}",
    )


_RUST_ONLY_REFUSAL = "is implemented only by the Rust edition of agentctl"
_BOOT_ID = re.compile(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")


def _rust_only(harness: Harness, report: Report, case: PairCase, label: str,
               arguments: Sequence[str]) -> Outcome:
    """Invoke a Rust-only command: the Python edition must refuse it by name with exit
    status 2 and create nothing, and the Rust outcome is returned for frozen checks.

    ``watch``, ``watch-agents`` and ``restore`` have no Python implementation to agree
    with (py/agentctl/cli.py ``_RUST_ONLY``), so this group pins the Rust behaviour
    against frozen expectations instead of forcing parity the contract does not have.
    """
    python, rust = harness.invoke(case, arguments)
    report.require(
        f"{label}/python-refusal",
        python.returncode == 2 and not python.stdout
        and _RUST_ONLY_REFUSAL in python.stderr
        and "traceback" not in python.stderr.lower(),
        f"the Python edition did not refuse a Rust-only command by name: {python!r}",
    )
    return rust


def _seed_restore_config(case: PairCase, document: Mapping[str, object]) -> dict[Path, bytes]:
    """Write the same restore.json into both registries (paths stay per-root)."""
    written: dict[Path, bytes] = {}
    for root in (case.python_root, case.rust_root):
        registry = root / "registry"
        registry.mkdir(mode=0o700, parents=True, exist_ok=True)
        registry.chmod(0o700)
        seeded = {
            key: (value.replace("<ROOT>", str(root)) if isinstance(value, str) else value)
            for key, value in document.items()
        }
        payload = json.dumps(seeded, indent=2, sort_keys=True).encode()
        (registry / "restore.json").write_bytes(payload)
        written[root] = payload
    return written


def _watch_services(harness: Harness, report: Report) -> None:
    """`watch` and `watch-agents` are Rust-only: the Python edition refuses them, and
    the Rust edition is pinned against frozen health reports on fixture registries.

    Every pass is `--once --check-only` against the case's fake Herdr with an explicit
    fixture `--state-dir`, so no unit is started, no alert is sent, and no live agent
    or host unit is touched. Configurations name no units or timers for the same
    reason: a configured unit would be judged by the host's real service manager.
    """
    case = harness.case("watch-services")
    for command in ("watch", "watch-agents"):
        python, rust = harness.invoke(case, (command, "--help"))
        report.require(
            f"watch/help/{command}",
            python.returncode == rust.returncode == 0
            and "Rust edition only" in python.stdout
            and "--once" in rust.stdout and "--check-only" in rust.stdout,
            f"help for a Rust-only command diverged: python={python!r} rust={rust!r}",
        )
    rust = _rust_only(harness, report, case, "watch/once",
                      ("watch", "--once", "--check-only", "--state-dir", "<ROOT>/watch-state", *_COMMON))
    health = _json(rust)
    report.require(
        "watch/once/report",
        rust.returncode == 0 and not rust.stderr and isinstance(health, dict)
        and health.get("schema") == "agentctl-watch-health/v1" and health.get("ok") is True
        and health.get("findings") == [{
            "check": "herdr", "subject": "server", "ok": True,
            "detail": "answers", "repair": None,
        }]
        and health.get("notice") == {"sent": False, "reason": "check only"}
        and health.get("warnings") == []
        and isinstance(health.get("boot_id"), str)
        and _BOOT_ID.fullmatch(str(health["boot_id"])) is not None
        and isinstance(health.get("checked_at"), (int, float))
        and not isinstance(health.get("checked_at"), bool),
        f"the Rust watch pass on an empty registry diverged: {rust!r}",
    )
    written = case.rust_root / "watch-state" / "health.json"
    try:
        persisted = json.loads(written.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        persisted = None
    health_findings = health.get("findings") if isinstance(health, dict) else None
    report.require(
        "watch/once/health-file",
        isinstance(persisted, dict) and persisted.get("schema") == "agentctl-watch-health/v1"
        and persisted.get("ok") is True and persisted.get("findings") == health_findings
        and not (case.python_root / "watch-state").exists()
        and not (case.python_root / "registry").exists(),
        "watch --once did not persist its health report, or the refusal wrote state",
    )
    rust = _rust_only(harness, report, case, "watch/agents-once",
                      ("watch-agents", "--once", "--check-only", "--coordinator", "coord",
                       "--state-dir", "<ROOT>/agent-watch-state", *_COMMON))
    watched = _json(rust)
    report.require(
        "watch/agents-once/report",
        rust.returncode == 0 and isinstance(watched, dict)
        and watched.get("schema") == "agentctl-agent-watch/v1" and watched.get("ok") is True
        and watched.get("agents") == 0 and watched.get("coordinator") == "coord"
        and watched.get("episodes") == [] and watched.get("probe_error") is None
        and watched.get("notice") == {"sent": False, "reason": "check only", "waiting": 0},
        f"the Rust watch-agents pass on an empty registry diverged: {rust!r}",
    )
    report.require(
        "watch/agents-once/state-file",
        (case.rust_root / "agent-watch-state" / "agent-watch.json").is_file()
        and not (case.python_root / "agent-watch-state").exists(),
        "watch-agents --once did not persist its report, or the refusal wrote state",
    )
    configured = harness.case("watch-configured")
    _seed_restore_config(configured, {
        "schema": "agentctl-restore/v1", "coordinator": "coordinator",
        "report_dir": "<ROOT>/restore-reports",
    })
    rust = _rust_only(harness, report, configured, "watch/configured",
                      ("watch", "--once", "--check-only", "--state-dir", "<ROOT>/watch-state", *_COMMON))
    health = _json(rust)
    findings = health.get("findings") if isinstance(health, dict) else None
    report.require(
        "watch/configured/report",
        rust.returncode == 1 and isinstance(findings, list) and len(findings) == 3
        and findings[0] == {
            "check": "herdr", "subject": "server", "ok": True,
            "detail": "answers", "repair": None,
        }
        and isinstance(findings[1], dict)
        and findings[1].get("check") == "restore" and findings[1].get("ok") is False
        and findings[1].get("detail") == "no completed restore report for this boot"
        and isinstance(findings[1].get("subject"), str)
        and _BOOT_ID.fullmatch(str(findings[1]["subject"])) is not None
        and isinstance(findings[2], dict)
        and findings[2].get("check") == "coordinator" and findings[2].get("ok") is False
        and findings[2].get("subject") == "coordinator"
        and isinstance(health, dict) and health.get("ok") is False
        and "no completed restore report for this boot" in rust.stderr
        and not (configured.python_root / "restore-reports").exists(),
        f"the Rust watch pass on a configured registry diverged: {rust!r}",
    )
    rust = _rust_only(harness, report, case, "watch/invalid-interval",
                      ("watch", "--once", "--interval", "5", *_COMMON))
    report.require(
        "watch/invalid-interval/rust",
        rust.returncode == 2 and "invalid value" in rust.stderr
        and "traceback" not in rust.stderr.lower() and "panicked" not in rust.stderr.lower(),
        f"an out-of-range watch interval was not a clean Rust usage error: {rust!r}",
    )


def _restore_team(harness: Harness, report: Report) -> None:
    """`restore` is Rust-only: the Python edition refuses it, and the Rust edition is
    pinned against frozen restore reports on fixture registries.

    Dry runs and a report-dir-only real run keep every effect inside the case: no
    coordinator is registered, no unit is started (dry runs only name the units a
    real run would start), and slot images name no wrkslots binary.
    """
    case = harness.case("restore-team")
    python, rust = harness.invoke(case, ("restore", "--help"))
    report.require(
        "restore/help",
        python.returncode == rust.returncode == 0
        and "Rust edition only" in python.stdout
        and "--dry-run" in rust.stdout and "--write-agents" in rust.stdout,
        f"restore help diverged: python={python!r} rust={rust!r}",
    )
    rust = _rust_only(harness, report, case, "restore/dry-run-empty",
                      ("restore", "--dry-run", *_COMMON))
    value = _json(rust)
    report.require(
        "restore/dry-run-empty/report",
        rust.returncode == 0 and isinstance(value, dict)
        and value.get("schema") == "agentctl-restore-report/v1"
        and value.get("dry_run") is True and value.get("complete") is False
        and value.get("would_complete") is True and value.get("refused") == 0
        and value.get("agents") == [] and value.get("units") == []
        and value.get("herdr") == {"ready": True, "waited_seconds": 0.0}
        and value.get("slot_images") == {"action": "skipped", "reason": "dry run"}
        and value.get("certificate") == {"present": None, "detail": "not configured"}
        and value.get("trigger") == "manual"
        and isinstance(value.get("boot_id"), str)
        and _BOOT_ID.fullmatch(str(value["boot_id"])) is not None
        and str(value.get("report", "")).endswith(f"/{value['boot_id']}.json")
        and (case.rust_root / "registry" / ".restore.lock").is_file()
        and not (case.python_root / "registry").exists(),
        f"the Rust restore dry run on an empty registry diverged: {rust!r}",
    )
    configured = harness.case("restore-configured")
    written = _seed_restore_config(configured, {
        "schema": "agentctl-restore/v1", "coordinator": "coordinator",
        "units": ["example.service"], "report_dir": "<ROOT>/restore-reports",
    })
    rust = _rust_only(harness, report, configured, "restore/dry-run-configured",
                      ("restore", "--dry-run", *_COMMON))
    value = _json(rust)
    report.require(
        "restore/dry-run-configured/report",
        rust.returncode == 0 and isinstance(value, dict)
        and value.get("coordinator") == {
            "name": "coordinator", "action": "refused",
            "reason": "the coordinator is not registered",
        }
        and value.get("units") == [{"unit": "example.service", "action": "start", "dry_run": True}]
        and value.get("would_complete") is False and value.get("complete") is False
        and value.get("report") == f"<ROOT>/restore-reports/{value.get('boot_id')}.json"
        and not (configured.rust_root / "restore-reports").exists()
        and all((root / "registry" / "restore.json").read_bytes() == payload
                for root, payload in written.items()),
        f"the Rust restore dry run on a configured registry diverged: {rust!r}",
    )
    rust = _rust_only(harness, report, configured, "restore/write-agents",
                      ("restore", "--write-agents", *_COMMON))
    value = _json(rust)
    report.require(
        "restore/write-agents/preview",
        rust.returncode == 0 and value == {
            "action": "preview", "coordinator": "coordinator", "before": [], "after": [],
            "added": [], "removed": [], "mismatches": [],
        }
        and all((root / "registry" / "restore.json").read_bytes() == payload
                for root, payload in written.items()),
        f"restore --write-agents preview diverged or wrote: {rust!r}",
    )
    real = harness.case("restore-real")
    _seed_restore_config(real, {
        "schema": "agentctl-restore/v1", "report_dir": "<ROOT>/restore-reports",
    })
    rust = _rust_only(harness, report, real, "restore/real", ("restore", *_COMMON))
    value = _json(rust)
    report_path = real.rust_root / "restore-reports" / f"{value.get('boot_id')}.json" \
        if isinstance(value, dict) else Path()
    try:
        persisted = json.loads(report_path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        persisted = None
    report.require(
        "restore/real/report",
        rust.returncode == 0 and isinstance(value, dict)
        and value.get("complete") is True and value.get("dry_run") is False
        and value.get("refused") == 0
        and value.get("slot_images") == {
            "action": "skipped", "reason": "no wrkslots configured or on PATH",
        }
        and isinstance(persisted, dict) and persisted.get("complete") is True
        and persisted.get("schema") == "agentctl-restore-report/v1"
        and not (real.python_root / "restore-reports").exists()
        and not (real.python_root / "registry" / ".restore.lock").exists(),
        f"the Rust restore on an empty registry diverged: {rust!r}",
    )
    rust = _rust_only(harness, report, real, "restore/already-restored", ("restore", *_COMMON))
    value = _json(rust)
    report.require(
        "restore/already-restored/report",
        rust.returncode == 0 and isinstance(value, dict)
        and value.get("action") == "already-restored"
        and value.get("replayed") is True
        and value.get("complete") is True
        and value.get("refused") == 0
        and value.get("refused_lanes") == []
        and value.get("would_complete") is True
        and value.get("agents") == []
        and value.get("schema") == "agentctl-restore-report/v1"
        and value.get("report") == f"<ROOT>/restore-reports/{value.get('boot_id')}.json",
        f"a second restore for the same boot diverged: {rust!r}",
    )
    invalid = harness.case("restore-invalid-config")
    _seed_restore_config(invalid, {"schema": "nope/v9"})
    rust = _rust_only(harness, report, invalid, "restore/invalid-config",
                      ("restore", "--dry-run", *_COMMON))
    report.require(
        "restore/invalid-config/rust",
        rust.returncode == 75 and not rust.stdout
        and "schema must be agentctl-restore/v1" in rust.stderr,
        f"an invalid restore.json was not refused by the Rust edition: {rust!r}",
    )
    rust = _rust_only(harness, report, invalid, "watch/invalid-config",
                      ("watch", "--once", "--check-only", "--state-dir", "<ROOT>/watch-state", *_COMMON))
    report.require(
        "watch/invalid-config/rust",
        rust.returncode == 75 and not rust.stdout
        and "schema must be agentctl-restore/v1" in rust.stderr,
        f"watch accepted an invalid restore.json: {rust!r}",
    )


def _mcp_invoke(harness: Harness, case: PairCase, payload: str) -> tuple[Outcome, Outcome]:
    """Run both `agentctl mcp` servers on the same stdio payload, in the harness's
    guarded environment (the paired `Harness.invoke` gives editions an empty stdin,
    which an MCP server can only answer with silence)."""
    outcomes: list[Outcome] = []
    for command, root in ((harness.python, case.python_root), (harness.rust, case.rust_root)):
        argv = ["--registry", str(root / "registry"), "--herdr-bin", str(root / "fake-herdr"), "mcp"]
        refusal = harness.refuse_host_cli(root, argv)
        if refusal is not None:
            outcomes.append(_normalize_outcome(Outcome(
                127, "", f"cross harness host CLI guard: refused to run the editions: {refusal}\n",
            ), root))
            continue
        environment = with_case_git(without_ambient_executables(dict(os.environ)), root)
        existing = environment.get("PYTHONPATH", "")
        local = str(REPO_ROOT / "py")
        environment["PYTHONPATH"] = local if not existing else local + os.pathsep + existing
        environment.update({
            "LANG": "C.UTF-8", "LC_ALL": "C.UTF-8", "NO_COLOR": "1", "HERDR_WORKSPACE_ID": "w1",
        })
        for harness_name in ("CODEX", "CLAUDE"):
            environment[f"AGENTCTL_{harness_name}_SKILLS_DIR"] = str(
                root / "skill-homes" / harness_name.lower()
            )
        environment["PATH"] = guarded_path(
            root, str(root / "hostile-bin") if _state(root).get("hostile_path")
            else environment.get("PATH"),
        )
        environment["AGENTCTL_MUSE_BIN"] = str(root / "fake-muse-runtime")
        environment["XDG_CONFIG_HOME"] = str(root / "xdg-config")
        try:
            # Through `run_tolerant` like every paired invoke (#1067
            # cross-differential-load-aware): the budget is spent as CPU
            # progress, not raw wall, and the process group is cleaned
            # up on timeout. It takes a stdin fd rather than `input=`,
            # so the payload rides a temp file rewound to its start.
            with tempfile.TemporaryFile(mode="w+", encoding="utf-8") as stdin_file:
                stdin_file.write(payload)
                stdin_file.seek(0)
                completed = run_tolerant(
                    [*command, *argv], timeout_s=TIMEOUT_SECONDS, cwd=root, env=environment,
                    stdin=stdin_file.fileno(), encoding="utf-8", errors="replace",
                )
            outcome = Outcome(completed.returncode, completed.stdout, completed.stderr)
        except subprocess.TimeoutExpired as error:
            outcome = Outcome(124, _decode_timeout(error.stdout),
                              _decode_timeout(error.stderr) + "\nTIMEOUT\n")
        outcomes.append(_normalize_outcome(outcome, root))
    return outcomes[0], outcomes[1]


def _decode_timeout(value: bytes | str | None) -> str:
    if value is None:
        return ""
    return value.decode("utf-8", errors="replace") if isinstance(value, bytes) else value


def _mcp_replies(outcome: Outcome) -> list[object] | None:
    replies: list[object] = []
    for line in outcome.stdout.splitlines():
        try:
            replies.append(json.loads(line))
        except ValueError:
            return None
    return replies


def _mcp_stdio(harness: Harness, report: Report) -> None:
    """Both editions serve the same MCP stdio protocol (`agentctl mcp`).

    The corpus compares parsed JSON-RPC replies exactly, except the three replies
    whose message wording is each implementation's own (its JSON parser's error, its
    argument-shape sentence, and the operating system's error text): those compare
    the error code, the id, and the shared refusal prefix only.
    """
    case = harness.case("mcp-stdio")
    eof = harness.invoke(case, ("mcp", *_COMMON))
    report.require(
        "mcp/eof",
        all(outcome.returncode == 0 and not outcome.stdout and not outcome.stderr
            for outcome in eof),
        f"an MCP server at end of input was not silent: {eof!r}",
    )
    requests: list[tuple[str, object, str]] = [
        ("initialize", {"jsonrpc": "2.0", "id": 1, "method": "initialize",
                        "params": {"protocolVersion": "2024-11-05"}}, "exact"),
        ("notification", {"jsonrpc": "2.0", "method": "notifications/initialized"}, "none"),
        ("ping", {"jsonrpc": "2.0", "id": "p1", "method": "ping"}, "exact"),
        ("tools-list", {"jsonrpc": "2.0", "id": 2, "method": "tools/list"}, "exact"),
        ("list-empty", {"jsonrpc": "2.0", "id": 3, "method": "tools/call",
                        "params": {"name": "agent_list", "arguments": {}}}, "exact"),
        ("missing-required", {"jsonrpc": "2.0", "id": 4, "method": "tools/call",
                              "params": {"name": "agent_status", "arguments": {}}}, "exact"),
        ("unknown-field", {"jsonrpc": "2.0", "id": 5, "method": "tools/call",
                           "params": {"name": "agent_status",
                                      "arguments": {"name": "worker", "bogus": 1}}}, "exact"),
        ("bool-name", {"jsonrpc": "2.0", "id": 6, "method": "tools/call",
                       "params": {"name": "agent_status", "arguments": {"name": True}}}, "exact"),
        ("unknown-tool", {"jsonrpc": "2.0", "id": 7, "method": "tools/call",
                          "params": {"name": "agent_nope", "arguments": {}}}, "exact"),
        ("unsupported-method", {"jsonrpc": "2.0", "id": 8, "method": "bogus/method"}, "exact"),
        ("wrong-version", {"jsonrpc": "1.0", "id": 9, "method": "ping"}, "exact"),
        ("status-absent", {"jsonrpc": "2.0", "id": 10, "method": "tools/call",
                           "params": {"name": "agent_status",
                                      "arguments": {"name": "worker"}}}, "error-text"),
        ("arguments-not-object", {"jsonrpc": "2.0", "id": 11, "method": "tools/call",
                                  "params": {"name": "agent_status",
                                             "arguments": "nope"}}, "code"),
        ("missing-tool-name", {"jsonrpc": "2.0", "id": 12, "method": "tools/call",
                               "params": {"arguments": {}}}, "exact"),
        ("request-not-object", [1, 2], "code"),
        ("invalid-json", "not json", "code"),
    ]
    payload = "".join(
        (request if isinstance(request, str) else json.dumps(request)) + "\n"
        for _label, request, _mode in requests
    )
    python, rust = _mcp_invoke(harness, case, payload)
    python_replies, rust_replies = _mcp_replies(python), _mcp_replies(rust)
    expected = [(label, mode) for label, _request, mode in requests if mode != "none"]
    report.require(
        "mcp/stdio/framing",
        python.returncode == rust.returncode == 0 and not python.stderr and not rust.stderr
        and python_replies is not None and rust_replies is not None
        and len(python_replies) == len(rust_replies) == len(expected),
        f"MCP replies were not one JSON document per request: python={python!r} rust={rust!r}",
    )
    if python_replies is None or rust_replies is None \
            or len(python_replies) != len(expected) or len(rust_replies) != len(expected):
        return
    for (label, mode), python_reply, rust_reply in zip(expected, python_replies, rust_replies, strict=True):
        if mode == "exact":
            report.require(
                f"mcp/stdio/{label}",
                _normalize(python_reply) == _normalize(rust_reply),
                f"MCP {label} replies diverged: python={python_reply!r} rust={rust_reply!r}",
            )
        elif mode == "error-text":
            texts: list[object] = []
            for reply in (python_reply, rust_reply):
                result = reply.get("result") if isinstance(reply, dict) else None
                content = result.get("content") if isinstance(result, dict) else None
                texts.append(content[0].get("text") if isinstance(content, list) and content
                             and isinstance(content[0], dict) else None)
            report.require(
                f"mcp/stdio/{label}",
                all(isinstance(reply, dict) and isinstance(reply.get("result"), dict)
                    and reply["result"].get("isError") is True for reply in (python_reply, rust_reply))
                and all(isinstance(text, str)
                        and text.startswith("agentctl: cannot inspect agent registry <ROOT>/registry:")
                        for text in texts),
                f"MCP {label} refusals diverged: python={python_reply!r} rust={rust_reply!r}",
            )
        else:
            report.require(
                f"mcp/stdio/{label}",
                all(isinstance(reply, dict) and isinstance(reply.get("error"), dict)
                    and reply["error"].get("code") == -32602
                    and isinstance(reply["error"].get("message"), str)
                    and reply["error"]["message"] != ""
                    for reply in (python_reply, rust_reply))
                and isinstance(python_reply, dict) and isinstance(rust_reply, dict)
                and python_reply.get("id") == rust_reply.get("id"),
                f"MCP {label} error replies diverged: python={python_reply!r} rust={rust_reply!r}",
            )
    live = harness.case("mcp-live")
    if not _start(harness, report, live, "mcp/live/start", "--harness", "codex"):
        return
    payload = "".join(json.dumps(request) + "\n" for request in (
        {"jsonrpc": "2.0", "id": 1, "method": "tools/call",
         "params": {"name": "agent_list", "arguments": {}}},
        {"jsonrpc": "2.0", "id": 2, "method": "tools/call",
         "params": {"name": "agent_status", "arguments": {"name": "worker"}}},
        {"jsonrpc": "2.0", "id": 3, "method": "tools/call",
         "params": {"name": "agent_send",
                    "arguments": {"name": "worker", "text": "mcp followup",
                                  "message_id": "mcp-1"}}},
    ))
    python, rust = _mcp_invoke(harness, live, payload)
    python_replies, rust_replies = _mcp_replies(python), _mcp_replies(rust)
    contents: list[list[object]] = []
    for replies in (python_replies, rust_replies):
        live_texts: list[object] = []
        for reply in replies or []:
            result = reply.get("result") if isinstance(reply, dict) else None
            content = result.get("content") if isinstance(result, dict) else None
            text = content[0].get("text") if isinstance(content, list) and content \
                and isinstance(content[0], dict) else None
            try:
                live_texts.append(_normalize(json.loads(text)) if isinstance(text, str) else text)
            except ValueError:
                live_texts.append(text)
        contents.append(live_texts)
    report.require(
        "mcp/live/tools",
        python.returncode == rust.returncode == 0
        and python_replies is not None and rust_replies is not None
        and len(python_replies) == len(rust_replies) == 3
        and all(isinstance(reply, dict) and isinstance(reply.get("result"), dict)
                and reply["result"].get("isError") is False
                for reply in (*python_replies, *rust_replies))
        and contents[0] == contents[1]
        and isinstance(contents[0][0], list) and len(contents[0][0]) == 1
        and isinstance(contents[0][1], dict) and contents[0][1].get("name") == "worker"
        and all(_submission_count(root, "mcp followup") == 1
                for root in (live.python_root, live.rust_root)),
        f"MCP tools against a live registry diverged: python={python!r} rust={rust!r}",
    )
    _pair(harness, report, live, "mcp/live/stop", ("stop", "worker", *_COMMON))


def _model_override_edition(harness: Harness, report: Report) -> None:
    """`set-model` (#1396) is an intended edition difference: the Rust edition
    records the per-agent model override, and the Python edition refuses it
    by name (`_RUST_ONLY`), exactly as `_rust_only` pins for other Rust-only
    commands. The record field itself is shared: both editions load, keep
    and status-report `model_override` (see the profiles/nested groups)."""
    case = harness.case("model-override")
    rust = _rust_only(harness, report, case, "model-override/set-model",
                      ("set-model", "nobody", "some-model", *_COMMON))
    report.require(
        "model-override/set-model/rust",
        rust.returncode != 0 and "nobody" in (rust.stdout + rust.stderr),
        f"the Rust edition did not refuse set-model for a missing agent: {rust!r}",
    )


def _groups() -> list[tuple[str, object]]:
    # Imported here: the group's module imports this one's helpers.
    from agentctl_nested_records import nested_records

    return [
        ("orientation", _orientation), ("lifecycle", _lifecycle),
        ("conversations", _conversations), ("launch-specs", _launch_specs),
        ("profiles", _profiles), ("profile-resume", _profile_resume),
        ("workspace-policy", _workspace_policy), ("skill-install", _skill_install),
        ("profile-refusals", _profile_refusals),
        ("herdr-093", _herdr_093_compatibility), ("muse-adoption", _muse_adoption),
        ("muse14-status", _muse14_status), ("muse-busy", _muse_busy_followup),
        ("handoff", _handoff_and_pending), ("queue-lanes", _queue_lanes),
        ("queue-supersede", _queue_supersede),
        ("registry-interop", _registry_and_interop),
        ("legacy-registry", _legacy_adopted_registry_and_queue),
        ("registry-migration", _registry_layout_migration),
        ("ownership", _ownership), ("dead-retirement", _dead_adopted_retirement),
        ("dead-recovery", _managed_dead_recovery),
        ("stop-recovery", _stop_recovery_quoted_globals),
        ("revive", _revive_interop), ("adoption", _adoption),
        ("identity", _identity_and_rename), ("nested-records", nested_records),
        ("invalid-cli", _invalid_cli), ("watch", _watch_services),
        ("model-override", _model_override_edition),
        ("restore", _restore_team), ("mcp", _mcp_stdio),
    ]


def shard_groups(shard: int, shards: int) -> list[tuple[str, object]]:
    """Interleaved shard `shard` of `shards` over the group list (all if shards=1)."""
    groups = _groups()
    if not 0 <= shard < shards:
        raise ValueError(f"shard {shard}/{shards} out of range")
    return groups[shard::shards]


def build_report(
    python_command: Sequence[str], rust_command: Sequence[str],
    *, shard: int = 0, shards: int = 1,
) -> Report:
    """Compare the canonical interface through independent executable clients."""
    report = Report()
    with _signal_cleaned_tempdir("agentctl-cross-") as temporary:
        harness = Harness(temporary, python_command, rust_command)
        try:
            for _name, run_group in shard_groups(shard, shards):
                run_group(harness, report)  # type: ignore[operator]
            harness.require_no_host_cli(report)
        finally:
            harness.close()
    return report


def compare_agentctl(
    python_command: Sequence[str], rust_command: Sequence[str],
    *, shard: int = 0, shards: int = 1,
) -> int:
    """Report semantic differences and return a conventional process exit code."""
    report = build_report(python_command, rust_command, shard=shard, shards=shards)
    for failure in report.failures:
        print(f"DIVERGENCE [{failure}]")
    if report.failures:
        print(f"cross[agentctl]: {len(report.failures)} divergence(s) out of {report.checks} paired checks")
        return 1
    print(f"cross[agentctl]: OK - {report.checks} paired checks agree")
    return 0
