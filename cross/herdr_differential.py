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


#: Every subcommand the surface promises, at both levels.
#:
#: Named literally rather than scraped from either edition, so an edition that quietly loses a
#: subcommand fails here instead of teaching the harness its own smaller idea of the surface.
_SUBCOMMANDS: tuple[str, ...] = (
    "check",
    "config",
    "init",
    "net-doctor",
    "quickstart",
    "reap",
    "run",
    "status",
    "target",
    "userguide",
)

#: Options accepted BEFORE the subcommand, and nowhere else.
_GLOBAL_OPTIONS = frozenset({"--help", "--version", "--config", "--agent", "--json"})

#: Options accepted AFTER each subcommand, and nowhere else.
_LOCAL_OPTIONS: dict[str, frozenset[str]] = {
    "run": frozenset(
        {"--help", "--cwd", "--timeout", "--wait-ready", "--no-cache", "--dry-run"}
    ),
    "check": frozenset({"--help"}),
    "config": frozenset({"--help"}),
    "init": frozenset({"--help", "--force"}),
    "status": frozenset({"--help"}),
    "target": frozenset({"--help", "--no-cache"}),
    "reap": frozenset({"--help"}),
    "net-doctor": frozenset({"--help"}),
    "quickstart": frozenset({"--help"}),
    "userguide": frozenset({"--help"}),
}

#: Every configuration key the two editions accept, and therefore every key `init` must write.
_CONFIG_KEYS: tuple[str, ...] = (
    "allow",
    "allow_subcommand",
    "broker",
    "cwd",
    "deny_anywhere",
    "deny_global",
    "deny_subcommand",
    "max_panes",
    "prefixes",
    "probe_remote",
    "prompt_tail",
    "readiness",
    "ready_timeout_seconds",
    "retention_days",
    "shells",
    "spool_dir",
    "tab_name",
    "timeout_seconds",
    "value_options",
    "workspace",
)

_OPTION_TOKEN = re.compile(r"--[a-z][a-z0-9-]*")


def _block(text: str, header_prefix: str) -> list[str]:
    """Return the contiguous indented lines that follow the first line starting with a header."""
    lines = text.splitlines()
    for index, line in enumerate(lines):
        if line.startswith(header_prefix):
            body: list[str] = []
            for candidate in lines[index + 1 :]:
                if not candidate.strip():
                    break
                body.append(candidate)
            return body
    return []


def _declared_subcommands(text: str) -> tuple[str, ...]:
    return tuple(
        sorted(line.split()[0] for line in _block(text, "subcommands:") if line.split())
    )


def _declared_options(text: str, header_prefix: str) -> frozenset[str]:
    return frozenset(
        token
        for line in _block(text, header_prefix)
        for token in _OPTION_TOKEN.findall(line)
    )


def _help_surface(harness: Harness, report: Report, case: PairCase) -> None:
    """Compare the whole two-level help schema, not merely that some words appear in it.

    ``report.exact`` on every help text already makes one edition growing a subcommand or an option
    the other lacks a divergence.  The literal expectations below add the case ``exact`` cannot
    catch: both editions dropping the same thing at once, which is agreement about a surface that
    no longer matches what the tool promises.
    """

    python, rust = harness.invoke(case, ("--help",))
    report.exact("help/top-level", python, rust, expected_rc=0)
    report.require(
        "help/top-level-subcommands",
        _declared_subcommands(python.stdout) == tuple(sorted(_SUBCOMMANDS)),
        f"top-level help does not list exactly {sorted(_SUBCOMMANDS)}: {_describe(python)}",
    )
    report.require(
        "help/top-level-global-options",
        _declared_options(python.stdout, "global options") == _GLOBAL_OPTIONS,
        f"global option list changed: {_describe(python)}",
    )
    # The two levels must not document each other. This is the defect the surface exists to fix.
    top_level_local = _declared_options(python.stdout, "global options") & {
        option for options in _LOCAL_OPTIONS.values() for option in options
    } - {"--help"}
    report.require(
        "help/top-level-omits-local-options",
        not top_level_local,
        f"top-level help documents subcommand options {sorted(top_level_local)}",
    )

    for subcommand in _SUBCOMMANDS:
        python, rust = harness.invoke(case, (subcommand, "--help"))
        report.exact(f"help/{subcommand}", python, rust, expected_rc=0)
        declared = _declared_options(python.stdout, "options:")
        report.require(
            f"help/{subcommand}-options",
            declared == _LOCAL_OPTIONS[subcommand],
            f"'{subcommand} --help' declares {sorted(declared)}, "
            f"expected {sorted(_LOCAL_OPTIONS[subcommand])}: {_describe(python)}",
        )
        leaked = declared & (_GLOBAL_OPTIONS - {"--help"})
        report.require(
            f"help/{subcommand}-omits-global-options",
            not leaked,
            f"'{subcommand} --help' documents global options {sorted(leaked)}",
        )


