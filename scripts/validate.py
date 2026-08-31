#!/usr/bin/env python3
"""Run the checks this change actually needs, and say what was skipped and why.

The repository contract is expensive. A full run builds every Rust crate twice, executes the
Python and Rust suites, runs the cross-language differential over every paired tool, and then
packages and smoke-installs six distributions and four crates. That is the right price for a
change to `rs/` or `py/`. It is the wrong price for a change to `gent-talk/`, which is
deliberately outside the Rust workspace and shares no code with any of it: none of those checks
can observe the edit, so all they do is make the author wait.

So this maps changed paths onto the checks that could plausibly go red because of them, runs
those, and REPORTS THE REST AS SKIPPED, by name, with the reason. That last part is the whole
design. A selector that quietly does less is indistinguishable from a selector that is broken,
and this repository's rule is that an expected side effect which cannot happen gets reported
explicitly rather than dropped in silence.

Two safety properties, both tested in `--self-test`:

* **An unrecognised path selects EVERYTHING.** Adding a new top-level directory must not silently
  opt out of validation. Unknown means unknown, and unknown means run the lot.
* **`--all` is always available**, and is what CI and any release path should use. Selection is a
  convenience for the edit-run loop, not a new definition of "validated".

Usage:
    python3 scripts/validate.py                 # against origin/main
    python3 scripts/validate.py --base HEAD~1
    python3 scripts/validate.py --all           # the entire contract, no selection
    python3 scripts/validate.py --list          # print the plan, run nothing
    python3 scripts/validate.py --self-test     # check the mapping itself, offline
"""

from __future__ import annotations

import argparse
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent


@dataclass(frozen=True)
class Check:
    """One command, and where to run it from."""

    name: str
    argv: tuple[str, ...]
    cwd: str = "."


DOCS = "docs"
WORKSPACE = "workspace"
CROSS = "cross"
PACKAGES = "packages"
GENT_TALK = "gent-talk"
TIMELINE_BROWSER = "timeline-browser"
HYGIENE = "hygiene"

#: Every group, in the order a full run should execute them: cheapest and most likely to fail
#: first, so a broken build does not surface only after twenty minutes of packaging.
GROUPS: dict[str, tuple[Check, ...]] = {
    # Repo-wide and cheap, so it leads. A client name can be typed into ANY file, including the
    # prose directories below that map to no other check at all.
    #
    # The documented-defaults guard is here rather than under DOCS or WORKSPACE because it spans
    # both: it compares a number in `common/docs/` against a constant in `rs/` and `py/`, and
    # EITHER side changing alone is the drift it exists to catch. Putting it in one group would
    # leave the other edit unchecked, which is the shape of the bug it guards.
    HYGIENE: (
        Check("client-names-self-test", ("python3", "scripts/check_client_names.py", "--self-test")),
        Check("client-names", ("python3", "scripts/check_client_names.py")),
        Check(
            "documented-defaults-self-test",
            ("python3", "scripts/check_documented_defaults.py", "--self-test"),
        ),
        Check("documented-defaults", ("python3", "scripts/check_documented_defaults.py")),
    ),
    DOCS: (Check("embed-userguides", ("python3", "scripts/embed_userguides.py", "--check")),),
    WORKSPACE: (
        Check("rust-fmt", ("cargo", "fmt", "--all", "--manifest-path", "rs/Cargo.toml", "--", "--check")),
        Check("build", ("make", "both")),
        Check("lint-and-typecheck", ("make", "check")),
        Check("test", ("make", "test")),
    ),
    CROSS: (
        Check("mypy-differential", ("python3", "-m", "mypy", "cross/differential.py")),
        Check("cross-differential", ("make", "cross")),
    ),
    PACKAGES: (Check("packages", ("make", "check-packages")),),
    TIMELINE_BROWSER: (
        # Delegated to the tool's OWN Makefile rather than restated here, so there is one
        # definition of what boxing this tool means and `make validate` in its source directory
        # runs exactly what CI runs. Restating the `dagrun box` invocation in both places is how
        # the two drift until the local gate and the CI gate disagree about what passed.
        #
        # These ran nowhere before. The suite is 35 tests including pan and zoom, it lived in the
        # tree fully written, and nothing invoked it -- so a genuine failure sat unnoticed in it.
        Check("timeline-browser", ("make", "-C", "py/wrkviz", "browser")),
        # Pass/fail AND a measurement: it prints zoom/pan redraw latency every run and fails when
        # a redraw breaches the budget. A functional suite cannot see a change that makes
        # interaction ten times slower; this can.
        Check("timeline-benchmark", ("make", "-C", "py/wrkviz", "benchmark")),
    ),
    GENT_TALK: (
        Check("gent-talk-fmt", ("cargo", "fmt", "--check"), cwd="gent-talk"),
        Check("gent-talk-clippy", ("cargo", "clippy", "--all-targets", "--", "-D", "warnings"), cwd="gent-talk"),
        Check("gent-talk-test", ("cargo", "test"), cwd="gent-talk"),
        # EVERY page suite, not a named file. gent-talk serves two pages -- `/` and `/voice` --
        # and naming one of them here is how the second suite comes to exist without ever running.
        # The pattern is expanded by node, not by a shell, so there is no glob for a shell to eat.
        # `cargo test` runs both through their own harnesses too; this keeps the fast loop honest.
        Check("gent-talk-page", ("node", "--test", "tests/js/*.test.mjs"), cwd="gent-talk"),
        # The boxed graph, CHECKED but not run -- offline and instant. Running it here would be
        # running the same suites a second time.
        #
        # It used to catch only a file that had stopped being loadable, and said so. Since `#96
        # dagrun-loader-validation` the loader refuses a closed-schema violation, a duplicate step
        # tag, a missing dependency, a named cycle and a demand above its cap, so every inspection
        # subcommand carries them and a green tick here really does mean the graph is well formed.
        # `common/bin/dagrun`, which is TRACKED. `./bin/` is gitignored and only exists after
        # `./setup` has run, so shelling out to it made this check fail on a fresh clone --
        # for a reason that has nothing to do with whatever the author changed.
        Check("gent-talk-dag", ("common/bin/dagrun", "list", "--dag", "gent-talk/tests.dag.yaml")),
    ),
}

