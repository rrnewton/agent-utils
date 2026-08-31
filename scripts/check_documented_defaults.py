#!/usr/bin/env python3
"""Pin every number a herdr-run document states as a default to the constant in the code.

The guides are shipped artifacts: `common/docs/herdr-run/AGENT_USER_GUIDE.md` is embedded into the
crate and symlinked into the wheel, the rendered user guides go inside both distributions, and
`CONFIG_TEMPLATE.yaml` is what `herdr-run init` writes into a project. Agents read those documents
as fact. Until this guard existed, "up to `--ready-timeout` (900 seconds by default)" was held true
by nothing at all: the constant lives in four places across two implementations, and changing them
left the sentence quietly asserting the old number.

**This is not a self-fulfilling test.** The repository has repeatedly written checks that read a
constant, format the very sentence they then compare against, and pass whatever the constant is.
The comparison here is between two SEPARATE artifacts: a number parsed out of prose that nothing
generated, and a number parsed out of the source that defines it. Neither side is derived from the
other, and `--self-test` proves it the only way that means anything — by mutating each code
constant in memory and requiring this check to go red and name the document.

What is pinned, and the rule for each:

* A pin's CODE sites must all agree. `ready_timeout` is written out in four places (Rust default
  struct, Rust CLI default, Python signature default, Python argparse default); an edition that
  changes one of them is a bug before any document is consulted.
* A pin's DOC sites must each match at least once, and EVERY occurrence must equal the code value.
  At-least-once matters as much as the equality: a sentence that is reworded until the pattern
  stops matching would otherwise make this guard silently vacuous, so zero matches is a failure
  that names the file.

Numbers are compared numerically, so `900` in prose, `900.0` in Rust and `900.0` in Python are the
same number, `31,536,000` equals `31_536_000.0`, and a spelled-out `four` equals `4` — the guides
write small counts as words, and that spelling drifts exactly as easily as a digit.

Not covered, deliberately:

* **The other tools' guides.** `dagrun` states numbers in prose too — a two-hundred step ceiling,
  an 85%-of-`MemTotal` budget with an 8 GiB margin. They are real drift candidates and each is one
  more row in `PINS`, but they belong to a different tool and a different survey; this landed with
  `#88 herdr-run-pin-documented-defaults`, which is herdr-run's.
* **Exit codes.** The guides tabulate `75`/`76`/`77`/`78`, but those are a wire contract already
  asserted by the Python and Rust suites and by the cross-language differential, so a change to one
  cannot reach main with the table still standing.
* **The template's NON-numeric defaults** — `workspace`, `spool_dir`, `readiness`, `broker`,
  `probe_remote`, `shells`. `CONFIG_TEMPLATE.yaml` promises those are the tool's values too, and
  they can drift the same way; this guard compares numbers, which is the drift `#88` names, and
  pinning a string wants a second comparison rather than a tenth row.

Usage:
    python3 scripts/check_documented_defaults.py
    python3 scripts/check_documented_defaults.py --self-test
"""

from __future__ import annotations

import argparse
import re
import sys
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

#: Reads a repository-relative path and returns its text. Injectable so `--self-test` can hand the
#: real check a MUTATED copy of a real file and watch it fail.
Reader = Callable[[str], str]

#: Small counts the guides spell out. Only as far as the guides actually go: an unknown word is a
#: parse failure, not a silent skip, so extending the prose past this needs a deliberate edit here.
NUMBER_WORDS: dict[str, float] = {
    "zero": 0,
    "one": 1,
    "two": 2,
    "three": 3,
    "four": 4,
    "five": 5,
    "six": 6,
    "seven": 7,
    "eight": 8,
    "nine": 9,
    "ten": 10,
    "eleven": 11,
    "twelve": 12,
}


class Unparseable(Exception):
    """A captured number that is neither a numeral nor a word this guard knows."""


def parse_number(raw: str) -> float:
    """`1,000,000`, `31_536_000.0` and `four` all become a float."""
    cleaned = raw.strip().replace(",", "").replace("_", "")
    word = NUMBER_WORDS.get(cleaned.lower())
    if word is not None:
        return word
    try:
        return float(cleaned)
    except ValueError as exc:
        raise Unparseable(f"{raw!r} is not a number this guard can read") from exc


