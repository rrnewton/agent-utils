#!/usr/bin/env python3
"""Black-box comparisons of the shared interactive :command:`agentctl` contract.

Each edition runs against its own executable Herdr fixture. Comparisons include
durable identities, queue transitions, human handoff, native goal observation,
and refusal to close an unrelated pane. Optional headless and service adapters
are advertised separately and are not treated as shared capabilities.
"""
from __future__ import annotations

import json
import re
import tempfile
from collections.abc import Mapping, Sequence
from pathlib import Path

from herdr_agent_differential import Harness, Outcome, PairCase, Report, _queue_snapshot, _state

_COMMON = ("--herdr-bin", "<HERDR>", "--registry", "<ROOT>/registry")
_GOAL_COMMAND = ("--goal-command-json", '["<HERDR>","goal-rpc"]')
_CAPABILITIES = ["send", "status", "read", "wait", "stop", "attach", "pause", "resume",
                 "terminal-snapshot", "drain", "goal", "bind-session"]


def _normalize(value: object) -> object:
    if isinstance(value, list):
        return [_normalize(item) for item in value]
    if isinstance(value, dict):
        return {str(key): ("<TOKEN>" if key == "token" else 0 if key == "created_at"
                          else "<ARCHIVE>" if key == "archive"
                          # Native identity diagnostics use edition-specific quote styles.
                          else re.sub(r'"([a-z][a-z0-9-]*)"', r"'\1'", item)
                          if key == "probe_error" and isinstance(item, str) else _normalize(item))
                for key, item in value.items()}
    return value


def _json(outcome: Outcome) -> object:
    try:
        return _normalize(json.loads(outcome.stdout))
    except (TypeError, ValueError):
        return outcome.stdout


def _pair(harness: Harness, report: Report, case: PairCase, label: str,
          arguments: Sequence[str], expected: int = 0) -> tuple[Outcome, Outcome]:
    python, rust = harness.invoke(case, arguments)
    report.require(label, python.returncode == rust.returncode == expected
                   and _json(python) == _json(rust)
                   and (python.stderr == rust.stderr if expected == 0 else True),
                   f"expected rc {expected}; python={python!r} rust={rust!r}")
    return python, rust


def _change(case: PairCase, values: Mapping[str, object]) -> None:
    for root in (case.python_root, case.rust_root):
        state = _state(root)
        state.update(values)
        (root / "state.json").write_text(json.dumps(state), encoding="utf-8")


def _submission_count(root: Path, text: str) -> int:
    values = _state(root).get("submitted")
    return values.count(text) if isinstance(values, list) else 0


def _start(harness: Harness, report: Report, case: PairCase, label: str,
           *options: str) -> bool:
    outcomes = _pair(harness, report, case, label,
                     ("start", "worker", "--cwd", "<ROOT>", "--workspace-id", "w1", *options, *_COMMON))
    return all(outcome.returncode == 0 for outcome in outcomes)


