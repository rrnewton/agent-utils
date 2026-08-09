#!/usr/bin/env python3
"""Black-box Python/Rust differential for :command:`herdr-run`.

The commands under test are supplied by the caller as complete argv vectors.  This keeps the
harness independent of both packaging layouts and, importantly, never invokes a shell to locate or
compose either implementation.  The comparison covers bootstrap behavior, the command-policy
boundary, strict configuration loading/discovery, and the successful audited dry-run path.

Real pane execution is deliberately absent.  Both implementations resolve ``herdr`` only from
trusted installation locations and ignore caller-controlled ``PATH``.  There is consequently no
safe black-box way to inject a fake server without either weakening that production contract or
overwriting a user's real installation.  The report calls this limitation out explicitly.

Only values which are intrinsically different between two processes are normalized: temporary
roots, run IDs, timestamps, process IDs, and measured durations.  Policy text, quoting, exit codes,
JSON formatting, and diagnostics otherwise remain observable.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path
from typing import cast

REPO_ROOT = Path(__file__).resolve().parent.parent
TIMEOUT_SECONDS = 30

_RUN_ID_VALUE = re.compile(r'(?P<prefix>"run_id"\s*:\s*)"[^"]*"')
_RUN_ID_PATH = re.compile(r"\b\d{8}T\d{6}-[A-Za-z0-9_.-]+-\d+(?:-\d+)?\b")
_TIME_VALUE = re.compile(
    r'(?P<prefix>"(?:time|timestamp|started_at|finished_at)"\s*:\s*)"[^"]*"'
)
_PID_VALUE = re.compile(r'(?P<prefix>"(?:pid|shell_pid)"\s*:\s*)-?\d+')
_DURATION_VALUE = re.compile(
    r'(?P<prefix>"(?:duration|duration_seconds)"\s*:\s*)-?(?:\d+(?:\.\d*)?|\.\d+)'
)


@dataclass(frozen=True)
class Outcome:
    """One observable CLI result."""

    returncode: int
    stdout: str
    stderr: str


@dataclass
class Report:
    """Accumulated paired checks and actionable divergence descriptions."""

    checks: int = 0
    failures: list[str] = field(default_factory=list)
    notes: list[str] = field(default_factory=list)

    def require(self, label: str, condition: bool, detail: str) -> None:
        self.checks += 1
        if not condition:
            self.failures.append(f"{label}: {detail}")

    def exact(
        self, label: str, python: Outcome, rust: Outcome, *, expected_rc: int
    ) -> None:
        self.checks += 1
        if (
            python.returncode != expected_rc
            or rust.returncode != expected_rc
            or python != rust
        ):
            self.failures.append(
                f"{label}: expected rc {expected_rc}; "
                f"python={_describe(python)} rust={_describe(rust)}"
            )


@dataclass(frozen=True)
class PairCase:
    """Equivalent isolated projects, one for each implementation."""

    python_root: Path
    rust_root: Path


class Harness:
    """Create paired fixtures and invoke both supplied argv vectors hermetically."""

    def __init__(
        self,
        root: Path,
        python_command: Sequence[str],
        rust_command: Sequence[str],
    ) -> None:
        self.root = root
        self.python_command = _prepare_command(python_command)
        self.rust_command = _prepare_command(rust_command)
        self._serial = 0

    def case(self, label: str, files: Mapping[str, str] | None = None) -> PairCase:
        self._serial += 1
        safe_label = re.sub(r"[^A-Za-z0-9_.-]+", "-", label).strip("-") or "case"
        case_root = self.root / f"{self._serial:03d}-{safe_label}"
        # Keep the project basename identical: ``{project}`` is intentionally observable in tab
        # labels, so naming the two roots "python" and "rust" would manufacture a divergence.
        python_root = case_root / "python" / "project"
        rust_root = case_root / "rust" / "project"
        for project in (python_root, rust_root):
            project.mkdir(parents=True)
            (project / ".home").mkdir()
            if files is not None:
                for relative, text in files.items():
                    path = project / relative
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_text(text, encoding="utf-8")
        return PairCase(python_root, rust_root)

    def invoke(
        self,
        case: PairCase,
        args: Sequence[str],
        *,
        cwd: str = ".",
        extra_env: Mapping[str, str] | None = None,
    ) -> tuple[Outcome, Outcome]:
        python_cwd = case.python_root / cwd
        rust_cwd = case.rust_root / cwd
        python_cwd.mkdir(parents=True, exist_ok=True)
        rust_cwd.mkdir(parents=True, exist_ok=True)
        python = _run(
            self.python_command,
            args,
            cwd=python_cwd,
            home=case.python_root / ".home",
            extra_env=extra_env,
        )
        rust = _run(
            self.rust_command,
            args,
            cwd=rust_cwd,
            home=case.rust_root / ".home",
            extra_env=extra_env,
        )
        return (
            _normalize_outcome(python, case.python_root),
            _normalize_outcome(rust, case.rust_root),
        )


def _prepare_command(command: Sequence[str]) -> tuple[str, ...]:
    if not command:
        raise ValueError("implementation command argv must not be empty")
    prepared = list(command)
    executable = prepared[0]
    if os.sep in executable and not os.path.isabs(executable):
        prepared[0] = str(Path(executable).resolve())
    return tuple(prepared)


def _environment(home: Path, extra: Mapping[str, str] | None) -> dict[str, str]:
    environment = dict(os.environ)
    original_home = environment.get("HOME")
    existing_pythonpath = environment.get("PYTHONPATH", "")
    local_pythonpath = str(REPO_ROOT / "py")
    environment["PYTHONPATH"] = (
        local_pythonpath
        if not existing_pythonpath
        else local_pythonpath + os.pathsep + existing_pythonpath
    )
    environment.update(
        {
            "HOME": str(home),
            "LANG": "C.UTF-8",
            "LC_ALL": "C.UTF-8",
            "NO_COLOR": "1",
            "TZ": "UTC",
        }
    )
    # A development interpreter may have declared dependencies in its normal user site.  Changing
    # HOME for deterministic tilde expansion must not make that already-installed site disappear.
    if "PYTHONUSERBASE" not in environment and original_home:
        environment["PYTHONUSERBASE"] = str(Path(original_home) / ".local")
    if extra is not None:
        environment.update(extra)
    return environment


def _decode_output(value: bytes | str | None) -> str:
    if value is None:
        return ""
    if isinstance(value, bytes):
        return value.decode("utf-8", errors="replace")
    return value


def _run(
    command: Sequence[str],
    args: Sequence[str],
    *,
    cwd: Path,
    home: Path,
    extra_env: Mapping[str, str] | None,
) -> Outcome:
    try:
        completed = subprocess.run(
            [*command, *args],
            cwd=cwd,
            env=_environment(home, extra_env),
            capture_output=True,
            check=False,
            start_new_session=True,
            timeout=TIMEOUT_SECONDS,
        )
    except subprocess.TimeoutExpired as error:
        return Outcome(
            124,
            _decode_output(error.stdout),
            _decode_output(error.stderr)
            + f"\nTIMEOUT after {TIMEOUT_SECONDS} seconds\n",
        )
    except OSError as error:
        return Outcome(127, "", f"could not invoke implementation: {error}\n")
    return Outcome(
        completed.returncode,
        completed.stdout.decode("utf-8", errors="replace"),
        completed.stderr.decode("utf-8", errors="replace"),
    )


def _normalize_text(text: str, root: Path) -> str:
    normalized = text.replace(str(root), "<ROOT>")
    normalized = _RUN_ID_VALUE.sub(r'\g<prefix>"<RUN_ID>"', normalized)
    normalized = _RUN_ID_PATH.sub("<RUN_ID>", normalized)
    normalized = _TIME_VALUE.sub(r'\g<prefix>"<TIME>"', normalized)
    normalized = _PID_VALUE.sub(r"\g<prefix><PID>", normalized)
    normalized = _DURATION_VALUE.sub(r"\g<prefix><DURATION>", normalized)
    return normalized


def _normalize_outcome(outcome: Outcome, root: Path) -> Outcome:
    return Outcome(
        outcome.returncode,
        _normalize_text(outcome.stdout, root),
        _normalize_text(outcome.stderr, root),
    )


def _describe(outcome: Outcome) -> str:
    return (
        f"Outcome(rc={outcome.returncode}, stdout={outcome.stdout!r}, "
        f"stderr={outcome.stderr!r})"
    )


def _bootstrap(harness: Harness, report: Report) -> None:
    case = harness.case("bootstrap")
    python, rust = harness.invoke(case, ("--version",))
    report.exact("bootstrap/version", python, rust, expected_rc=0)
    version_pattern = re.compile(
        r"herdr-run (?P<version>\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?)\n"
    )
    python_version = version_pattern.fullmatch(python.stdout)
    rust_version = version_pattern.fullmatch(rust.stdout)
    report.require(
        "bootstrap/version-shape",
        python_version is not None
        and rust_version is not None
        and python.stderr == "",
        f"unexpected version output: python={_describe(python)} rust={_describe(rust)}",
    )
    report.require(
        "bootstrap/version-source-parity",
        python_version is not None
        and rust_version is not None
        and python_version.group("version") == rust_version.group("version"),
        "Python package release version and Rust Cargo package version differ: "
        f"python={_describe(python)} rust={_describe(rust)}",
    )

    help_python, help_rust = harness.invoke(case, ("--help",))
    required = (
        "check",
        "doctor",
        "config",
        "target",
        "userguide",
        "--version",
        "--userguide",
        "--config",
        "--agent",
        "--cwd",
        "--timeout",
        "--wait-ready",
        "--no-cache",
        "--json",
        "--dry-run",
    )
    for edition, outcome in (("python", help_python), ("rust", help_rust)):
        report.require(
            f"bootstrap/help-schema/{edition}",
            outcome.returncode == 0
            and outcome.stderr == ""
            and all(token in outcome.stdout for token in required),
            f"help omitted a required command/option: {_describe(outcome)}",
        )

    bare_python, bare_rust = harness.invoke(case, ())
    for edition, outcome in (("python", bare_python), ("rust", bare_rust)):
        report.require(
            f"bootstrap/bare/{edition}",
            outcome.returncode == 0
            and outcome.stderr == ""
            and "Nothing to do" in outcome.stdout
            and "userguide" in outcome.stdout,
            f"bare invocation did not print the successful bootstrap help: {_describe(outcome)}",
        )

    python, rust = harness.invoke(
        case,
        ("--dry-run", "--agent", "fixture-agent", "--", "git --help"),
    )
    report.exact("bootstrap/double-dash-command-help", python, rust, expected_rc=0)
    report.require(
        "bootstrap/double-dash-command-help-shape",
        python.stdout == "git --help\n" and python.stderr == "",
        f"command-local --help was intercepted: {_describe(python)}",
    )

    python, rust = harness.invoke(
        case,
        ("--dry", "--ag", "fixture-agent", "git status"),
    )
    report.require(
        "bootstrap/no-option-abbreviations",
        python.returncode == rust.returncode == 2
        and "traceback" not in (python.stderr + rust.stderr).lower()
        and "panicked" not in (python.stderr + rust.stderr).lower(),
        f"abbreviated options did not fail cleanly in both editions: "
        f"python={_describe(python)} rust={_describe(rust)}",
    )


def _policy(harness: Harness, report: Report) -> None:
    allowed = (
        ("git-status", "git status"),
        ("git-remote", "git ls-remote origin main"),
        ("prefix", "with-proxy git push origin HEAD:refs/heads/feature"),
        ("gh", "gh pr list --state open"),
        ("global-value", "git -C /tmp/repo log --oneline -5"),
        ("quoted-spaces", "git commit -m 'two words'"),
        ("empty-argument", "git commit -m ''"),
        ("mixed-quotes", "git commit -m pre\"mid\"'post'"),
        ("hash-literal", "gh issue view #12"),
        (
            "metacharacters-are-data",
            "git commit -m ';' '&&' '|' '$(id)' '`id`'",
        ),
        ("tilde", "git -C ~/work/repo status"),
    )
    for label, command in allowed:
        case = harness.case(f"policy-allow-{label}")
        python, rust = harness.invoke(case, ("check", command))
        report.exact(f"policy/allowed/{label}", python, rust, expected_rc=0)

    refused = (
        ("empty", ""),
        ("unbalanced-quote", "git status '"),
        ("curl", "curl https://example.invalid"),
        ("bash", "bash -lc true"),
        ("sh", "sh -c true"),
        ("python", "python3 -c pass"),
        ("rm", "rm -rf /tmp/example"),
        ("wrapper-only", "with-proxy"),
        ("wrapper-repeated", "with-proxy with-proxy git status"),
        ("absolute-program", "/bin/git status"),
        ("dot-program", "./git status"),
        ("parent-program", "../bin/gh pr list"),
        ("git-config", "git -c alias.pwn='!sh' pwn"),
        ("git-exec-path", "git --exec-path=/tmp log"),
        ("git-config-env", "git --config-env=x=y status"),
        ("git-namespace", "git --namespace=x status"),
        ("gh-alias", "gh alias set pwn '!sh'"),
        ("gh-extension", "gh extension exec pwn"),
        ("gh-ext", "gh ext exec pwn"),
        ("gh-codespace", "gh codespace ssh"),
        ("upload-pack", "git fetch --upload-pack=/tmp/evil origin"),
        ("receive-pack", "git push --receive-pack /tmp/evil origin"),
        ("control-bel", "git status\x07"),
        ("control-tab", "git\tstatus"),
        ("control-line-feed", "git status\nwhoami"),
        ("control-carriage-return", "git status\rwhoami"),
        ("control-escape", "git status\x1b[2J"),
        ("control-del", "git status\x7f"),
        ("control-c1", "git status\x85"),
        ("cargo-bare", "cargo"),
        ("cargo-fetch-default", "cargo fetch"),
        ("cargo-build", "cargo build --release"),
        ("cargo-unknown", "cargo something-new"),
        ("cargo-config", "cargo --config build.rustc-wrapper=/tmp/evil fetch"),
    )
    for label, command in refused:
        case = harness.case(f"policy-refuse-{label}")
        python, rust = harness.invoke(case, ("check", command))
        report.exact(f"policy/refused/{label}", python, rust, expected_rc=77)

    widened = {
        ".herdr-run.yaml": "allow: [git, cargo]\nprefixes: []\n",
    }
    for label, command in (
        ("fetch", "cargo fetch"),
        ("update", "cargo update -p serde"),
    ):
        case = harness.case(f"policy-custom-widened-{label}", widened)
        python, rust = harness.invoke(case, ("check", command))
        report.exact(f"policy/custom-widened/{label}", python, rust, expected_rc=0)

    for label, command in (
        ("config-before", "cargo --config build.rustc-wrapper=/tmp/evil fetch"),
        ("config-after", "cargo fetch --config build.rustc-wrapper=/tmp/evil"),
        ("config-attached-after", "cargo fetch --config=build.rustc-wrapper=/tmp/evil"),
        ("z-before-attached", "cargo -Zunstable-options fetch"),
        ("z-after", "cargo fetch -Z unstable-options"),
        ("z-after-attached", "cargo fetch -Zunstable-options"),
        ("z-after-clustered", "cargo fetch -qZunstable-options"),
    ):
        case = harness.case(f"policy-custom-widened-refuse-{label}", widened)
        python, rust = harness.invoke(case, ("check", command))
        report.exact(f"policy/custom-widened/refused/{label}", python, rust, expected_rc=77)

    minimum_cargo_guards = {
        ".herdr-run.yaml": (
            "allow: [cargo]\n"
            "deny_global: {}\n"
            "allow_subcommand:\n  cargo: [fetch]\n"
        ),
    }
    for label, command in (
        ("config-attached", "cargo fetch --config=x"),
        ("unstable-attached", "cargo fetch -Zunstable-options"),
    ):
        case = harness.case(f"policy-cargo-minimum-guard-{label}", minimum_cargo_guards)
        python, rust = harness.invoke(case, ("check", command))
        report.exact(f"policy/cargo-minimum-guard/{label}", python, rust, expected_rc=77)

    narrowed = {
        ".herdr-run.yaml": "allow: [git]\nprefixes: []\n",
    }
    case = harness.case("policy-custom-narrowed", narrowed)
    python, rust = harness.invoke(case, ("check", "gh pr list"))
    report.exact("policy/custom-narrowed", python, rust, expected_rc=77)


def _config_success(harness: Harness, report: Report) -> None:
    case = harness.case("config-default")
    python, rust = harness.invoke(case, ("config", "--agent", "fixture-agent"))
    report.exact("config/default", python, rust, expected_rc=0)
    report.require(
        "config/default-fields",
        '"source": "(built-in defaults)"' in python.stdout
        and '"project_root": "<ROOT>"' in python.stdout
        and '"tab_label": "fixture-agent"' in python.stdout,
        f"default config omitted resolved fields: {_describe(python)}",
    )

    case = harness.case("config-trimmed-environment-agent")
    python, rust = harness.invoke(
        case,
        ("config",),
        extra_env={
            "HERDR_RUN_AGENT": "   ",
            "DG_AGENT_NAME": "  fixture-agent  ",
            "ORC_AGENT_NAME": "ignored-agent",
        },
    )
    report.exact("config/trimmed-environment-agent", python, rust, expected_rc=0)
    report.require(
        "config/trimmed-environment-agent-value",
        '"tab_label": "fixture-agent"' in python.stdout,
        f"environment agent was not trimmed: {_describe(python)}",
    )

    full = """\