def show(value: float) -> str:
    """Render a number the way a reader would say it: `900`, not `900.0`."""
    return str(int(value)) if value == int(value) else str(value)


@dataclass(frozen=True)
class Site:
    """One place a number is written down, and the pattern that lifts it back out.

    `pattern` must capture the number as the group `value`, and must be anchored on enough
    surrounding text that it cannot drift onto a different number in the same file.
    """

    path: str
    pattern: str
    says: str

    def occurrences(self, read: Reader) -> tuple[list[tuple[int, float]], list[str]]:
        """`(occurrences, problems)`, where each occurrence is `(line number, value)`."""
        try:
            text = read(self.path)
        except OSError as exc:
            return [], [f"{self.path}: cannot be read ({exc})"]
        found: list[tuple[int, float]] = []
        problems: list[str] = []
        for match in re.finditer(self.pattern, text):
            # The line of the NUMBER, not of the match: a pattern may be anchored several lines
            # above the value it captures, and the reader needs the line to go and edit.
            line = text.count("\n", 0, match.start("value")) + 1
            raw = match.group("value")
            try:
                found.append((line, parse_number(raw)))
            except Unparseable as exc:
                problems.append(f"{self.path}:{line}: {exc}")
        if not found and not problems:
            problems.append(
                f"{self.path}: nothing matches the pattern for {self.says!r} any more — the text was"
                " reworded or moved, and this guard was about to stop checking it"
            )
        return found, problems


@dataclass(frozen=True)
class Pin:
    """One default: the code that defines it, and the prose that promises it."""

    slug: str
    what: str
    code: tuple[Site, ...]
    docs: tuple[Site, ...]