ALL_GROUPS: frozenset[str] = frozenset(GROUPS)

#: Groups that run for EVERY change, whatever the paths. A group belongs here only if a change
#: anywhere could break it -- which for a repo-wide text scan is the literal truth.
ALWAYS: frozenset[str] = frozenset({HYGIENE})

#: Why a group runs, phrased so the reason survives being read months later.
WHY: dict[str, str] = {
    DOCS: "a document that is embedded into packaged user guides changed",
    WORKSPACE: "workspace Rust or Python source changed",
    CROSS: "code with a paired cross-language implementation changed",
    PACKAGES: "something that ships inside a distribution changed",
    GENT_TALK: "gent-talk changed (it is outside the Rust workspace and has its own suite)",
    TIMELINE_BROWSER: (
        "the timeline's browser-facing assets or its browser suite changed; these drive a real"
        " Chromium and are the only checks that can see a rendering or interaction regression"
    ),
    HYGIENE: (
        "always: a consuming project's name can be typed into any file, and a documented default"
        " can be broken from either the prose or the code side"
    ),
}

#: Longest prefix wins, so `scripts/embed_userguides.py` beats the bare `scripts/` catch-all.
PREFIX_RULES: tuple[tuple[str, frozenset[str]], ...] = (
    # gent-talk is outside the Rust workspace, but its PYTHON is not outside the repository's
    # type check: `make check` runs mypy over `.`, which walks `gent-talk/scripts/`. Selecting
    # only the gent-talk group for those files let a real type error reach main — the scan for
    # leaked credentials in smoke-agent.py called a method that does not exist, and nothing that
    # ran locally could see it. Longest-prefix wins, so this beats the bare `gent-talk/` rule.
    ("gent-talk/scripts/", frozenset({GENT_TALK, WORKSPACE})),
    ("gent-talk/", frozenset({GENT_TALK})),
    ("rs/", frozenset({WORKSPACE, CROSS, PACKAGES})),
    # Longest prefix wins, so these beat the bare `py/` rule below. They add the browser group
    # WITHOUT removing the others: the static bundle is packaged and type-checked like the rest
    # of the package, so a change to it still owes the workspace and packaging checks. A rule
    # that selected only the browser group here would be the cheaper-of-the-two answer the
    # self-test exists to forbid.
    ("py/wrkviz/static/", frozenset({TIMELINE_BROWSER, WORKSPACE, CROSS, PACKAGES})),
    ("py/tests/js/", frozenset({TIMELINE_BROWSER, WORKSPACE, CROSS, PACKAGES})),
    ("py/", frozenset({WORKSPACE, CROSS, PACKAGES})),
    ("cross/", frozenset({CROSS})),
    ("common/docs/", frozenset({DOCS, PACKAGES})),
    ("examples/", frozenset({CROSS})),
    # The log fetcher and its shell wrapper. Type-checked by `make check` (mypy walks the repo)
    # and covered by `py/tests/test_agent_log_archive_fetcher.py`, so it owes the workspace
    # group -- and nothing else: it ships in no distribution and has no cross-language twin.
    # Before this rule it read as unclassified and selected EVERY group, which is the safe
    # direction to be wrong in but meant a one-line fetcher change ran gent-talk's Rust suite.
    ("scripts/agent-log-archive/", frozenset({WORKSPACE})),
    ("scripts/embed_userguides.py", frozenset({DOCS, PACKAGES})),
    ("scripts/check_python_packages.py", frozenset({PACKAGES})),
    ("scripts/check_rust_packages.py", frozenset({PACKAGES})),
    ("scripts/validate.py", frozenset()),
    # Their own group is always-on, so naming them here only stops them reading as unclassified.
    ("scripts/check_client_names.py", frozenset()),
    ("scripts/check_documented_defaults.py", frozenset()),
    # Prose and agent-facing material. Not embedded anywhere, so nothing can go red for it.
    ("ai_docs/", frozenset()),
    ("reviews/", frozenset()),
    ("skills/", frozenset()),
    ("bin/", frozenset()),
    ("AGENTS.md", frozenset()),
    ("CLAUDE.md", frozenset()),
    ("LICENSE", frozenset()),
    (".gitignore", frozenset()),
    # CI config is validated by CI actually running, which no local command can substitute for.
    (".github/", frozenset()),
)