workspace: command-deck
tab_name: "{project}-{agent}"
cwd: work/tree
allow: [git, gh, cargo]
prefixes: [with-proxy, envwrap]
deny_global:
  git: [-c]
  gh: []
  cargo: []
deny_subcommand:
  git: [daemon]
  gh: [alias]
  cargo: [install]
deny_anywhere: [--upload-pack]
allow_subcommand:
  cargo: [fetch, update]
value_options:
  git: [-C]
  gh: [-R]
  cargo: [--manifest-path]
spool_dir: .state/herdr
timeout_seconds: 12.5
retention_days: 7
ready_timeout_seconds: 3
readiness: process
prompt_tail: "$ "
shells: [bash, zsh]
probe_remote: https://example.invalid/repository
broker: systemd-run
"""
    case = harness.case("config-full", {".herdr-run.yaml": full})
    python, rust = harness.invoke(case, ("config", "--agent", "fixture-agent"))
    report.exact("config/full", python, rust, expected_rc=0)

    discovery = {
        ".herdr-run.yaml": "workspace: outer\n",
        "slot/.herdr-run.yaml": "workspace: inner\ntab_name: '{project}-{agent}'\n",
    }
    case = harness.case("config-nearest", discovery)
    python, rust = harness.invoke(
        case,
        ("config", "--agent", "fixture-agent"),
        cwd="slot/deep",
    )
    report.exact("config/discovery-nearest", python, rust, expected_rc=0)
    report.require(
        "config/discovery-nearest-value",
        '"workspace": "inner"' in python.stdout
        and '"project_root": "<ROOT>/slot"' in python.stdout,
        f"nearest config was not selected: {_describe(python)}",
    )

    precedence = {
        ".herdr-run.yaml": "workspace: yaml-wins\n",
        ".herdr-run.yml": "workspace: yml-loses\n",
    }
    case = harness.case("config-extension-precedence", precedence)
    python, rust = harness.invoke(case, ("config",))
    report.exact("config/yaml-before-yml", python, rust, expected_rc=0)
    report.require(
        "config/yaml-before-yml-value",
        '"workspace": "yaml-wins"' in python.stdout,
        f".yaml did not win over .yml: {_describe(python)}",
    )

    yaml_12 = """\