PINS: tuple[Pin, ...] = (
    Pin(
        "agent-ready-timeout",
        "herdr-agent `--ready-timeout`, in seconds",
        code=(
            Site(
                "rs/herdr-run/src/agent.rs",
                r"(?s)impl Default for DrainOptions \{.{0,200}?"
                r"ready_timeout: Duration::from_secs\((?P<value>[\d_]+)\)",
                "the Rust delivery default",
            ),
            Site(
                "rs/herdr-run/src/agent_cli.rs",
                r"(?m)^\s+ready_timeout: (?P<value>[\d_.]+),$",
                "the Rust CLI default",
            ),
            Site(
                "py/herdr_run/agent.py",
                r"(?m)^\s+ready_timeout: float = (?P<value>[\d_.]+),$",
                "the Python delivery default",
            ),
            Site(
                "py/herdr_run/agent_cli.py",
                r'"--ready-timeout", type=_ascii_float, default=(?P<value>[\d_.]+)\)',
                "the Python CLI default",
            ),
        ),
        docs=(
            Site(
                "common/docs/herdr-run/AGENT_USER_GUIDE.md",
                r"`--ready-timeout` \((?P<value>[\d,]+) seconds by default\)",
                "the readiness wait the agent guide promises",
            ),
        ),
    ),
    Pin(
        "command-timeout-seconds",
        "`timeout_seconds`: the wait for a command's exit code",
        code=(
            Site(
                "rs/herdr-run/src/config.rs",
                r"(?m)^\s+timeout_seconds: (?P<value>[\d_.]+),$",
                "the Rust config default",
            ),
            Site(
                "py/herdr_run/config.py",
                r"(?m)^\s+timeout_seconds: float = (?P<value>[\d_.]+)$",
                "the Python config default",
            ),
        ),
        docs=(
            Site(
                "common/docs/herdr-run/CONFIG_TEMPLATE.yaml",
                r"(?m)^timeout_seconds: (?P<value>[\d_.,]+)$",
                "the value `herdr-run init` writes",
            ),
        ),
    ),
    Pin(
        "pane-ready-timeout-seconds",
        "`ready_timeout_seconds`: the wait for a busy pane",
        code=(
            Site(
                "rs/herdr-run/src/config.rs",
                r"(?m)^\s+ready_timeout_seconds: (?P<value>[\d_.]+),$",
                "the Rust config default",
            ),
            Site(
                "py/herdr_run/config.py",
                r"(?m)^\s+ready_timeout_seconds: float = (?P<value>[\d_.]+)$",
                "the Python config default",
            ),
        ),
        docs=(
            Site(
                "common/docs/herdr-run/CONFIG_TEMPLATE.yaml",
                r"(?m)^ready_timeout_seconds: (?P<value>[\d_.,]+)$",
                "the value `herdr-run init` writes",
            ),
        ),
    ),
    Pin(
        "control-timeout-seconds",
        "the bound on one Herdr/systemd control subprocess, in seconds",
        code=(
            Site(
                "rs/herdr-run/src/client.rs",
                r"(?m)^pub\(crate\) const CONTROL_TIMEOUT: Duration = Duration::from_secs\((?P<value>[\d_]+)\);$",
                "the Rust bound",
            ),
            Site(
                "py/herdr_run/client.py",
                r"(?m)^CONTROL_TIMEOUT_SECONDS = (?P<value>[\d_.]+)$",
                "the Python bound",
            ),
        ),
        docs=(
            Site(
                "common/docs/herdr-run/USER_GUIDE.template.md",
                r"control subprocess is bounded to (?P<value>[A-Za-z\d_,]+) seconds",
                "the control bound the user guide states",
            ),
            Site(
                "common/docs/herdr-run/rendered/python/USER_GUIDE.md",
                r"control subprocess is bounded to (?P<value>[A-Za-z\d_,]+) seconds",
                "the control bound the packaged Python guide states",
            ),
            Site(
                "common/docs/herdr-run/rendered/rust/USER_GUIDE.md",
                r"control subprocess is bounded to (?P<value>[A-Za-z\d_,]+) seconds",
                "the control bound the packaged Rust guide states",
            ),
        ),
    ),
    Pin(
        "max-panes",
        "`max_panes`: the pane ceiling before a NEW tab is refused",
        code=(
            Site(
                "rs/herdr-run/src/config.rs",
                r"(?m)^pub const DEFAULT_MAX_PANES: u64 = (?P<value>[\d_]+);$",
                "the Rust constant",
            ),
            Site(
                "py/herdr_run/config.py",
                r"(?m)^DEFAULT_MAX_PANES = (?P<value>[\d_]+)$",
                "the Python constant",
            ),
        ),
        docs=(
            Site(
                "common/docs/herdr-run/CONFIG_TEMPLATE.yaml",
                r"(?m)^max_panes: (?P<value>[\d_,]+)$",
                "the value `herdr-run init` writes",
            ),
            Site(
                "common/docs/herdr-run/USER_GUIDE.template.md",
                r"`max_panes` \((?P<value>[\d_,]+) by default\)",
                "the cap the user guide states",
            ),
            Site(
                "common/docs/herdr-run/rendered/python/USER_GUIDE.md",
                r"`max_panes` \((?P<value>[\d_,]+) by default\)",
                "the cap the packaged Python guide states",
            ),
            Site(
                "common/docs/herdr-run/rendered/rust/USER_GUIDE.md",
                r"`max_panes` \((?P<value>[\d_,]+) by default\)",
                "the cap the packaged Rust guide states",
            ),
            # The rationale beside each constant argues from the number. It ships in the crate's
            # rustdoc and the wheel's docstrings, so it is documentation like any other.
            Site(
                "rs/herdr-run/src/config.rs",
                r"(?P<value>[\d_,]+) keeps an eightfold margin",
                "the margin the Rust rationale claims",
            ),
            Site(
                "py/herdr_run/config.py",
                r"(?P<value>[\d_,]+) keeps an eightfold margin",
                "the margin the Python rationale claims",
            ),
        ),
    ),
    Pin(
        "retention-days",
        "`retention_days`: how long captured run output is kept",
        code=(
            Site(
                "rs/herdr-run/src/retention.rs",
                r"(?m)^pub const RETENTION_DAYS: u64 = (?P<value>[\d_]+);$",
                "the Rust constant",
            ),
            Site(
                "py/herdr_run/retention.py",
                r"(?m)^RETENTION_DAYS = (?P<value>[\d_]+)$",
                "the Python constant",
            ),
        ),
        docs=(
            Site(
                "common/docs/herdr-run/CONFIG_TEMPLATE.yaml",
                r"(?m)^retention_days: (?P<value>[\d_,]+)$",
                "the value `herdr-run init` writes",
            ),
            # Twice in this document, and both are checked: a guide that is right in one paragraph
            # and stale in the next is exactly the failure this guards.
            Site(
                "common/docs/herdr-run/USER_GUIDE.template.md",
                r"`retention_days` \((?P<value>[A-Za-z\d_,]+) by default\)",
                "the retention window the user guide states",
            ),
            Site(
                "common/docs/herdr-run/rendered/python/USER_GUIDE.md",
                r"`retention_days` \((?P<value>[A-Za-z\d_,]+) by default\)",
                "the retention window the packaged Python guide states",
            ),
            Site(
                "common/docs/herdr-run/rendered/rust/USER_GUIDE.md",
                r"`retention_days` \((?P<value>[A-Za-z\d_,]+) by default\)",
                "the retention window the packaged Rust guide states",
            ),
            Site(
                "py/herdr_run/retention.py",
                r"(?P<value>[A-Za-z\d_,]+) days spans a long weekend",
                "the reason the Python constant gives for itself",
            ),
        ),
    ),
    Pin(
        "max-timeout-seconds",
        "the largest accepted timeout, in seconds",
        code=(
            Site(
                "rs/herdr-run/src/config.rs",
                r"(?m)^pub const MAX_TIMEOUT_SECONDS: f64 = (?P<value>[\d_.]+);$",
                "the Rust config bound",
            ),
            Site(
                "py/herdr_run/config.py",
                r"(?m)^MAX_TIMEOUT_SECONDS = (?P<value>[\d_.]+)$",
                "the Python config bound",
            ),
            Site(
                "rs/herdr-run/src/agent_cli.rs",
                r"(?m)^const MAX_WAIT_SECONDS: f64 = (?P<value>[\d_.]+);$",
                "the Rust herdr-agent bound",
            ),
            Site(
                "py/herdr_run/agent_cli.py",
                r"(?m)^_MAX_WAIT_SECONDS = (?P<value>[\d_.]+)$",
                "the Python herdr-agent bound",
            ),
        ),
        docs=(
            Site(
                "common/docs/herdr-run/AGENT_USER_GUIDE.md",
                r"finite seconds no\s+greater than (?P<value>[\d_,]+)",
                "the wait ceiling the agent guide states",
            ),
            Site(
                "common/docs/herdr-run/USER_GUIDE.template.md",
                r"no greater than (?P<value>[\d_,]+) seconds \(one year\)",
                "the timeout ceiling the user guide states",
            ),
            Site(
                "common/docs/herdr-run/rendered/python/USER_GUIDE.md",
                r"no greater than (?P<value>[\d_,]+) seconds \(one year\)",
                "the timeout ceiling the packaged Python guide states",
            ),
            Site(
                "common/docs/herdr-run/rendered/rust/USER_GUIDE.md",
                r"no greater than (?P<value>[\d_,]+) seconds \(one year\)",
                "the timeout ceiling the packaged Rust guide states",
            ),
        ),
    ),
    Pin(
        "max-count",
        "the largest accepted `--max-attempts` / `--lines`",
        code=(
            Site(
                "rs/herdr-run/src/agent_cli.rs",
                r"(?m)^const MAX_COUNT: u64 = (?P<value>[\d_]+);$",
                "the Rust bound",
            ),
            Site(
                "py/herdr_run/agent_cli.py",
                r"(?m)^_MAX_COUNT = (?P<value>[\d_]+)$",
                "the Python bound",
            ),
        ),
        docs=(
            Site(
                "common/docs/herdr-run/AGENT_USER_GUIDE.md",
                r"must be between 1 and (?P<value>[\d_,]+)",
                "the count ceiling the agent guide states",
            ),
        ),
    ),
    Pin(
        "max-retention-days",
        "the largest accepted `retention_days`",
        code=(
            Site(
                "rs/herdr-run/src/retention.rs",
                r"(?m)^pub const MAX_RETENTION_DAYS: u64 = (?P<value>[\d_]+);$",
                "the Rust bound",
            ),
            Site(
                "py/herdr_run/retention.py",
                r"(?m)^MAX_RETENTION_DAYS = (?P<value>[\d_]+)$",
                "the Python bound",
            ),
        ),
        docs=(
            Site(
                "common/docs/herdr-run/USER_GUIDE.template.md",
                r"retention beyond (?P<value>[\d_,]+) days",
                "the retention ceiling the user guide states",
            ),
            Site(
                "common/docs/herdr-run/rendered/python/USER_GUIDE.md",
                r"retention beyond (?P<value>[\d_,]+) days",
                "the retention ceiling the packaged Python guide states",
            ),
            Site(
                "common/docs/herdr-run/rendered/rust/USER_GUIDE.md",
                r"retention beyond (?P<value>[\d_,]+) days",
                "the retention ceiling the packaged Rust guide states",
            ),
        ),
    ),
)


