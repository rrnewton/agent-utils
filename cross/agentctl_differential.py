#!/usr/bin/env python3
"""Black-box comparisons of the shared interactive :command:`agentctl` contract.

Each edition runs against its own executable Herdr fixture. Comparisons include
durable identities, queue transitions, human handoff, native goal observation,
and refusal to close an unrelated pane. Optional headless and service adapters
are advertised separately and are not treated as shared capabilities.
"""
from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from collections.abc import Mapping, Sequence
from pathlib import Path

from herdr_agent_differential import Harness, Outcome, PairCase, Report, _queue_snapshot, _state

_COMMON = ("--herdr-bin", "<HERDR>", "--registry", "<ROOT>/registry")
_GOAL_COMMAND = ("--goal-command-json", '["<HERDR>","goal-rpc"]')
_CAPABILITIES = ["send", "status", "read", "wait", "stop", "attach", "pause", "resume",
                 "terminal-snapshot", "drain", "goal", "bind-session"]
_SHELL_IDENTITY_FIELDS = {
    "version", "boot_id", "pid", "starttime_ticks",
    "executable_device", "executable_inode",
}


def _normalize(value: object) -> object:
    if isinstance(value, list):
        return [_normalize(item) for item in value]
    if isinstance(value, dict):
        return {str(key): ("<TOKEN>" if key == "token" else 0 if key == "created_at"
                          else "<PROCESS_IDENTITY>" if key == "custom_process_identity"
                          else "<ARCHIVE>" if key == "archive"
                          # Native identity diagnostics use edition-specific quote styles.
                          else re.sub(r'"([a-z][a-z0-9-]*)"', r"'\1'", item)
                          if key == "probe_error" and isinstance(item, str) else _normalize(item))
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


def _retire_fixture_processes(case: PairCase) -> None:
    """Stop only custom children created by this differential's private Herdr fixtures."""
    process_ids: list[int] = []
    for root in (case.python_root, case.rust_root):
        state = _state(root)
        for field in ("custom_pid", "retired_custom_pid"):
            process_id = state.get(field)
            if isinstance(process_id, int) and process_id not in process_ids:
                process_ids.append(process_id)
        subprocess.run(
            [str(root / "fake-herdr"), "pane", "close", "w1:p1"],
            check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
    deadline = time.monotonic() + 2
    while process_ids and time.monotonic() < deadline:
        remaining: list[int] = []
        for process_id in process_ids:
            try:
                process_stat = Path(f"/proc/{process_id}/stat").read_text(encoding="utf-8")
            except (FileNotFoundError, ProcessLookupError):
                continue
            close = process_stat.rfind(")")
            if close < 0 or process_stat[close + 2:].split(maxsplit=1)[0] != "Z":
                remaining.append(process_id)
        process_ids = remaining
        if process_ids:
            time.sleep(0.01)
    if process_ids:
        raise AssertionError(f"fixture custom processes did not exit: {process_ids!r}")


def _start(harness: Harness, report: Report, case: PairCase, label: str,
           *options: str) -> bool:
    outcomes = _pair(harness, report, case, label,
                     ("start", "worker", "--cwd", "<ROOT>", "--workspace-id", "w1", *options, *_COMMON))
    return all(outcome.returncode == 0 for outcome in outcomes)


def _orientation(harness: Harness, report: Report) -> None:
    case = harness.case("primary-orientation")
    _pair(harness, report, case, "primary/version", ("--version",))
    for arguments in ((), ("--help",), ("start", "--help"), ("adopt", "--help"),
                      ("send", "--help"), ("goal", "--help")):
        for edition, outcome in zip(("python", "rust"), harness.invoke(case, arguments), strict=True):
            report.require(f"primary/help/{arguments}/{edition}",
                           outcome.returncode == 0 and not outcome.stderr and "--registry" in outcome.stdout
                           and ("--message-id" in outcome.stdout if arguments == ("send", "--help") else True)
                           and ("--env" in outcome.stdout if arguments == ("start", "--help") else True)
                           and (all(option in outcome.stdout for option in
                                    ("--pane", "--workspace", "--cwd", "--harness", "--session"))
                                and "muse is refused" in outcome.stdout.lower()
                                if arguments == ("adopt", "--help") else True),
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
                         else ["--resume", "session-1"]) + ["--model", "chosen-model", "--extra"]
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
                           "paused": False, "runtime_home": None, "capabilities": _CAPABILITIES,
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

    custom = harness.case("primary-existing-native-kind")
    python, _ = _pair(harness, report, custom, "primary/native-kind/start", (
        "start", "worker", "--cwd", "<ROOT>", "--workspace-id", "w1",
        "--harness", "third-party", *_COMMON,
    ))
    status = _json(python)
    report.require("primary/native-kind/adapter",
                   isinstance(status, dict) and status.get("adapter") == "herdr"
                   and all(_state(root).get("harness") == "third-party"
                           and _state(root).get("custom_harness") is not True
                           for root in (custom.python_root, custom.rust_root)),
                   f"existing Herdr-native third-party kind was rerouted: {status!r}")
    _pair(harness, report, custom, "primary/native-kind/stop", (
        "stop", "worker", *_COMMON,
    ))


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
                "harness": "muse", "mode": "interactive",
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
        },
    }
    expected = {
        "astra-ultra": ["--no-alt-screen", "--model", "gpt-6-astra", "--config",
                         "model_reasoning_effort=ultra", "--dangerously-enable-internet-mode",
                         "--dangerously-bypass-approvals-and-sandbox"],
        "sol": ["--no-alt-screen", "--model", "gpt-5.6-sol",
                "--dangerously-enable-internet-mode", "--dangerously-bypass-approvals-and-sandbox"],
        "watermelon": ["--model", "kiki_gb300_mxfp8_6p2_840_nwr", "--reasoning-effort", "ultra"],
        "muse-literal": ["--trust-workspace", '--meta-tag=literal $(unexpanded) "quotes"',
                         "--meta-tag=repeatable"],
        "codex-safe-config": ["--no-alt-screen", "--config=features.web_search=true"],
    }
    for profile in ("astra-ultra", "sol", "watermelon", "muse-literal", "codex-safe-config"):
        case = harness.case(f"primary-profile-{profile}")
        for root in (case.python_root, case.rust_root):
            subprocess.run(["git", "init", "-q", str(root)], check=True)
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
        ))
        status = _json(python)
        report.require(f"primary/profile/{profile}/argv", all(
            _state(root).get("launch_arguments") == expected[profile]
            for root in (case.python_root, case.rust_root)
        ), "profile argv changed, split, or reordered")
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
                    (root / "registry/worker/agent.json").read_text(encoding="utf-8")
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
                    (root / "registry/worker/agent.json").write_text(
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
                        (root / "registry/worker/agent.json").is_file()
                        and not _state(root).get("closed")
                        for root in (case.python_root, case.rust_root)
                    ),
                    f"{label} identity mismatch allowed pane retirement: {refused!r}",
                )
                for root, original in original_records.items():
                    (root / "registry/worker/agent.json").write_text(
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
                    (root / "registry/worker/agent.json").is_file()
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
                ("unknown", "unexpected", 1),
            )
            for label, field, value in malformed_variants:
                for root, original in original_records.items():
                    changed = json.loads(json.dumps(original))
                    changed["custom_process_identity"][field] = value
                    (root / "registry/worker/agent.json").write_text(
                        json.dumps(changed), encoding="utf-8"
                    )
                _pair(
                    harness, report, case,
                    f"primary/profile/watermelon/malformed-{label}",
                    ("status", "worker", *_COMMON), 75,
                )
                for root, original in original_records.items():
                    (root / "registry/worker/agent.json").write_text(
                        json.dumps(original), encoding="utf-8"
                    )
            prompt = "Muse literal $(unexpanded) delivery\nsecond line"
            _pair(harness, report, case, "primary/profile/watermelon/send", (
                "send", "worker", prompt, "--message-id", "muse-first", *_COMMON,
            ))
            report.require("primary/profile/watermelon/pane-delivery",
                           all(_state(root).get("submitted") == [prompt]
                               and _state(root).get("paste_wrapped") is True
                               and (root / "registry/worker/queue/processed/muse-first.json").is_file()
                               and not list((root / "registry/worker/queue/inflight").glob("*.json"))
                               and not list((root / "registry/worker/queue/failed").glob("*.json"))
                               for root in (case.python_root, case.rust_root)),
                           "Muse pane delivery lacked an exact post-Enter receipt")
            _change(case, {
                "custom_post_error": True, "custom_submitted": False, "custom_draft": "",
            })
            _pair(harness, report, case, "primary/profile/watermelon/redraw-refusal", (
                "send", "worker", "must remain uncertain", "--message-id", "muse-error",
                "--working-timeout", "0.05", *_COMMON,
            ), 76)
            report.require("primary/profile/watermelon/redraw-quarantine", all(
                (root / "registry/worker/queue/failed/muse-error.json").is_file()
                for root in (case.python_root, case.rust_root)
            ), "Muse draft/error redraw was incorrectly accepted as delivery")
            _change(case, {
                "custom_post_error": False, "custom_clear_without_submit": True,
                "custom_submitted": False, "custom_draft": "",
            })
            _pair(harness, report, case, "primary/profile/watermelon/repeated-redraw", (
                "send", "worker", prompt, "--message-id", "muse-repeat",
                "--working-timeout", "0.05", *_COMMON,
            ), 76)
            report.require("primary/profile/watermelon/repeated-quarantine", all(
                (root / "registry/worker/queue/failed/muse-repeat.json").is_file()
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
    for label in ("nonignored", "permissions", "symlink", "unknown-field"):
        case = harness.case(f"primary-profile-refusal-{label}")
        for root in (case.python_root, case.rust_root):
            subprocess.run(["git", "init", "-q", str(root)], check=True)
            if label != "nonignored":
                (root / ".gitignore").write_text(".agentctl/\n", encoding="utf-8")
            directory = root / ".agentctl"
            directory.mkdir(mode=0o700)
            document = json.loads(json.dumps(base))
            if label == "unknown-field":
                document["profiles"]["worker"]["surprise"] = True
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
        subprocess.run(["git", "init", "-q", str(root)], check=True)
        (root / ".gitignore").write_text(".agentctl/\n", encoding="utf-8")
        directory = root / ".agentctl"
        directory.mkdir(mode=0o700)
        config = directory / "profiles.json"
        config.write_text(json.dumps(base), encoding="utf-8")
        config.chmod(0o600)
    outcomes = harness.invoke(overlap, (
        "start", "worker", "--cwd", "<ROOT>", "--profile", "worker", "--model", "override",
    ))
    report.require("primary/profile/refusal/precedence",
                   all(outcome.returncode == 2 for outcome in outcomes),
                   f"profile precedence was ambiguous: {outcomes!r}")
    outcomes = harness.invoke(overlap, (
        "start", "worker", "--cwd", "<ROOT>", "--profile", "worker", "--resume", "session-1",
    ))
    report.require("primary/profile/refusal/resume-precedence",
                   all(outcome.returncode == 2 for outcome in outcomes),
                   f"profile resume precedence was ambiguous: {outcomes!r}")

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

    adopt_muse = harness.case("primary-adopt-muse-refusal")
    outcomes = harness.invoke(adopt_muse, (
        "adopt", "worker", "--pane", "w1:p1", "--workspace", "project",
        "--cwd", "<ROOT>", "--harness", "muse", *_COMMON,
    ))
    report.require(
        "primary/adopt/muse-refusal",
        all(outcome.returncode == 75 and "adopting Muse is unsupported" in outcome.stderr
            for outcome in outcomes)
        and all(not (root / "registry").exists()
                for root in (adopt_muse.python_root, adopt_muse.rust_root)),
        f"Muse adoption was accepted or allocated state: {outcomes!r}",
    )

    hostile_path = harness.case("primary-profile-hostile-path", {"hostile_path": True})
    for root in (hostile_path.python_root, hostile_path.rust_root):
        subprocess.run(["/usr/bin/git", "init", "-q", str(root)], check=True)
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
            subprocess.run(["git", "init", "-q", str(root)], check=True)
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
    outcomes = harness.invoke(missing, (
        "start", "worker", "--cwd", "<ROOT>", "--workspace-id", "w1",
        "--harness", "muse", *_COMMON,
    ))
    report.require("primary/custom/missing-executable",
                   all(outcome.returncode == 75
                   and "cannot inspect muse executable" in outcome.stderr
                       for outcome in outcomes),
                   f"missing custom executable was not refused: {outcomes!r}")
    stopped = harness.invoke(missing, ("stop", "worker", *_COMMON))
    report.require("primary/custom/missing-executable-retained",
                   all(outcome.returncode == 69 for outcome in stopped)
                   and all((root / "registry/worker/agent.json").is_file()
                           and not _state(root).get("closed")
                           for root in (missing.python_root, missing.rust_root)),
                   f"unobserved missing-executable launch was incorrectly retired: {stopped!r}")

    wrong_process = harness.case(
        "primary-custom-wrong-process", {"wrong_custom_process": True}
    )
    outcomes = harness.invoke(wrong_process, (
        "start", "worker", "--cwd", "<ROOT>", "--workspace-id", "w1",
        "--harness", "muse", "--startup-timeout", "0.05", *_COMMON,
    ))
    report.require("primary/custom/exact-process",
                   all(outcome.returncode == 75 and "foreground process" in outcome.stderr
                       for outcome in outcomes),
                   f"lookalike custom process was accepted: {outcomes!r}")
    stopped = harness.invoke(wrong_process, ("stop", "worker", *_COMMON))
    report.require("primary/custom/exact-process-retained",
                   all(outcome.returncode == 69 for outcome in stopped)
                   and all((root / "registry/worker/agent.json").is_file()
                           and not _state(root).get("closed")
                           for root in (wrong_process.python_root, wrong_process.rust_root)),
                   f"unobserved wrong-process launch was incorrectly retired: {stopped!r}")
    _retire_fixture_processes(wrong_process)

    wrong_identity = harness.case(
        "primary-custom-wrong-kernel-identity", {"wrong_custom_process_identity": True}
    )
    outcomes = harness.invoke(wrong_identity, (
        "start", "worker", "--cwd", "<ROOT>", "--workspace-id", "w1",
        "--harness", "muse", "--startup-timeout", "0.05", *_COMMON,
    ))
    report.require("primary/custom/kernel-process-identity",
                   all(outcome.returncode == 75 and "foreground process" in outcome.stderr
                       for outcome in outcomes),
                   f"self-reported executable bypassed kernel identity: {outcomes!r}")
    stopped = harness.invoke(wrong_identity, ("stop", "worker", *_COMMON))
    report.require("primary/custom/kernel-process-retained",
                   all(outcome.returncode == 69 for outcome in stopped)
                   and all((root / "registry/worker/agent.json").is_file()
                           and not _state(root).get("closed")
                           for root in (wrong_identity.python_root, wrong_identity.rust_root)),
                   f"unobserved wrong-kernel launch was incorrectly retired: {stopped!r}")
    _retire_fixture_processes(wrong_identity)

    trust = harness.case("primary-muse-trust", {"screen": "Do you trust this workspace? yes / no"})
    outcomes = harness.invoke(trust, (
        "start", "worker", "--cwd", "<ROOT>", "--workspace-id", "w1",
        "--harness", "muse", *_COMMON,
    ))
    report.require("primary/muse/trust-refusal",
                   all(outcome.returncode == 75 and "trust prompt" in outcome.stderr
                       and _state(root).get("submitted") == []
                       for outcome, root in zip(outcomes, (trust.python_root, trust.rust_root), strict=True)),
                   f"Muse trust prompt was accepted or mutated: {outcomes!r}")
    stopped = harness.invoke(trust, ("stop", "worker", *_COMMON))
    report.require("primary/muse/failed-start-cleanup",
                   all(outcome.returncode == 0 for outcome in stopped)
                   and all(_state(root).get("closed_panes") == ["w1:p1"]
                           and not (root / "registry/worker").exists()
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
                and _state(root).get("submitted") == []
                and (root / "registry/worker/queue/failed/exit-race.json").is_file()
                for root in (exited.python_root, exited.rust_root)
            ),
            f"Muse exit between text and Enter submitted to a replacement: {outcomes!r}",
        )
        _retire_fixture_processes(exited)


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


def _legacy_adopted_registry_and_queue(harness: Harness, report: Report) -> None:
    """Read and drain one literal pre-profile adopted record and queue artifact."""

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
            "tab_id": "w1:t1",
            "pane_id": "w1:p1",
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
    _pair(
        harness, report, case, "primary/legacy-adopted/drain",
        ("drain", "foreign", *_COMMON),
    )
    snapshots = [
        _queue_snapshot(root, "registry/foreign/queue")
        for root in (case.python_root, case.rust_root)
    ]
    immutable_after_drain = [
        {
            relative: (root / "registry/foreign" / relative).read_bytes()
            for relative in immutable_paths
        }
        for root in (case.python_root, case.rust_root)
    ]
    processed = snapshots[0].get("processed/000000000007.json")
    report.require(
        "primary/legacy-adopted/drained-once",
        all(_state(root).get("submitted") == ["legacy fifo"]
            and not list((root / "registry/foreign/queue/inbox").iterdir())
            and (root / "registry/foreign/queue/processed/000000000007.json").is_file()
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
            (root / "registry/foreign/agent.json").is_file()
            and not _state(root).get("closed")
            and not _state(root).get("closed_panes")
            for root in (case.python_root, case.rust_root)
        ),
        "legacy adopted record without shell identity was retired or mutated",
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
                           and all(_state(root).get("closed_panes") == ["w1:p1"] and _state(root).get("extra_pane")
                                   for root in (case.python_root, case.rust_root)),
                           f"closing the owned pane also closed the human pane: {result!r}")
        else:
            _pair(harness, report, case, f"primary/ownership/{label}/status", ("status", "worker", *_COMMON))
            outcomes = harness.invoke(case, ("stop", "worker", *_COMMON))
            report.require(f"primary/ownership/{label}/stop-refusal", all(outcome.returncode == 69 for outcome in outcomes)
                           and all((root / "registry/worker/agent.json").exists() and not _state(root).get("closed")
                                   for root in (case.python_root, case.rust_root)), f"uncertain identity was retired: {outcomes!r}")


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
                       "session_value": "session-1", "capabilities": _CAPABILITIES,
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
                           and not (root / "registry/foreign").exists()
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
                and (root / "registry/foreign").exists() == (expected != 0)
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
                (root / "registry/foreign/agent.json").is_file()
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
        path = root / "registry/foreign/agent.json"
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
            (root / "registry/foreign/agent.json").is_file()
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
        ) and all(not (root / "registry/foreign").exists()
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
        ) and _state(root).get("submitted") == ["cross-engine adopted request"]
            and not _state(root).get("closed") and not _state(root).get("closed_panes")
            and len(list((root / "registry/archive").glob("*/agent.json"))) == 1,
            f"adopted record was not safely portable between editions: "
            f"{(adopted, status, sent, stopped)!r}")


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
        ("environment-missing-equals", ("start", "worker", "--env", "MISSING_EQUALS")),
        ("environment-empty-name", ("start", "worker", "--env", "=value")),
        ("environment-invalid-name", ("start", "worker", "--env", "BAD-NAME=value")),
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
        try:
            _orientation(harness, report)
            _lifecycle(harness, report)
            _profiles(harness, report)
            _skill_install(harness, report)
            _profile_refusals(harness, report)
            _handoff_and_pending(harness, report)
            _registry_and_interop(harness, report)
            _legacy_adopted_registry_and_queue(harness, report)
            _ownership(harness, report)
            _adoption(harness, report)
            _invalid_cli(harness, report)
        finally:
            harness.close()
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