def groups_for(paths: list[str]) -> tuple[frozenset[str], dict[str, str]]:
    """Map changed paths onto the groups they can affect.

    Returns the groups, and — for the report — the first path that pulled each one in. An
    unrecognised path selects every group, and names itself as the reason.
    """
    selected: set[str] = set(ALWAYS)
    because: dict[str, str] = {group: "always" for group in ALWAYS}
    for path in sorted(paths):
        best: frozenset[str] | None = None
        best_len = -1
        for prefix, groups in PREFIX_RULES:
            if path == prefix or path.startswith(prefix):
                if len(prefix) > best_len:
                    best, best_len = groups, len(prefix)
        if best is None:
            # Fail safe. A path nobody has classified is a path nobody has reasoned about.
            for group in sorted(ALL_GROUPS):
                because.setdefault(group, f"{path} (unclassified path — running everything)")
            selected |= set(ALL_GROUPS)
            continue
        for group in sorted(best):
            because.setdefault(group, path)
        selected |= set(best)
    return frozenset(selected), because


def changed_paths(base: str) -> list[str]:
    """Paths differing from `base`, including uncommitted and untracked work."""
    out: set[str] = set()
    merge_base = subprocess.run(
        ("git", "merge-base", base, "HEAD"), cwd=REPO_ROOT, capture_output=True, text=True, check=False
    )
    ref = merge_base.stdout.strip() if merge_base.returncode == 0 else base
    for argv in (
        ("git", "diff", "--name-only", f"{ref}...HEAD"),
        ("git", "diff", "--name-only", "HEAD"),
        ("git", "ls-files", "--others", "--exclude-standard"),
    ):
        done = subprocess.run(argv, cwd=REPO_ROOT, capture_output=True, text=True, check=False)
        if done.returncode != 0:
            raise SystemExit(f"validate: `{' '.join(argv)}` failed:\n{done.stderr.strip()}")
        out.update(line for line in done.stdout.splitlines() if line)
    return sorted(out)


def run(checks: tuple[Check, ...]) -> int:
    for check in checks:
        print(f"\n=== {check.name} ===", flush=True)
        done = subprocess.run(check.argv, cwd=REPO_ROOT / check.cwd, check=False)
        if done.returncode != 0:
            print(f"\nvalidate: FAILED at {check.name} ({' '.join(check.argv)})", file=sys.stderr)
            return done.returncode
    return 0


def report(selected: frozenset[str], because: dict[str, str], paths: list[str]) -> None:
    print(f"validate: {len(paths)} changed path(s)")
    for group in sorted(ALL_GROUPS):
        if group in selected:
            print(f"  RUN   {group:<11} — {WHY[group]} [{because.get(group, '?')}]")
        else:
            print(f"  skip  {group:<11} — nothing changed that it can observe")
    if selected <= ALWAYS:
        print("  (nothing selected by path: every changed path is prose or configuration)")