def read_repo(path: str) -> str:
    return (REPO_ROOT / path).read_text(encoding="utf-8")


def check_pin(pin: Pin, read: Reader) -> list[str]:
    """Every way this default can be wrong, reported with file, line and both numbers."""
    failures: list[str] = []

    # 1. What the code says. Every definition site must agree before any prose is consulted.
    code_values: list[tuple[Site, int, float]] = []
    for site in pin.code:
        found, problems = site.occurrences(read)
        failures.extend(f"{pin.slug}: {problem}" for problem in problems)
        code_values.extend((site, line, value) for line, value in found)
    if not code_values:
        return failures

    agreed = code_values[0][2]
    for site, line, value in code_values[1:]:
        if value != agreed:
            first = code_values[0]
            failures.append(
                f"{pin.slug}: the implementations disagree about {pin.what} — "
                f"{first[0].path}:{first[1]} says {show(agreed)}, "
                f"{site.path}:{line} says {show(value)}"
            )

    # 2. What the documents say. Compared against the code, never derived from it.
    for site in pin.docs:
        found, problems = site.occurrences(read)
        failures.extend(f"{pin.slug}: {problem}" for problem in problems)
        for line, value in found:
            if value != agreed:
                source = code_values[0]
                failures.append(
                    f"{pin.slug}: {site.path}:{line} states {show(value)} for {pin.what}, "
                    f"but the code default is {show(agreed)} "
                    f"({source[0].path}:{source[1]}) — {site.says} is stale"
                )
    return failures