workspace: yes
allow: [git, on, off]
timeout_seconds: 0o10
"""
    case = harness.case("config-yaml-12", {".herdr-run.yaml": yaml_12})
    python, rust = harness.invoke(case, ("config",))
    report.exact("config/yaml-1.2-scalars", python, rust, expected_rc=0)
    report.require(
        "config/yaml-1.2-scalar-values",
        '"workspace": "yes"' in python.stdout
        and '"on"' in python.stdout
        and '"off"' in python.stdout
        and '"timeout_seconds": 8.0' in python.stdout,
        f"YAML 1.2 core scalars resolved incorrectly: {_describe(python)}",
    )


def _malformed_contract(outcome: Outcome) -> bool:
    lowered = outcome.stderr.lower()
    return (
        outcome.returncode == 78
        and outcome.stdout == ""
        and bool(outcome.stderr.strip())
        and outcome.stderr.startswith("herdr-run:")
        and "traceback" not in lowered
        and "panicked at" not in lowered
    )


def _config_malformed(harness: Harness, report: Report) -> None:
    malformed = (
        ("invalid-yaml", "workspace: [\n"),
        ("duplicate-top-key", "workspace: one\nworkspace: two\n"),
        (
            "duplicate-nested-key",
            "deny_global:\n  git: []\n  git: [--exec-path]\n",
        ),
        ("merge-key", "deny_global:\n  <<: {git: []}\n"),
        ("nonfinite-inf", "timeout_seconds: .inf\n"),
        ("nonfinite-nan", "timeout_seconds: .nan\n"),
        ("unknown-key", "allowlist: [git]\n"),
        ("allow-scalar", "allow: git\n"),
        ("allow-non-string", "allow: [git, true]\n"),
        ("empty-allow", "allow: []\n"),
        (
            "cargo-positive-list-omitted",
            "allow: [git, cargo]\nallow_subcommand:\n  custom-tool: [inspect]\n",
        ),
        ("negative-timeout", "timeout_seconds: -1\n"),
        ("excessive-timeout", "timeout_seconds: 1e300\n"),
        ("negative-retention", "retention_days: -1\n"),
        ("fractional-retention", "retention_days: 1.5\n"),
        ("malformed-tab-template", 'tab_name: "{agent"\n'),
        ("attribute-tab-template", 'tab_name: "{agent.__class__}"\n'),
        ("format-tab-template", 'tab_name: "{agent:>10}"\n'),
        ("config-control", 'workspace: "bad\\x1bname"\n'),
        ("custom-tag", "workspace: !evil nope\n"),
        ("bad-readiness", "readiness: maybe\n"),
        ("bad-broker", "broker: shell\n"),
        ("huge-integer", "timeout_seconds: " + "9" * 5_000 + "\n"),
        ("unicode-surrogate", 'workspace: "\\uD800"\n'),
    )
    for label, document in malformed:
        case = harness.case(f"config-malformed-{label}", {".herdr-run.yaml": document})
        python, rust = harness.invoke(case, ("config",))
        report.require(
            f"config/malformed/{label}",
            _malformed_contract(python) and _malformed_contract(rust),
            "expected both editions to fail closed with rc 78, no stdout, and no "
            f"traceback/panic; python={_describe(python)} rust={_describe(rust)}",
        )

    case = harness.case("config-missing-explicit")
    python, rust = harness.invoke(case, ("--config", "missing.yaml", "config"))
    report.require(
        "config/malformed/missing-explicit",
        _malformed_contract(python) and _malformed_contract(rust),
        "missing explicit config did not fail as configuration error; "
        f"python={_describe(python)} rust={_describe(rust)}",
    )


def _read_normalized(path: Path, root: Path) -> Outcome:
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as error:
        return Outcome(1, "", f"cannot read {path}: {error}")
    return Outcome(0, _normalize_text(text, root), "")


def _dry_run(harness: Harness, report: Report) -> None:
    config = """\