def _levels(harness: Harness, report: Report, case: PairCase) -> None:
    """An option offered at the wrong level must be refused, identically, by both editions."""

    misplaced: tuple[tuple[str, tuple[str, ...]], ...] = (
        ("global-cwd", ("--cwd", "/tmp", "run", "git status")),
        ("global-timeout", ("--timeout", "5", "run", "git status")),
        ("global-wait-ready", ("--wait-ready", "5", "run", "git status")),
        ("global-no-cache", ("--no-cache", "target")),
        ("global-dry-run", ("--dry-run", "run", "git status")),
        ("local-agent", ("run", "--agent", "fixture-agent", "git status")),
        ("local-config", ("run", "--config", "x.yaml", "git status")),
        ("local-json", ("run", "--json", "git status")),
        ("foreign-dry-run", ("check", "--dry-run", "git status")),
        ("foreign-cwd", ("reap", "--cwd", "/tmp")),
        # Right level, wrong subcommand: `--config` is global, but these three read no
        # configuration file, so accepting it would be accepting an instruction and then
        # disregarding it. `--config P init` is the acute one: it reads as "write P".
        ("config-blind-init", ("--config", "x.yaml", "init")),
        ("config-blind-init-inline", ("--config=x.yaml", "init")),
        ("config-blind-init-force", ("--config", "x.yaml", "init", "--force")),
        ("config-blind-quickstart", ("--config", "x.yaml", "quickstart")),
        ("config-blind-userguide", ("--config", "x.yaml", "userguide")),
        # `--agent NAME` says who this invocation speaks for, and the one thing that does is name
        # a tab. These five resolve no tab, so accepting it would be accepting an instruction and
        # then doing nothing about it.
        ("agent-blind-check", ("--agent", "a", "check", "git status")),
        ("agent-blind-check-inline", ("--agent=a", "check", "git status")),
        ("agent-blind-init", ("--agent", "a", "init")),
        ("agent-blind-reap", ("--agent", "a", "reap")),
        ("agent-blind-quickstart", ("--agent", "a", "quickstart")),
        ("agent-blind-userguide", ("--agent", "a", "userguide")),
        # `--json` promises machine-readable output where a subcommand has it; these three have
        # only prose to give, so a caller who was humoured would fail at the parse of the output
        # rather than at the flag.
        ("json-blind-net-doctor", ("--json", "net-doctor")),
        ("json-blind-quickstart", ("--json", "quickstart")),
        ("json-blind-userguide", ("--json", "userguide")),
    )
    for label, args in misplaced:
        python, rust = harness.invoke(case, args)
        report.exact(f"levels/{label}", python, rust, expected_rc=2)

    # Two blind globals at once must produce ONE refusal, and the same one in both editions.
    # `report.exact` already pins the agreement; naming --config pins which of the two it is, so
    # the editions cannot agree by both drifting to the other.
    python, rust = harness.invoke(case, ("--config", "x.yaml", "--agent", "a", "init"))
    report.exact("levels/two-blind-globals-give-one-refusal", python, rust, expected_rc=2)
    report.require(
        "levels/two-blind-globals-report-the-first",
        "argument --config:" in python.stderr and "argument --agent:" not in python.stderr,
        f"a --config/--agent pair did not report --config alone: {_describe(python)}",
    )

    # The globals a subcommand CAN observe stay accepted: the refusals are exactly as wide as the
    # blindness. `check` is the one that both refuses a global and accepts another.
    for label, args in (
        ("agent-sighted-run", ("--agent", "a", "run", "--dry-run", "git status")),
        ("json-sighted-check", ("--json", "check", "git status")),
        ("json-sighted-config", ("--json", "config")),
    ):
        python, rust = harness.invoke(case, args)
        report.exact(f"levels/{label}", python, rust, expected_rc=0)

    # The subcommand's own parse still wins, so `--help` and a bad local option are unaffected.
    python, rust = harness.invoke(case, ("--config", "x.yaml", "init", "--help"))
    report.exact("levels/config-blind-init-still-helps", python, rust, expected_rc=0)
    python, rust = harness.invoke(case, ("--config", "x.yaml", "init", "--nonsense"))
    report.exact("levels/config-blind-init-names-local-option", python, rust, expected_rc=2)
    report.require(
        "levels/config-blind-init-names-local-option-first",
        "--nonsense" in python.stderr,
        f"the local option error was replaced by the --config refusal: {_describe(python)}",
    )
    python, rust = harness.invoke(case, ("--agent", "a", "quickstart", "--nonsense"))
    report.exact("levels/agent-blind-names-local-option", python, rust, expected_rc=2)
    report.require(
        "levels/agent-blind-names-local-option-first",
        "--nonsense" in python.stderr,
        f"the local option error was replaced by the --agent refusal: {_describe(python)}",
    )