def check(read: Reader, pins: tuple[Pin, ...] = PINS) -> list[str]:
    failures: list[str] = []
    for pin in pins:
        failures.extend(check_pin(pin, read))
    return failures


def occurrence_count(read: Reader, pins: tuple[Pin, ...] = PINS) -> int:
    """How many documented occurrences were actually compared, for an honest success line."""
    total = 0
    for pin in pins:
        for site in pin.docs:
            found, _ = site.occurrences(read)
            total += len(found)
    return total


def _rewrite_value(replacement: str) -> Callable[[re.Match[str]], str]:
    """Substitution that swaps only the captured `value`, leaving its surroundings intact."""

    def rewrite(match: re.Match[str]) -> str:
        whole = match.group(0)
        start, end = match.span("value")
        return whole[: start - match.start()] + replacement + whole[end - match.start() :]

    return rewrite


def mutated_reader(read: Reader, edits: tuple[tuple[Site, str], ...]) -> Reader:
    """A reader that rewrites the captured number at each given site. Used only by `--self-test`."""
    overrides: dict[str, str] = {}
    for site, replacement in edits:
        text = overrides.get(site.path, read(site.path))
        overrides[site.path] = re.sub(site.pattern, _rewrite_value(replacement), text)

    def reader(path: str) -> str:
        return overrides[path] if path in overrides else read(path)

    return reader