allow: [printf]
prefixes: []
spool_dir: .audit-spool
"""
    case = harness.case("dry-run", {".herdr-run.yaml": config})
    command = "printf '[%s]' 'two words' '; rm -rf /' '$(id)'"
    python, rust = harness.invoke(
        case,
        ("--dry-run", "--json", "fixture-agent", command),
    )
    report.exact("dry-run/success-json", python, rust, expected_rc=0)
    expected = (
        "{\n"
        '  "program": "printf",\n'
        "  \"rendered\": \"printf '[%s]' 'two words' '; rm -rf /' '$(id)'\",\n"
        '  "verdict": "allowed"\n'
        "}\n"
    )
    report.require(
        "dry-run/success-json-shape",
        python.stdout == expected and python.stderr == "",
        f"dry-run JSON/rendering changed: {_describe(python)}",
    )

    python_audit = _read_normalized(
        case.python_root / ".audit-spool" / "audit.jsonl",
        case.python_root,
    )
    rust_audit = _read_normalized(
        case.rust_root / ".audit-spool" / "audit.jsonl",
        case.rust_root,
    )
    report.exact("dry-run/audit-jsonl", python_audit, rust_audit, expected_rc=0)


def build_report(
    python_command: Sequence[str],
    rust_command: Sequence[str],
) -> Report:
    """Run the complete differential and return its structured report without printing."""

    report = Report()
    with tempfile.TemporaryDirectory(prefix="herdr-run-cross-") as temporary:
        harness = Harness(Path(temporary), python_command, rust_command)
        _bootstrap(harness, report)
        _policy(harness, report)
        _config_success(harness, report)
        _config_malformed(harness, report)
        _dry_run(harness, report)
    report.notes.append(
        "external fake-Herdr lifecycle/protocol checks were not run: production resolution "
        "intentionally ignores caller PATH and exposes no safe executable override"
    )
    report.notes.append(
        "NUL cannot be represented in a POSIX argv entry; all other terminal-control classes are covered"
    )
    return report


def compare_herdr_run(
    python_command: Sequence[str],
    rust_command: Sequence[str],
) -> int:
    """Run, print, and return the status of the Python/Rust black-box comparison."""

    report = build_report(python_command, rust_command)
    for note in report.notes:
        print(f"NOTE [herdr-run] {note}")
    if report.failures:
        for failure in report.failures:
            print(f"DIVERGENCE [{failure}]")
        print(
            f"cross[herdr-run]: {len(report.failures)} divergence(s) "
            f"out of {report.checks} paired checks"
        )
        return 1
    print(f"cross[herdr-run]: OK - {report.checks} paired checks agree")
    return 0


def _decode_argv(encoded: str, option: str) -> list[str]:
    try:
        decoded: object = json.loads(encoded)
    except json.JSONDecodeError as error:
        raise ValueError(f"{option} must be valid JSON: {error}") from error
    if not isinstance(decoded, list):
        raise ValueError(f"{option} must decode to a JSON array of strings")
    values = cast(list[object], decoded)
    if not values or not all(isinstance(value, str) for value in values):
        raise ValueError(f"{option} must decode to a non-empty JSON array of strings")
    return [value for value in values if isinstance(value, str)]


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="black-box Python/Rust differential for herdr-run",
    )
    parser.add_argument(
        "--python-command-json",
        required=True,
        help='complete Python implementation argv as JSON, e.g. ["python3","-m","herdr_run"]',
    )
    parser.add_argument(
        "--rust-command-json",
        required=True,
        help='complete Rust implementation argv as JSON, e.g. ["rs/bin/herdr-run"]',
    )
    namespace = parser.parse_args(list(argv) if argv is not None else None)
    python_encoded = cast(str, namespace.python_command_json)
    rust_encoded = cast(str, namespace.rust_command_json)
    try:
        python_command = _decode_argv(python_encoded, "--python-command-json")
        rust_command = _decode_argv(rust_encoded, "--rust-command-json")
    except ValueError as error:
        parser.error(str(error))
    return compare_herdr_run(python_command, rust_command)


if __name__ == "__main__":
    raise SystemExit(main())