def _removed_bare_form(harness: Harness, report: Report, case: PairCase) -> None:
    """Running a command with no subcommand must fail, and must name what to type instead."""

    python, rust = harness.invoke(case, ("git status",))
    report.exact("bare-form/command-only", python, rust, expected_rc=2)
    report.require(
        "bare-form/command-only-names-run",
        "herdr-run run 'git status'" in python.stderr,
        f"the removed bare form did not name its replacement: {_describe(python)}",
    )

    python, rust = harness.invoke(case, ("release-agent", "git status"))
    report.exact("bare-form/agent-and-command", python, rust, expected_rc=2)
    report.require(
        "bare-form/agent-and-command-names-run",
        "herdr-run --agent release-agent run 'git status'" in python.stderr,
        f"the removed agent form did not name its replacement: {_describe(python)}",
    )

    python, rust = harness.invoke(case, ("run", "git", "status"))
    report.exact("bare-form/loose-words", python, rust, expected_rc=2)
    report.require(
        "bare-form/loose-words-refuse-rejoining",
        "ONE quoted argument" in python.stderr,
        f"loose command words were not refused: {_describe(python)}",
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

    _help_surface(harness, report, case)
    _levels(harness, report, case)
    _removed_bare_form(harness, report, case)

    bare_python, bare_rust = harness.invoke(case, ())
    report.exact("bootstrap/bare", bare_python, bare_rust, expected_rc=0)
    for edition, outcome in (("python", bare_python), ("rust", bare_rust)):
        report.require(
            f"bootstrap/bare/{edition}",
            outcome.returncode == 0
            and outcome.stderr == ""
            and "subcommands:" in outcome.stdout,
            f"a bare invocation must print the subcommand list: {_describe(outcome)}",
        )

    python, rust = harness.invoke(
        case,
        ("--agent", "fixture-agent", "run", "--dry-run", "--", "git --help"),
    )
    report.exact("bootstrap/double-dash-command-help", python, rust, expected_rc=0)
    report.require(
        "bootstrap/double-dash-command-help-shape",
        python.stdout == "git --help\n" and python.stderr == "",
        f"command-local --help was intercepted: {_describe(python)}",
    )

    python, rust = harness.invoke(
        case,
        ("--ag", "fixture-agent", "run", "--dry", "git status"),
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
    python, rust = harness.invoke(case, ("--agent", "fixture-agent", "config"))
    report.exact("config/default", python, rust, expected_rc=0)
    report.require(
        "config/default-fields",
        '"source": "(built-in defaults)"' in python.stdout
        and '"project_root": "<ROOT>"' in python.stdout
        and '"tab_label": "fixture-agent"' in python.stdout
        and '"max_panes": 32' in python.stdout,
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
max_panes: 12
ready_timeout_seconds: 3
readiness: process
prompt_tail: "$ "
shells: [bash, zsh]
probe_remote: https://example.invalid/repository
broker: systemd-run
"""
    case = harness.case("config-full", {".herdr-run.yaml": full})
    python, rust = harness.invoke(case, ("--agent", "fixture-agent", "config"))
    report.exact("config/full", python, rust, expected_rc=0)

    discovery = {
        ".herdr-run.yaml": "workspace: outer\n",
        "slot/.herdr-run.yaml": "workspace: inner\ntab_name: '{project}-{agent}'\n",
    }
    case = harness.case("config-nearest", discovery)
    python, rust = harness.invoke(
        case,
        ("--agent", "fixture-agent", "config"),
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
        ("negative-max-panes", "max_panes: -1\n"),
        ("fractional-max-panes", "max_panes: 1.5\n"),
        ("boolean-max-panes", "max_panes: true\n"),
        ("excessive-max-panes", "max_panes: 1000001\n"),
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
        ("--json", "--agent", "fixture-agent", "run", "--dry-run", command),
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


#: Workspace label used by the reap fixtures. Deliberately not a name any real session would carry,
#: so a reachable Herdr server cannot resolve it. A host with no reachable server reports the
#: safety-equivalent unavailable-evidence reason instead; both paths must keep the pane UNKNOWN.
_REAP_WORKSPACE = "cross-differential-fixture"

_REAP_CONFIG = f"""\
workspace: {_REAP_WORKSPACE}
allow: [printf]
prefixes: []
spool_dir: .herdr-run
retention_days: 7
"""


def _reap_record(pane_id: str, workspace: str, exit_code: str = "0") -> str:
    """One planted ``meta.json``, shaped exactly as the runner writes it."""
    return json.dumps(
        {
            "agent": "kvm",
            "exit_code": None if exit_code == "null" else int(exit_code),
            "pane_id": pane_id,
            "readiness": {
                "boot_id": "3f2b1c8e-0000-4000-8000-000000000001",
                "shell_pid": 4242,
                "shell_start_ticks": 900,
            },
            "run_id": "20260819T000000-kvm-1",
            "tab": {"id": "w1:t1", "label": "kvm"},
            "workspace": {"id": "w1", "label": workspace},
        },
        indent=2,
        sort_keys=True,
    )


def _workspace_is_safely_unresolvable(stdout: str) -> bool:
    """The two safe reasons a planted workspace cannot establish live-pane evidence."""

    return (
        "no workspace labelled" in stdout
        or "evidence unavailable: workspace list:" in stdout
    )


def _reap(harness: Harness, report: Report) -> None:
    """Compare the REAPING VERDICTS, not just that both editions list the subcommand.

    ``reap`` is the one subcommand whose output can authorise destroying someone's running work, so
    "both help texts contain the word reap" is not a cross-edition guarantee of anything. These
    cases plant a run spool and compare the whole document byte for byte: counts, per-pane verdicts,
    reason wording, key ordering, and the retention window the candidate set is bounded by.

    Only the host-independent verdicts can be paired here. STALE and SHELL_ALIVE need a pane a live
    Herdr server still lists, and this harness deliberately cannot stand one up; those two are
    covered by the planted-population unit tests in both editions instead.
    """
    empty = harness.case("reap-empty", {".herdr-run.yaml": _REAP_CONFIG})
    python, rust = harness.invoke(empty, ("reap",))
    report.exact("reap/empty-spool", python, rust, expected_rc=0)
    report.require(
        "reap/empty-spool-shape",
        python.stderr == ""
        and '"considered": 0' in python.stdout
        and all(
            f'"{verdict}": 0' in python.stdout
            for verdict in ("STALE", "IN_FLIGHT", "SHELL_ALIVE", "UNKNOWN", "OUT_OF_SCOPE")
        )
        and '"retention_days": 7' in python.stdout
        and f'"workspace": "{_REAP_WORKSPACE}"' in python.stdout,
        "an inert sweep must still print its own shape, and the window bounding it: "
        f"{_describe(python)}",
    )

    scoped = harness.case(
        "reap-in-scope",
        {
            ".herdr-run.yaml": _REAP_CONFIG,
            ".herdr-run/runs/20260819T000000-kvm-1/meta.json": _reap_record(
                "w1:p1", _REAP_WORKSPACE
            ),
        },
    )
    python, rust = harness.invoke(scoped, ("reap",))
    report.exact("reap/unresolvable-workspace", python, rust, expected_rc=0)
    report.require(
        "reap/unresolvable-workspace-shape",
        '"STALE": 0' in python.stdout
        and '"UNKNOWN": 1' in python.stdout
        and '"reapable": []' in python.stdout
        and _workspace_is_safely_unresolvable(python.stdout),
        "a workspace herdr cannot resolve must reap nothing and say why: "
        f"{_describe(python)}",
    )

    foreign = harness.case(
        "reap-out-of-scope",
        {
            ".herdr-run.yaml": _REAP_CONFIG,
            ".herdr-run/runs/20260819T000000-kvm-1/meta.json": _reap_record(
                "w1:p1", "someone-elses"
            ),
        },
    )
    python, rust = harness.invoke(foreign, ("reap",))
    report.exact("reap/out-of-scope", python, rust, expected_rc=0)
    report.require(
        "reap/out-of-scope-shape",
        '"OUT_OF_SCOPE": 1' in python.stdout
        and '"STALE": 0' in python.stdout
        and '"reapable": []' in python.stdout,
        f"a pane recorded in another workspace must never be a candidate: {_describe(python)}",
    )


#: `status` fixtures name a workspace no real session would carry, so a host that happens to be
#: running a live Herdr server cannot resolve it and the report stays the same everywhere.
_STATUS_CONFIG = f"""\
workspace: {_REAP_WORKSPACE}
allow: [git, gh]
prefixes: [with-proxy]
spool_dir: .herdr-run
"""

_STATUS_ANY_CONFIG = f"""\
workspace: {_REAP_WORKSPACE}
allow: ["*"]
prefixes: []
"""


#: The one heading whose section is allowed to differ between the two editions.
_INSTALLATION_HEADING = "## Installation"


def _split_installation(text: str) -> tuple[str, str]:
    """Split a user guide into (everything else, the per-edition installation section).

    The two guides ship ONE deliberately different section: how you install THAT edition — `pip`
    against `cargo`, a Python version against a Rust version. Everything else — the subcommands,
    the options, the exit codes, the retention rules, the trust model — is one shared source, so it
    is compared byte for byte with that single section lifted out rather than the comparison being
    abandoned because one paragraph is allowed to differ.
    """
    lines = text.splitlines(keepends=True)
    start = next(
        (
            index
            for index, line in enumerate(lines)
            if line.startswith(_INSTALLATION_HEADING)
        ),
        None,
    )
    if start is None:
        return text, ""
    end = next(
        (index for index in range(start + 1, len(lines)) if lines[index].startswith("## ")),
        len(lines),
    )
    return "".join(lines[:start] + lines[end:]), "".join(lines[start:end])


def _documentation(harness: Harness, report: Report) -> None:
    """Both documentation commands must say the same thing in both editions.

    `quickstart` is one shared source and is compared byte for byte. The user guide is shared
    everywhere except its installation section, which names the edition's own package manager on
    purpose, so it is compared byte for byte with that one section lifted out — and the lifted
    sections are separately required to DIFFER, so a normalisation that quietly removed the whole
    document could not pass.

    The two documents are also compared to each other: `quickstart` is supposed to be a much
    shorter amended version, and two commands printing the same text would be one command with two
    names.
    """

    case = harness.case("documentation")
    quickstart_python, quickstart_rust = harness.invoke(case, ("quickstart",))
    report.exact("documentation/quickstart", quickstart_python, quickstart_rust, expected_rc=0)
    guide_python, guide_rust = harness.invoke(case, ("userguide",))
    shared_python, install_python = _split_installation(guide_python.stdout)
    shared_rust, install_rust = _split_installation(guide_rust.stdout)
    report.exact(
        "documentation/userguide-outside-installation",
        Outcome(guide_python.returncode, shared_python, guide_python.stderr),
        Outcome(guide_rust.returncode, shared_rust, guide_rust.stderr),
        expected_rc=0,
    )
    report.require(
        "documentation/userguide-installation-is-the-only-difference",
        bool(install_python)
        and bool(install_rust)
        and install_python != install_rust
        and len(shared_python) > 500,
        "the installation section is the ONE section allowed to differ, and it must be found in "
        f"both editions and actually differ: python={install_python!r} rust={install_rust!r}",
    )
    report.require(
        "documentation/quickstart-is-shorter",
        quickstart_python.stdout != guide_python.stdout
        and 2 * quickstart_python.stdout.count("\n") < guide_python.stdout.count("\n"),
        "quickstart is supposed to be a much shorter amended version of the guide: "
        f"{quickstart_python.stdout.count(chr(10))} lines against "
        f"{guide_python.stdout.count(chr(10))}",
    )

    # A configuration file too broken to load must not be able to withhold the documentation.
    broken = harness.case("documentation-broken-config", {".herdr-run.yaml": "allow: [\n"})
    python, rust = harness.invoke(broken, ("quickstart",))
    report.exact("documentation/quickstart-despite-broken-config", python, rust, expected_rc=0)
    python, rust = harness.invoke(broken, ("userguide",))
    report.require(
        "documentation/userguide-despite-broken-config",
        python == guide_python and rust == guide_rust,
        "a configuration file too broken to load withheld the user guide: "
        f"python={_describe(python)} rust={_describe(rust)}",
    )

    help_python, _ = harness.invoke(case, ("--help",))
    report.require(
        "documentation/help-distinguishes-them",
        "quickstart" in help_python.stdout
        and "userguide" in help_python.stdout
        and "Two documentation commands" in help_python.stdout,
        f"the top-level help does not say what each document is for: {_describe(help_python)}",
    )


def _status(harness: Harness, report: Report) -> None:
    """`status` must read the same and say the same, and must still be read-only in both."""

    case = harness.case("status", {".herdr-run.yaml": _STATUS_CONFIG})
    python, rust = harness.invoke(case, ("--agent", "fixture-agent", "status"))
    report.exact("status/text", python, rust, expected_rc=0)
    for fragment in (
        "    file          <ROOT>/.herdr-run.yaml\n",
        "    project root  <ROOT>\n",
        "    spool dir     .herdr-run\n",
        "    allow         git, gh\n",
        "    prefixes      with-proxy\n",
        "    agent         fixture-agent\n",
        f"    workspace     {_REAP_WORKSPACE}\n",
        "    tab label     fixture-agent",
        "\nNothing was changed: status only reads.\n",
    ):
        report.require(
            f"status/text-says/{fragment.split()[0]}",
            fragment in python.stdout,
            f"status omitted {fragment!r}: {_describe(python)}",
        )

    json_python, json_rust = harness.invoke(
        case, ("--json", "--agent", "fixture-agent", "status")
    )
    report.exact("status/json", json_python, json_rust, expected_rc=0)
    report.require(
        "status/json-shape",
        '"agent": "fixture-agent"' in json_python.stdout
        and '"allow_any_program": false' in json_python.stdout
        and '"config_file": "<ROOT>/.herdr-run.yaml"' in json_python.stdout
        and '"label": "fixture-agent"' in json_python.stdout
        and f'"workspace": "{_REAP_WORKSPACE}"' in json_python.stdout,
        f"status JSON changed shape: {_describe(json_python)}",
    )

    # Nothing status did may have created state on disk either.
    for edition, root in (("python", case.python_root), ("rust", case.rust_root)):
        report.require(
            f"status/creates-nothing/{edition}",
            not (root / ".herdr-run").exists(),
            "status created spool state in a directory it was only supposed to describe",
        )

    wildcard = harness.case("status-allow-any", {".herdr-run.yaml": _STATUS_ANY_CONFIG})
    python, rust = harness.invoke(wildcard, ("--agent", "fixture-agent", "status"))
    report.exact("status/allow-everything", python, rust, expected_rc=0)
    report.require(
        "status/allow-everything-shape",
        '    allow         any program ("*")\n' in python.stdout
        and "    prefixes      (none)\n" in python.stdout,
        f"the allow-everything mode was not reported as such: {_describe(python)}",
    )


def _init(harness: Harness, report: Report) -> None:
    """`init` must write the same bytes in both editions, and both must then read them the same.

    Comparing the generated `.herdr-run.yaml` byte for byte is what lets the user guide point at
    that file instead of restating it: the file is the reference, so a reference that differed
    between editions would be two references.
    """

    # `--config PATH` reads as "write PATH" here and would in fact write ./.herdr-run.yaml, so it
    # is refused. The refusal has to be total: nothing written, in either edition.
    refused = harness.case("init-config-refused")
    python, rust = harness.invoke(refused, ("--config", "elsewhere.yaml", "init"))
    report.exact("init/config-is-refused", python, rust, expected_rc=2)
    report.require(
        "init/config-refusal-writes-nothing",
        not (refused.python_root / ".herdr-run.yaml").exists()
        and not (refused.rust_root / ".herdr-run.yaml").exists(),
        "a refused '--config PATH init' still wrote a configuration file",
    )

    # `--agent NAME init` is refused for the same reason and must be just as total: `init` names no
    # tab, so the name has nowhere to go, and a refusal that still wrote the file would be worse
    # than accepting the flag.
    refused = harness.case("init-agent-refused")
    python, rust = harness.invoke(refused, ("--agent", "a", "init"))
    report.exact("init/agent-is-refused", python, rust, expected_rc=2)
    report.require(
        "init/agent-refusal-writes-nothing",
        not (refused.python_root / ".herdr-run.yaml").exists()
        and not (refused.rust_root / ".herdr-run.yaml").exists(),
        "a refused '--agent NAME init' still wrote a configuration file",
    )

    case = harness.case("init")
    python, rust = harness.invoke(case, ("init",))
    report.exact("init/first-write", python, rust, expected_rc=0)

    python_file = _read_normalized(case.python_root / ".herdr-run.yaml", case.python_root)
    rust_file = _read_normalized(case.rust_root / ".herdr-run.yaml", case.rust_root)
    report.exact("init/template-bytes", python_file, rust_file, expected_rc=0)
    for marker in (
        "a human-only knob",
        "DO NOT LET AN AGENT EDIT THIS SECTION",
        "worktrees/slotNN/",
        "ALLOW-EVERYTHING MODE",
        'allow: ["*"]',
    ):
        report.require(
            f"init/template-says/{marker[:24]}",
            marker in python_file.stdout,
            f"the generated configuration never says {marker!r}",
        )

    # Every knob has to be there, or the guide cannot point at this file instead of listing them.
    missing = [key for key in _CONFIG_KEYS if f"\n{key}:" not in python_file.stdout]
    report.require(
        "init/template-covers-every-key",
        not missing,
        f"the generated configuration never sets {missing}",
    )

    resolved_python, resolved_rust = harness.invoke(
        case, ("--agent", "fixture-agent", "config")
    )
    report.exact("init/config-after-init", resolved_python, resolved_rust, expected_rc=0)

    python, rust = harness.invoke(case, ("init",))
    report.exact("init/refuses-to-clobber", python, rust, expected_rc=78)
    report.require(
        "init/refuses-to-clobber-names-force",
        "--force" in python.stderr and python.stdout == "",
        f"a refused init must name --force on stderr and print nothing: {_describe(python)}",
    )

    python, rust = harness.invoke(case, ("init", "--force"))
    report.exact("init/force-overwrites", python, rust, expected_rc=0)


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
        _init(harness, report)
        _status(harness, report)
        _documentation(harness, report)
        _reap(harness, report)
    report.notes.append(
        "external fake-Herdr lifecycle/protocol checks were not run: production resolution "
        "intentionally ignores caller PATH and exposes no safe executable override"
    )
    report.notes.append(
        "reap is paired on safety-equivalent verdicts (empty spool, unresolvable or unavailable "
        "workspace evidence, out of scope); STALE and SHELL_ALIVE need a pane a live Herdr server "
        "still lists, and are covered by planted-population unit tests in each edition instead"
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