def _orientation(harness: Harness, report: Report) -> None:
    case = harness.case("primary-orientation")
    _pair(harness, report, case, "primary/version", ("--version",))
    for arguments in ((), ("--help",), ("start", "--help"), ("send", "--help"), ("goal", "--help")):
        for edition, outcome in zip(("python", "rust"), harness.invoke(case, arguments), strict=True):
            report.require(f"primary/help/{arguments}/{edition}",
                           outcome.returncode == 0 and not outcome.stderr and "--registry" in outcome.stdout
                           and ("--message-id" in outcome.stdout if arguments == ("send", "--help") else True),
                           f"missing operation help: {outcome!r}")
    for command in ("quickstart", "userguide"):
        for edition, outcome in zip(("python", "rust"), harness.invoke(case, (command,)), strict=True):
            report.require(f"primary/{command}/{edition}", outcome.returncode == 0 and not outcome.stderr
                           and "agentctl start" in outcome.stdout and "agentctl send" in outcome.stdout,
                           f"installed guide has no working lifecycle: {outcome!r}")
    for edition, outcome in zip(("python", "rust"), harness.invoke(case, ("capabilities",)), strict=True):
        value = _json(outcome)
        report.require(f"primary/capabilities/{edition}", outcome.returncode == 0 and isinstance(value, dict)
                       and value.get("interactive") == {"backends": ["herdr"], "harnesses": ["codex", "claude"]}
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
                      "--model", "chosen-model", "--resume", "session-1", "--harness-arg=--extra"):
            continue
        for command in (("status", "worker"), ("list",), ("wait", "worker", "--timeout", "0")):
            _pair(harness, report, case, f"primary/{kind}/{command[0]}", (*command, *_COMMON))
        expected_args = (["resume", "session-1", "--no-alt-screen"] if kind == "codex"
                         else ["--resume", "session-1"]) + ["--model", "chosen-model", "--extra"]
        report.require(f"primary/{kind}/native-launch-presets",
                       all(_state(root).get("launch_arguments") == expected_args
                           for root in (case.python_root, case.rust_root)), "native launch argv was changed or split")
        python, _ = _pair(harness, report, case, f"primary/{kind}/metadata", ("status", "worker", *_COMMON))
        status = _json(python)
        report.require(f"primary/{kind}/metadata-contract", isinstance(status, dict)
                       and all(status.get(key) == value for key, value in {
                           "name": "worker", "adapter": "herdr", "mode": "interactive", "backend": "herdr",
                           "paused": False, "runtime_home": None, "capabilities": _CAPABILITIES,
                           "pane_id": "w1:p1", "session_value": "session-1", "lifecycle": "running",
                       }.items()), f"missing canonical identity: {status!r}")
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
        for root in (case.python_root, case.rust_root):
            (root / "prompt.txt").write_text("file instruction\nwith newline\n", encoding="utf-8")
        _pair(harness, report, case, f"primary/{kind}/send-file", ("send", "worker", "--file", "<ROOT>/prompt.txt", *_COMMON))
        _pair(harness, report, case, f"primary/{kind}/read", ("read", "worker", "--lines", "17", *_COMMON))
        report.require(f"primary/{kind}/read-snapshot", all((root / "registry/worker/output.json").is_file()
                       for root in (case.python_root, case.rust_root)), "read did not retain its documented terminal snapshot")
        _pair(harness, report, case, f"primary/{kind}/stop", ("stop", "worker", *_COMMON))
        _pair(harness, report, case, f"primary/{kind}/list-stopped", ("list", *_COMMON))
        report.require(f"primary/{kind}/exact-pane-retirement", all(
            _state(root).get("closed_panes") == ["w1:p1"] and not (root / "registry/worker").exists()
            and len(list((root / "registry/archive").iterdir())) == 1
            for root in (case.python_root, case.rust_root)), "owned pane or archival contract diverged")