def self_test() -> int:
    """Prove the guard is not tautological. No git, no network, no build.

    The controls that matter are the mutation ones: for every pin, this rewrites the code constant
    in memory and requires the real check to go red and NAME every document that promises it. A
    check that formatted the sentence from the constant would stay green here.
    """
    failures: list[str] = []

    def expect(condition: bool, why: str) -> None:
        if not condition:
            failures.append(why)

    expect(parse_number("1,000,000") == 1_000_000, "a comma-grouped numeral must parse")
    expect(parse_number("31_536_000.0") == 31_536_000, "an underscore-grouped float must parse")
    expect(parse_number("Four") == 4, "a spelled-out count must parse, case-insensitively")
    try:
        parse_number("several")
        failures.append("an unknown word must be a parse failure, not a silent skip")
    except Unparseable:
        pass

    expect(not check(read_repo), "the tree as committed must pass")

    for pin in PINS:
        # (a) The whole point. Change the code default in EVERY edition -- the realistic edit --
        #     and every document that states it must be reported, by path and by both numbers.
        code_value = code_value_of(pin, read_repo)
        if code_value is None:
            failures.append(f"{pin.slug}: no code site could be read, so nothing was proven")
            continue
        bumped = show(code_value + 1)
        read = mutated_reader(read_repo, tuple((site, bumped) for site in pin.code))
        reported = check_pin(pin, read)
        for site in pin.docs:
            named = [line for line in reported if line.startswith(f"{pin.slug}: {site.path}:")]
            expect(
                bool(named),
                f"{pin.slug}: changing the code default to {bumped} left {site.path} unreported —"
                " this guard is not comparing the document against the code",
            )
            expect(
                any(
                    f"states {show(code_value)} for" in line and f"code default is {bumped}" in line
                    for line in named
                ),
                f"{pin.slug}: the failure for {site.path} must name BOTH numbers — what the"
                f" document says and what the code now says — got {named}",
            )

        # (b) One edition changed alone is a divergence, reported before any prose.
        if len(pin.code) > 1:
            drifted = mutated_reader(read_repo, ((pin.code[1], bumped),))
            expect(
                any("disagree" in line for line in check_pin(pin, drifted)),
                f"{pin.slug}: one implementation changing alone must be reported as a divergence",
            )

        # (c) The prose changing alone is caught too, from the other direction.
        for site in pin.docs:
            edited = mutated_reader(read_repo, ((site, bumped),))
            expect(
                any(line.startswith(f"{pin.slug}: {site.path}:") for line in check_pin(pin, edited)),
                f"{pin.slug}: editing {site.path} to {bumped} must be caught",
            )

        # (d) A pattern that stops matching is loud. A silently vacuous guard is the worse bug:
        #     it reads as green forever while checking nothing.
        for site in (*pin.code, *pin.docs):
            def blanked(path: str, site: Site = site) -> str:
                text = read_repo(path)
                return re.sub(site.pattern, "<reworded>", text) if path == site.path else text

            expect(
                any("nothing matches the pattern" in line for line in check_pin(pin, blanked)),
                f"{pin.slug}: rewording {site.path} past the pattern must fail, not pass vacuously",
            )

    for failure in failures:
        print(f"FAIL  {failure}", file=sys.stderr)
    if failures:
        print(f"\n{len(failures)} self-test failure(s)", file=sys.stderr)
        return 1
    controls = 5 + sum(3 * len(pin.docs) + len(pin.code) + (1 if len(pin.code) > 1 else 0) for pin in PINS)
    print(f"check-documented-defaults --self-test: PASSED ({controls} controls, {len(PINS)} pins)")
    return 0


def code_value_of(pin: Pin, read: Reader) -> float | None:
    """The value the code currently defines for `pin`, or `None` if no site could be read."""
    for site in pin.code:
        found, _ = site.occurrences(read)
        if found:
            return found[0][1]
    return None


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--self-test", action="store_true", help="check the guard offline, then exit")
    args = parser.parse_args(argv)

    if args.self_test:
        return self_test()

    failures = check(read_repo)
    if failures:
        print("check-documented-defaults: FAIL", file=sys.stderr)
        for failure in failures:
            print(f"  {failure}", file=sys.stderr)
        print(
            "\n  A document that states a default is a promise about the code. Change both, or\n"
            "  stop stating the number. See `#88 herdr-run-pin-documented-defaults`.",
            file=sys.stderr,
        )
        return 1

    print(
        f"check-documented-defaults: ok — {occurrence_count(read_repo)} documented occurrences of "
        f"{len(PINS)} defaults agree with the code"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