def self_test() -> int:
    """Check the mapping itself. No git, no network, no build."""
    failures: list[str] = []

    def expect(paths: list[str], want: set[str], why: str) -> None:
        """`want` is what the PATHS select. Always-on groups are added here and checked below."""
        got, _ = groups_for(paths)
        expected = want | ALWAYS
        if set(got) != expected:
            failures.append(
                f"{why}\n    paths={paths}\n    want={sorted(expected)}\n    got ={sorted(got)}"
            )

    expect(["gent-talk/src/ops.rs"], {GENT_TALK}, "a gent-talk change must not drag in the workspace contract")
    expect(["gent-talk/web/voice.js", "gent-talk/README.md"], {GENT_TALK}, "gent-talk docs are still gent-talk")
    expect(
        ["gent-talk/scripts/smoke-agent.py"],
        {GENT_TALK, WORKSPACE},
        "gent-talk PYTHON is inside the repository mypy run, so it must select the workspace too",
    )
    expect(["rs/tick-hub/src/lib.rs"], {WORKSPACE, CROSS, PACKAGES}, "a workspace crate needs the full chain")
    expect(["py/dagrun/sizing.py"], {WORKSPACE, CROSS, PACKAGES}, "python source needs the full chain")
    expect(["cross/differential.py"], {CROSS}, "the differential harness needs only itself")
    expect(["common/docs/herdr-run/README.template.md"], {DOCS, PACKAGES}, "embedded docs ship inside packages")
    expect(["AGENTS.md"], set(), "the agent guide is embedded nowhere")
    expect(["ai_docs/note.md", "reviews/x.md"], set(), "prose selects nothing")
    expect(["scripts/embed_userguides.py"], {DOCS, PACKAGES}, "longest prefix must beat the scripts catch-all")
    expect(["scripts/something_new.py"], set(ALL_GROUPS), "an unclassified script must select EVERYTHING")
    expect(["brand_new_toplevel/x"], set(ALL_GROUPS), "an unclassified top level must select EVERYTHING")
    expect(
        ["gent-talk/src/ops.rs", "rs/tick-hub/src/lib.rs"],
        {GENT_TALK, WORKSPACE, CROSS, PACKAGES},
        "a change spanning two areas is the UNION, never the smaller of the two",
    )
    expect(["scripts/check_client_names.py"], set(), "the hygiene guard needs only its own always-on group")
    expect(
        ["scripts/check_documented_defaults.py"],
        set(),
        "the documented-defaults guard needs only its own always-on group",
    )
    expect([], set(), "no changes selects nothing by path")

    # ALWAYS is the property the table above cannot state, because it folds it in silently.
    for paths, why in (
        ([], "no changes at all"),
        (["ai_docs/note.md"], "a prose path that selects nothing else"),
        (["AGENTS.md"], "a file mapped to the empty set"),
        (["rs/tick-hub/src/lib.rs"], "an ordinary source change"),
    ):
        got, because = groups_for(paths)
        if not ALWAYS <= set(got):
            failures.append(f"an always-on group must run for {why}: got {sorted(got)}")
        for group in ALWAYS:
            if group not in because:
                failures.append(f"an always-on group must state a reason for {why}")

    # Every group named by a rule must exist, or the selector silently runs nothing for it.
    for prefix, groups in PREFIX_RULES:
        for group in groups:
            if group not in GROUPS:
                failures.append(f"rule {prefix!r} names unknown group {group!r}")
    for group in GROUPS:
        if group not in WHY:
            failures.append(f"group {group!r} has no stated reason in WHY")

    for failure in failures:
        print(f"FAIL  {failure}", file=sys.stderr)
    if failures:
        print(f"\n{len(failures)} self-test failure(s)", file=sys.stderr)
        return 1
    print("validate --self-test: PASSED (16 mapping controls, 8 always-on controls)")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--base", default="origin/main", help="compare against this ref (default: origin/main)")
    parser.add_argument("--all", action="store_true", help="run the entire contract, selecting nothing")
    parser.add_argument("--list", action="store_true", help="print the plan and exit without running")
    parser.add_argument("--self-test", action="store_true", help="check the mapping offline, then exit")
    args = parser.parse_args(argv)

    if args.self_test:
        return self_test()

    if args.all:
        selected, because, paths = ALL_GROUPS, {g: "--all" for g in ALL_GROUPS}, []
        print("validate: --all, running the entire contract")
    else:
        paths = changed_paths(args.base)
        selected, because = groups_for(paths)
        report(selected, because, paths)

    if args.list:
        return 0

    for group in GROUPS:  # dict order is the intended execution order
        if group in selected:
            code = run(GROUPS[group])
            if code != 0:
                return code
    print("\nvalidate: OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