def _handoff_and_pending(harness: Harness, report: Report) -> None:
    case = harness.case("primary-handoff")
    if not _start(harness, report, case, "primary/handoff/start"):
        return
    _change(case, {"status": "working"})
    _pair(harness, report, case, "primary/handoff/busy-pending",
          ("send", "worker", "queued before handoff", "--message-id", "waiting", "--ready-timeout", "0", *_COMMON), 75)
    _pair(harness, report, case, "primary/handoff/pause", ("pause", "worker", *_COMMON))
    for command in (("send", "worker", "refuse while paused"), ("drain", "worker"), ("goal", "worker", "refuse new goal")):
        _pair(harness, report, case, f"primary/handoff/paused-{command[0]}", (*command, *_COMMON), 75)
    _pair(harness, report, case, "primary/handoff/attach", ("attach", "worker", *_COMMON))
    _pair(harness, report, case, "primary/handoff/read", ("read", "worker", *_COMMON))
    report.require("primary/handoff/preserved-input", all(_state(root).get("submitted") == []
                   and _state(root).get("focused") == "w1:p1" for root in (case.python_root, case.rust_root)),
                   "handoff injected input or failed to focus the owned pane")
    _change(case, {"status": "idle"})
    _pair(harness, report, case, "primary/handoff/resume", ("resume", "worker", *_COMMON))
    _pair(harness, report, case, "primary/handoff/drain", ("drain", "worker", *_COMMON))
    report.require("primary/handoff/pending-once", all(_state(root).get("submitted") == ["queued before handoff"]
                   for root in (case.python_root, case.rust_root)) and
                   _queue_snapshot(case.python_root, "registry/worker/queue") == _queue_snapshot(case.rust_root, "registry/worker/queue"),
                   "resume lost, duplicated, or changed the pending request")
    _change(case, {"run_mode": "fail"})
    _pair(harness, report, case, "primary/handoff/ambiguous",
          ("send", "worker", "uncertain request", "--message-id", "uncertain", *_COMMON), 76)
    _change(case, {"run_mode": "normal"})
    _pair(harness, report, case, "primary/handoff/no-replay", ("drain", "worker", *_COMMON))
    report.require("primary/handoff/uncertain-retained", all(
        _state(root).get("submitted") == ["queued before handoff", "uncertain request"]
        and (root / "registry/worker/queue/failed/uncertain.json").exists()
        for root in (case.python_root, case.rust_root)), "uncertain work was replayed or discarded")


def _registry_and_interop(harness: Harness, report: Report) -> None:
    defaults = harness.case("primary-default-registry")
    _pair(harness, report, defaults, "primary/default-registry/start", ("start", "worker", "--herdr-bin", "<HERDR>"))
    report.require("primary/default-registry/location", all((root / ".agentctl/worker/agent.json").is_file()
                   for root in (defaults.python_root, defaults.rust_root)), "default registry was not .agentctl")
    older = harness.case("primary-record-defaults")
    if _start(harness, report, older, "primary/old-record/start"):
        for root in (older.python_root, older.rust_root):
            path = root / "registry/worker/agent.json"
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
                (root / "chosen/worker/agent.json").is_file() and not (root / ".agentctl").exists()
                and _state(root).get("launch_arguments") == ["--no-alt-screen"]
                for root in (case.python_root, case.rust_root)), "registry alias/placement or native defaults changed")
    for label, producer, consumer in (("python-rust", harness.python, harness.rust), ("rust-python", harness.rust, harness.python)):
        case = harness.case(f"primary/interop/{label}")
        root = case.python_root
        started = harness._invoke_one(producer, root, ("start", "worker", *_COMMON))
        path = root / "registry/worker/agent.json"
        if path.exists():
            record = json.loads(path.read_text(encoding="utf-8"))
            record["extension_metadata"] = {"purpose": "review", "labels": ["persistent"]}
            path.write_text(json.dumps(record), encoding="utf-8")
        paused = harness._invoke_one(consumer, root, ("pause", "worker", *_COMMON))
        refused = harness._invoke_one(producer, root, ("send", "worker", "human owns input", *_COMMON))
        resumed = harness._invoke_one(consumer, root, ("resume", "worker", *_COMMON))
        sent = harness._invoke_one(consumer, root, ("send", "worker", "shared primary registry", *_COMMON))
        stopped = harness._invoke_one(producer, root, ("stop", "worker", *_COMMON))
        outcomes = (started, paused, resumed, sent, stopped)
        report.require(f"primary/interop/{label}", all(outcome.returncode == 0 for outcome in outcomes)
                       and refused.returncode == 75 and _state(root).get("submitted") == ["shared primary registry"],
                       f"canonical registry/handoff incompatible: {outcomes!r}; refused={refused!r}")
        archives = list((root / "registry/archive").glob("*/agent.json"))
        report.require(f"primary/interop/{label}/extension-metadata", len(archives) == 1
                       and json.loads(archives[0].read_text(encoding="utf-8")).get("extension_metadata")
                       == {"purpose": "review", "labels": ["persistent"]},
                       "another edition discarded unknown record metadata during handoff/retirement")


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
                           and all(_state(root).get("closed_panes") == ["w1:p1"] and _state(root).get("extra_pane")
                                   for root in (case.python_root, case.rust_root)),
                           f"closing the owned pane also closed the human pane: {result!r}")
        else:
            _pair(harness, report, case, f"primary/ownership/{label}/status", ("status", "worker", *_COMMON))
            outcomes = harness.invoke(case, ("stop", "worker", *_COMMON))
            report.require(f"primary/ownership/{label}/stop-refusal", all(outcome.returncode == 69 for outcome in outcomes)
                           and all((root / "registry/worker/agent.json").exists() and not _state(root).get("closed")
                                   for root in (case.python_root, case.rust_root)), f"uncertain identity was retired: {outcomes!r}")


def _invalid_cli(harness: Harness, report: Report) -> None:
    case = harness.case("primary-invalid-cli")
    for label, arguments in (
        ("unknown", ("unknown",)), ("missing-name", ("status",)),
        ("missing-message", ("send", "worker")), ("empty-message", ("send", "worker", "")),
        ("blank-message", ("send", "worker", " \n\t")), ("empty-goal", ("goal", "worker", "")),
        ("abbreviation", ("start", "worker", "--work", "w1")),
        ("nonfinite", ("send", "worker", "text", "--ready-timeout", "nan")),
        ("zero-working", ("send", "worker", "text", "--working-timeout", "0")),
        ("zero-startup", ("start", "worker", "--startup-timeout", "0")),
        ("zero-lines", ("read", "worker", "--lines", "0")),
        ("underscore-count", ("read", "worker", "--lines", "1_0")),
        ("unicode-time", ("wait", "worker", "--timeout", "١.0")),
        ("prompt-conflict", ("send", "worker", "text", "--file", "file.txt")),
        ("bad-goal-command", ("goal", "worker", "--goal-command-json", "[]")),
        ("missing-global-value", ("--registry", "--help")),
        ("wrong-command-option", ("status", "worker", "--brief", "text")),
    ):
        outcomes = harness.invoke(case, arguments)
        report.require(f"primary/cli/{label}", all(outcome.returncode == 2
                       and "traceback" not in outcome.stderr.lower() and "panicked" not in outcome.stderr.lower()
                       for outcome in outcomes), f"invalid invocation was not a clean usage error: {outcomes!r}")
    report.require("primary/cli/no-registry-side-effect", all(not (root / ".agentctl").exists()
                   for root in (case.python_root, case.rust_root)), "invalid syntax created persistent state")


def build_report(python_command: Sequence[str], rust_command: Sequence[str]) -> Report:
    """Compare the canonical interface through independent executable clients."""
    report = Report()
    with tempfile.TemporaryDirectory(prefix="agentctl-cross-") as temporary:
        harness = Harness(Path(temporary), python_command, rust_command)
        _orientation(harness, report)
        _lifecycle(harness, report)
        _handoff_and_pending(harness, report)
        _registry_and_interop(harness, report)
        _ownership(harness, report)
        _invalid_cli(harness, report)
    return report


def compare_agentctl(python_command: Sequence[str], rust_command: Sequence[str]) -> int:
    """Report semantic differences and return a conventional process exit code."""
    report = build_report(python_command, rust_command)
    for failure in report.failures:
        print(f"DIVERGENCE [{failure}]")
    if report.failures:
        print(f"cross[agentctl]: {len(report.failures)} divergence(s) out of {report.checks} paired checks")
        return 1
    print(f"cross[agentctl]: OK - {report.checks} paired checks agree")
    return 0
