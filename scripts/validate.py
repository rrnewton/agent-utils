#!/usr/bin/env python3
"""Run the checks this change actually needs, and say what was skipped and why.

The portable, current-toolchain repository contract is expensive. A full run builds and tests all
workspace code, runs the cross-language differential over every paired tool, and then packages and
smoke-installs eight distributions and seven crates. That is the right price for a broad or
unclassified change. It is the wrong price for a change to an independent tool or to
repository-only prose: checks that cannot observe the edit only make the author wait.

So this maps changed paths onto the checks that could plausibly go red because of them and reports
the plan by name. Broad groups say RUN or skip; when component labels select only part of a broad
group, the report says narrow rather than falsely claiming that group cannot observe the edit.
A selector that quietly does less is indistinguishable from one that is broken, so expected work
is always reported explicitly rather than dropped in silence.

Two safety properties, both tested in `--self-test`:

* **An unrecognised path selects EVERYTHING.** Adding a new top-level directory must not silently
  opt out of validation. Unknown means unknown, and unknown means run the lot.
* **`--all` is always available**. It runs the entire main-phase contract; since #1190 its
  gate-exclusive cross steps are still path-filtered (fail-closed, named in the plan), because
  at gate prices half of all heads were paying ~22 minutes for differentials their paths
  cannot affect (#1180 Part B). **`--all --cross-all`** is the unfiltered contract the nightly
  and release paths use (`make validate-all`). Selection is a convenience for the edit-run
  loop, not a new definition of "validated".
* **The five gate-exclusive cross differentials are disabled by default** (owner decision,
  2026-11-10, reply 346; #1249 cross-differentials-robust tracks bringing them back without
  wall-clock budgets): no default, `--all`, or `--cross-all` run selects them, so the
  gate-exclusive phase is empty. `--cross-differentials` is the explicit opt-in that runs
  them on demand, in the exclusive phase unless `AGENT_UTILS_CROSS_IN_MAIN_PHASE=1`.
* **Real-time-sensitive steps labelled `disabled-optin` are disabled by default** (same
  owner decision; #1250 vibe-talk-timing-robust and #1252 timeline-benchmark-optin track
  bringing them back as behaviour-only checks): no default or `--all` run selects them, and
  every plan names them as skipped. `--include-disabled` (or
  `AGENT_UTILS_INCLUDE_DISABLED=1`) is the explicit opt-in that runs them on demand.

Usage:
    python3 scripts/validate.py                 # against origin/main
    python3 scripts/validate.py --base HEAD~1
    python3 scripts/validate.py --all           # the entire portable contract (cross steps path-filtered, #1190)
    python3 scripts/validate.py --all --cross-all  # the entire contract, every cross step forced except the disabled differentials (#1249)
    python3 scripts/validate.py --cross-differentials --all  # on demand only: also run the five disabled cross differentials (#1249)
    python3 scripts/validate.py --include-disabled --all  # on demand only: also run steps disabled until robust (#1250, #1252)
    python3 scripts/validate.py --keep-going    # report every independent failure, not just the first
    python3 scripts/validate.py --list          # print the plan, run nothing
    python3 scripts/validate.py --self-test     # check the mapping itself, offline

HOST SETTINGS. A host may need to tell the graph which npm binary to run, a package registry
mirror, or a Node release mirror -- facts about the machine that the repository must not name.
They go in ${XDG_CONFIG_HOME:-~/.config}/agent-utils/validate.env, one KEY=VALUE per line ('#'
comments and blank lines allowed, VALUE taken literally), and only these keys are accepted:
AGENT_UTILS_NPM, AGENT_UTILS_NODE_DIST_URL, npm_config_registry. Any other key, a malformed or
repeated line, or an empty value stops validation before it starts. The values reach the
validation graph's environment and nothing else; a variable already set in the caller's
environment wins. Each run prints the file and the keys it set, never their values.
"""

from __future__ import annotations

import argparse
import contextlib
import json
import os
import shlex
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time
from collections.abc import Iterator, Mapping, Sequence
from pathlib import Path
from types import FrameType
from typing import NamedTuple

import case_timings
from run_component_tests import Manifest, components_for_paths, load_manifest

REPO_ROOT = Path(__file__).resolve().parent.parent

#: The variable pytest reads for the root of its numbered temp directories (#941
#: gate-pytest-basetemp-leak). Without it every pytest process numbers its own basetemp under
#: the shared ``/tmp/pytest-of-USER``, where its keep-three retention never catches up.
PYTEST_TEMPROOT_ENV = "PYTEST_DEBUG_TEMPROOT"
#: Prefix of one validation run's private pytest temp root, in the system temp directory.
PYTEST_ROOT_PREFIX = "validate-pytest-"
#: How long a root a killed run left behind must stand untouched before a later run removes it.
STALE_PYTEST_ROOT_SECONDS = 24 * 3600


DOCS = "docs"
# Full-impact repository wiring (Makefile, setup, validation graph, workflows) activates every
# group, including this umbrella for component-only nodes. It deliberately has no narrow emitter.
WORKSPACE = "workspace"
PYTHON = "python"
PYTHON_COMPONENT_TESTS = "python-component-tests"
PYTEST_SHARDS = "pytest-shards"
PYTHON_NAMESPACE_TESTS = "python-namespace-tests"
RUST = "rust"
RUST_WORKSPACE_TESTS = "rust-workspace-tests"
CROSS = "cross"
PACKAGES = "packages"
PYTHON_PACKAGES = "python-packages"
RUST_PACKAGES = "rust-packages"
VIBE_TALK = "vibe-talk"
VIBE_TALK_PAGE = "vibe-talk-page"
TIMELINE_BROWSER = "timeline-browser"
HYGIENE = "hygiene"

#: Logical selection groups map one-to-one onto labels in the flattened validation DAG. A full
#: run omits label selection and therefore executes every node exactly once.
GROUPS: tuple[str, ...] = (
    HYGIENE,
    DOCS,
    PYTHON,
    PYTHON_COMPONENT_TESTS,
    PYTEST_SHARDS,
    PYTHON_NAMESPACE_TESTS,
    RUST,
    RUST_WORKSPACE_TESTS,
    WORKSPACE,
    CROSS,
    PACKAGES,
    PYTHON_PACKAGES,
    RUST_PACKAGES,
    TIMELINE_BROWSER,
    VIBE_TALK,
    VIBE_TALK_PAGE,
)

GROUP_LABELS: dict[str, str] = {
    HYGIENE: "validation-hygiene",
    DOCS: "validation-docs",
    PYTHON: "validation-python",
    PYTHON_COMPONENT_TESTS: "validation-python-component-tests",
    PYTEST_SHARDS: "validation-pytest-shards",
    PYTHON_NAMESPACE_TESTS: "validation-python-namespace-tests",
    RUST: "validation-rust",
    RUST_WORKSPACE_TESTS: "validation-rust-workspace-tests",
    WORKSPACE: "validation-workspace",
    CROSS: "validation-cross",
    PACKAGES: "validation-packages",
    PYTHON_PACKAGES: "validation-python-packages",
    RUST_PACKAGES: "validation-rust-packages",
    TIMELINE_BROWSER: "validation-timeline",
    VIBE_TALK: "validation-vibe-talk",
    VIBE_TALK_PAGE: "validation-vibe-talk-page",
}

ALL_GROUPS: frozenset[str] = frozenset(GROUPS)

#: Groups that run for EVERY change, whatever the paths. A group belongs here only if a change
#: anywhere could break it -- which for a repo-wide text scan is the literal truth.
ALWAYS: frozenset[str] = frozenset({HYGIENE})

#: Why a group runs, phrased so the reason survives being read months later.
WHY: dict[str, str] = {
    DOCS: "a document that is embedded into packaged user guides changed",
    PYTHON: "Python source or its validation configuration changed",
    PYTHON_COMPONENT_TESTS: "shared Python component-test infrastructure changed",
    PYTEST_SHARDS: "the pytest sharding implementation changed",
    PYTHON_NAMESPACE_TESTS: "the mapped-namespace test launcher changed",
    RUST: "Rust source or its validation configuration changed",
    RUST_WORKSPACE_TESTS: "shared Rust workspace manifests or locks changed",
    WORKSPACE: "shared workspace configuration changed",
    CROSS: "code with a paired cross-language implementation changed",
    PACKAGES: "something that ships inside a distribution changed",
    PYTHON_PACKAGES: "Python packaging inputs or checks changed",
    RUST_PACKAGES: "Rust packaging inputs or checks changed",
    VIBE_TALK: "vibe-talk changed (it is outside the Rust workspace and has its own suite)",
    VIBE_TALK_PAGE: "page assets changed: all browser, page, embedded-page Rust, and build checks",
    TIMELINE_BROWSER: (
        "the timeline's browser-facing assets or its browser suite changed; these drive a real"
        " Chromium and are the only checks that can see a rendering or interaction regression"
    ),
    HYGIENE: (
        "always: a consuming project's name can be typed into any file, a documented default"
        " can be broken from either the prose or the code side, and renaming any file or"
        " rewording any heading can break a website link"
    ),
}

#: Longest prefix wins, so `scripts/embed_userguides.py` beats the bare `scripts/` catch-all.
PREFIX_RULES: tuple[tuple[str, frozenset[str]], ...] = (
    # Page inputs cannot change Rust formatting/lints or the Python operator tools.
    # Keep all Rust tests: several read page source or serve its embedded assets.
    ("vibe-talk/web/", frozenset({VIBE_TALK_PAGE})),
    # The tool's own gate includes strict typing for its Python operator scripts. Keeping that
    # check beside the scripts means a one-line helper fix no longer drags in the unrelated Rust
    # and Python workspaces merely to reach the repository-wide mypy invocation.
    ("vibe-talk/scripts/", frozenset({VIBE_TALK})),
    ("vibe-talk/", frozenset({VIBE_TALK})),
    # Shared workspace manifests and test configuration can affect every component. Tool-owned
    # source below these paths is resolved through validation/components.json instead.
    # Workspace dependency, feature, and profile changes can alter both implementations' runtime
    # behavior even when no paired source file changed, so they owe the parity contract too.
    (
        "rs/Cargo.toml",
        frozenset({RUST, RUST_WORKSPACE_TESTS, CROSS, RUST_PACKAGES}),
    ),
    (
        "rs/Cargo.lock",
        frozenset({RUST, RUST_WORKSPACE_TESTS, CROSS, RUST_PACKAGES}),
    ),
    # Every public Rust launcher dispatches through this one script. A change can affect every
    # tool, package smoke, and cross-runtime invocation, so treating it as one component would be
    # false precision; run the complete portable contract explicitly.
    ("rs/bin/wrkslotsd", frozenset({RUST, RUST_WORKSPACE_TESTS, RUST_PACKAGES})),
    ("rs/bin/wrkvizd", frozenset({RUST, RUST_WORKSPACE_TESTS, RUST_PACKAGES})),
    ("rs/bin/cargo-runner", frozenset(GROUPS)),
    ("rustfmt.toml", frozenset({RUST, VIBE_TALK})),
    (
        "py/pyproject.toml",
        frozenset({PYTHON, PYTHON_COMPONENT_TESTS, PYTEST_SHARDS, PYTHON_PACKAGES}),
    ),
    # These are imported by tests across component and lifecycle lanes. No single tool owns their
    # effects, so use the conservative all-workspace test boundary.
    ("py/tests/__init__.py", frozenset({PYTHON, PYTHON_COMPONENT_TESTS})),
    ("py/tests/conftest.py", frozenset({PYTHON, PYTHON_COMPONENT_TESTS})),
    # Browser assets are also owned by wrkviz through the component manifest; this group adds the
    # real browser checks to that component's unit/package/cross closure.
    ("py/wrkviz/static/", frozenset({TIMELINE_BROWSER, PYTHON})),
    ("py/tests/js/", frozenset({TIMELINE_BROWSER, PYTHON})),
    # The language indexes are package-check inputs: each checker verifies that its public index
    # names exactly the distributions it ships. These exact rules must not bleed into a future
    # `py/README.md.generated`, which `_rule_matches` prevents.
    ("py/README.md", frozenset({PYTHON_PACKAGES})),
    ("rs/README.md", frozenset({RUST_PACKAGES})),
    # Rust-only components have crate-owned test labels, including embedded guide edits.
    ("rs/gh-paced/", frozenset({RUST})),
    ("common/docs/gh-paced/", frozenset({DOCS, RUST})),
    ("rs/agent-usage/", frozenset({RUST})),
    ("common/docs/agent-usage/", frozenset({DOCS, RUST})),
    ("rs/agent-teams/", frozenset({RUST})),
    ("common/docs/agent-teams/", frozenset({DOCS, RUST})),
    ("rs/merge-queue/", frozenset({RUST})),
    ("rs/change-classifier/", frozenset({RUST})),
    ("common/docs/change-classifier/", frozenset({DOCS, RUST})),
    ("common/docs/merge-queue/", frozenset({DOCS, RUST})),
    ("cross/", frozenset({CROSS})),
    ("common/docs/", frozenset({DOCS})),
    ("common/README.md", frozenset()),
    ("skills/agentctl/", frozenset({DOCS})),
    ("examples/", frozenset({CROSS})),
    # The log fetcher and its shell wrapper. Type-checked by `make check` (mypy walks the repo)
    # and covered by `py/tests/test_agent_log_archive_fetcher.py`, so it owes the workspace
    # group -- and nothing else: it ships in no distribution and has no cross-language twin.
    # Before this rule it read as unclassified and selected EVERY group, which is the safe
    # direction to be wrong in but meant a one-line fetcher change ran vibe-talk's Rust suite.
    ("scripts/agent-log-archive/", frozenset({PYTHON})),
    ("scripts/embed_userguides.py", frozenset({DOCS, PACKAGES})),
    ("scripts/check_python_packages.py", frozenset({PYTHON_PACKAGES})),
    ("scripts/check_rust_packages.py", frozenset({RUST_PACKAGES})),
    # Its subsets install Python distributions and run beside the package smoke checks.
    ("scripts/check_standalone.py", frozenset({PYTHON_PACKAGES})),
    ("scripts/with-node22", frozenset({TIMELINE_BROWSER})),
    ("scripts/provision-node-packages", frozenset({TIMELINE_BROWSER, VIBE_TALK})),
    ("scripts/provision-node22", frozenset({TIMELINE_BROWSER})),
    # These define the planner or executable graph itself; only the complete portable contract can prove a
    # change to them. The exact rules beat the repository-infrastructure component mapping.
    ("scripts/validate.py", frozenset(GROUPS)),
    ("scripts/gate.py", frozenset({PYTHON_COMPONENT_TESTS})),
    ("scripts/run_component_tests.py", frozenset({PYTHON_COMPONENT_TESTS})),
    ("scripts/run_pytest_shard.py", frozenset({PYTEST_SHARDS})),
    ("scripts/pid_namespace_init.py", frozenset({PYTHON_NAMESPACE_TESTS})),
    ("validation.dag.yaml", frozenset(GROUPS)),
    ("validation/", frozenset(GROUPS)),
    # Their own group is always-on, so naming them here only stops them reading as unclassified.
    ("scripts/check_editions.py", frozenset()),
    ("scripts/check_agent_queue_schemas.py", frozenset()),
    ("scripts/check_wrkslots_schemas.py", frozenset()),
    ("scripts/check_wrkviz_schemas.py", frozenset()),
    ("scripts/check_rust_only.py", frozenset()),
    ("scripts/check_client_names.py", frozenset()),
    ("scripts/check_documented_defaults.py", frozenset()),
    ("scripts/check_issue_numbers.py", frozenset()),
    ("scripts/check_issue_parse.py", frozenset()),
    ("scripts/check_site.py", frozenset()),
    # The public website is static HTML and CSS that nothing builds or embeds. Its checker runs in
    # the always-on hygiene group because the site links into the whole tree: a renamed document
    # or a reworded heading ANYWHERE can break it, so a site-only rule here could not be complete.
    # site/demos/ holds the Python that records the site's media clips; a Python file there owes
    # repository-wide strict typing, which the root-mypy rule in selection_for adds.
    ("site/", frozenset()),
    ("scripts/check_no_any.py", frozenset({PYTHON, CROSS, VIBE_TALK})),
    # Submodules: each project under ext/ has its own repository, CI and releases, and nothing in
    # this repository builds, embeds or tests it, so a pin bump owes only the always-on hygiene
    # group. Exact rules, not "ext/": a new submodule must be classified on purpose.
    (".gitmodules", frozenset()),
    ("ext/minibeads", frozenset()),
    # Prose and agent-facing material. Not embedded anywhere, so nothing can go red for it.
    (".minibeads/", frozenset()),
    ("ai_docs/", frozenset()),
    ("reviews/", frozenset()),
    ("skills/", frozenset()),
    ("bin/", frozenset()),
    ("common/bin/", frozenset(GROUPS)),
    ("Makefile", frozenset(GROUPS)),
    ("setup", frozenset(GROUPS)),
    ("README.md", frozenset()),
    ("QUICKSTART.md", frozenset()),
    ("RELATED_WORK.md", frozenset()),
    ("scripts/README.md", frozenset()),
    ("cross/README.md", frozenset()),
    ("AGENTS.md", frozenset()),
    ("CLAUDE.md", frozenset()),
    ("LICENSE", frozenset({PACKAGES})),
    (".gitignore", frozenset()),
    # Workflow edits can change conditional compatibility jobs or scheduled behavior that a
    # path-filtered self-run would otherwise skip. Exercise the complete portable graph and make
    # every plan output true so the edited compatibility lanes run as well.
    (".github/", frozenset(GROUPS)),
    # The one exception: the Pages workflow has no compatibility lane and no validation of its own.
    # It checks and stages site/ (minus demos/ and README.md) with scripts/check_site.py and
    # uploads the staged copy, and the hygiene group already runs that checker on every change.
    # The component manifest still selects the infrastructure tests.
    (".github/workflows/pages.yml", frozenset()),
)


def _rule_matches(path: str, prefix: str) -> bool:
    """Match a directory prefix or an exact file rule without prefix bleed.

    Directory rules end in ``/`` deliberately. Every other rule names one exact path: treating
    ``AGENTS.md`` as a string prefix also exempted paths such as ``AGENTS.md.generated`` from the
    fail-safe all-checks behavior.
    """
    return path.startswith(prefix) if prefix.endswith("/") else path == prefix


def _longest_prefix_groups(path: str) -> frozenset[str] | None:
    """The groups named by the longest matching prefix rule for `path`."""

    best: frozenset[str] | None = None
    best_len = -1
    for prefix, groups in PREFIX_RULES:
        if _rule_matches(path, prefix) and len(prefix) > best_len:
            best, best_len = groups, len(prefix)
    return best


def _cross_shared_trigger(paths: list[str], selected: frozenset[str]) -> bool:
    """Whether the cross group was triggered by an input shared by every tool.

    A trigger from another tool's files under `cross/` selects only that
    tool's gate-exclusive steps. A trigger from outside `cross/` — workspace
    manifests, shared launchers, examples — can change every paired
    implementation at once, as can the shared differential runner itself.
    """

    if CROSS not in selected:
        return False
    for path in paths:
        if path == "cross/differential.py":
            return True
        if path.startswith("cross/"):
            continue
        groups = _longest_prefix_groups(path)
        if groups is not None and CROSS in groups:
            return True
    return False


def cross_filter_for_all(
    paths: list[str], *, manifest: Manifest | None = None
) -> tuple[frozenset[str] | None, str]:
    """Which components' gate-exclusive steps an `--all` run keeps (#1190).

    Returns (components, plan note) for `partition_gate_exclusive`, where
    components `None` means keep every gate-exclusive step. The filter is
    the existing path selection, fail-closed: a path `selection_for` would
    fail-safe on (no prefix rule and no owning component) or a shared cross
    trigger keeps all five, because either can change every paired
    implementation at once. A change that selects no paired tool's
    component — a page-only edit above all — keeps none of them: on
    2026-11-09 that was half of all gated heads, at ~22 minutes of
    whole-host serialization each (#1180 Part B).
    """

    active_manifest = manifest if manifest is not None else load_manifest()
    selected, _because, components, _component_because = selection_for(
        paths, manifest=active_manifest
    )
    unclassified = [
        path
        for path in paths
        if _longest_prefix_groups(path) is None
        and not components_for_paths([path], active_manifest)[0]
    ]
    if unclassified:
        shown = ", ".join(sorted(unclassified)[:3])
        return None, f"unclassified path(s) ({shown}) — keeping all gate-exclusive steps"
    if _cross_shared_trigger(paths, selected):
        return None, "shared cross trigger — keeping all gate-exclusive steps"
    return components, "path-filtered by changed paths"


def selection_for(
    paths: list[str], *, manifest: Manifest | None = None
) -> tuple[frozenset[str], dict[str, str], frozenset[str], dict[str, str]]:
    """Map changed paths onto validation groups and tool components.

    A component match selects only that tool and its declared reverse dependencies. An
    unrecognised path still selects every group and names itself as the reason.
    """
    active_manifest = manifest if manifest is not None else load_manifest()
    components, component_because = components_for_paths(paths, active_manifest)
    selected: set[str] = set(ALWAYS)
    because: dict[str, str] = {group: "always" for group in ALWAYS}
    for path in sorted(paths):
        path_components, _ = components_for_paths([path], active_manifest)
        best = _longest_prefix_groups(path)
        # Root `make mypy` scans the whole checkout, not only `py/`. A Python helper stored in an
        # otherwise prose-only directory is executable validation input and must not inherit the
        # directory's exemption. That includes the website's demo recorders under site/demos/.
        # Vibe-talk's operator scripts are the one explicit exception: its own selected gate runs
        # the same strict mypy contract over that directory.
        root_mypy_observes = (
            Path(path).suffix in {".py", ".pyi"}
            and path.startswith(("ai_docs/", "reviews/", "skills/", "common/docs/", "site/"))
            and (best is None or WORKSPACE not in best)
        )
        if root_mypy_observes:
            selected.add(PYTHON)
            because.setdefault(PYTHON, path)
            if best is None:
                best = frozenset()
        if path_components:
            # Component ownership answers WHICH tool. These labels cover shared language-level
            # checks that are intentionally not duplicated in every tool fragment.
            if path.startswith("py/") or path.startswith("scripts/"):
                selected.add(PYTHON)
                because.setdefault(PYTHON, path)
            if path.startswith("rs/"):
                selected.add(RUST)
                because.setdefault(RUST, path)
            if path.startswith("common/docs/"):
                selected.add(DOCS)
                because.setdefault(DOCS, path)
        if best is None:
            if path_components:
                continue
            # Fail safe. A path nobody has classified is a path nobody has reasoned about.
            for group in sorted(ALL_GROUPS):
                because.setdefault(group, f"{path} (unclassified path — running everything)")
            selected |= set(ALL_GROUPS)
            continue
        for group in sorted(best):
            because.setdefault(group, path)
        selected |= set(best)
    return frozenset(selected), because, components, component_because


def groups_for(
    paths: list[str], *, manifest: Manifest | None = None
) -> tuple[frozenset[str], dict[str, str]]:
    """Compatibility surface for tests and callers interested only in broad groups."""

    selected, because, _components, _component_because = selection_for(
        paths, manifest=manifest
    )
    return selected, because


def changed_paths(base: str, *, repo_root: Path = REPO_ROOT) -> list[str]:
    """Paths differing from `base`, including uncommitted and untracked work."""
    out: set[str] = set()
    merge_base = subprocess.run(
        ("git", "merge-base", base, "HEAD"), cwd=repo_root, capture_output=True, text=True, check=False
    )
    ref = merge_base.stdout.strip() if merge_base.returncode == 0 else base
    for argv in (
        # Disable rename collapsing deliberately. A move can cross component boundaries, so the
        # source and destination must both participate in selection: a rename from a package into
        # prose still owes the checks which could observe the removed package file, and vice versa.
        ("git", "diff", "--no-renames", "--name-only", f"{ref}...HEAD"),
        ("git", "diff", "--no-renames", "--name-only", "HEAD"),
        ("git", "ls-files", "--others", "--exclude-standard"),
    ):
        done = subprocess.run(argv, cwd=repo_root, capture_output=True, text=True, check=False)
        if done.returncode != 0:
            raise SystemExit(f"validate: `{' '.join(argv)}` failed:\n{done.stderr.strip()}")
        out.update(line for line in done.stdout.splitlines() if line)
    return sorted(out)


#: The cross-language differential writes one JSON verdict record per tool into this directory
#: (``COVERAGE_DIR_ENV`` in ``cross/differential.py``). dagrun reports a node that passed with
#: allowed skips as a plain PASS, so these records are the only way this summary can tell a
#: PARTIAL cross node from a complete one.
CROSS_COVERAGE_ENV = "AGENT_UTILS_CROSS_COVERAGE_DIR"

#: The host settings validate.env may supply (see HOST SETTINGS in the module docstring).
HOST_ENV_KEYS = ("AGENT_UTILS_NPM", "AGENT_UTILS_NODE_DIST_URL", "npm_config_registry")


class HostEnvError(ValueError):
    """validate.env is malformed or names a setting validation does not accept."""


def host_env_path(environ: Mapping[str, str]) -> Path:
    """Where this user's host settings live: $XDG_CONFIG_HOME, else ~/.config."""
    config = environ.get("XDG_CONFIG_HOME") or str(Path(environ.get("HOME") or "~").expanduser() / ".config")
    return Path(config) / "agent-utils" / "validate.env"


def parse_host_env(text: str, source: str) -> dict[str, str]:
    """Parse validate.env, refusing rather than ignoring anything it does not understand."""
    values: dict[str, str] = {}
    for number, raw in enumerate(text.splitlines(), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        key, sep, value = line.partition("=")
        key, value = key.strip(), value.strip()
        where = f"{source}:{number}"
        if not sep or not key:
            raise HostEnvError(f"{where}: expected KEY=VALUE, got {line!r}")
        if key not in HOST_ENV_KEYS:
            raise HostEnvError(f"{where}: {key!r} is not a host setting; allowed: {', '.join(HOST_ENV_KEYS)}")
        if key in values:
            raise HostEnvError(f"{where}: {key} is set twice")
        if not value:
            raise HostEnvError(f"{where}: {key} has an empty value")
        values[key] = value
    return values


def host_env(environ: Mapping[str, str]) -> dict[str, str]:
    """The settings validate.env adds to the graph's environment, announced by key only."""
    path = host_env_path(environ)
    try:
        text = path.read_text(encoding="utf-8")
    except FileNotFoundError:
        return {}
    values = parse_host_env(text, str(path))
    added = {key: value for key, value in values.items() if key not in environ}
    kept = sorted(set(values) - set(added))
    line = f"validate: host settings {path} set {', '.join(sorted(added)) or 'nothing'}"
    if kept:
        line += f"; already in the environment, kept: {', '.join(kept)}"
    print(line, flush=True)
    return added


class CrossCoverage(NamedTuple):
    """What the cross verdict records of one validation run show."""

    #: Lines to print, one per record that skipped checks, record that could not be read, or
    #: selected cross node that left no readable record.
    lines: list[str]
    #: Skipped checks across all readable records.
    skipped: int
    #: Records that could not be read or did not have the expected shape.
    unreadable: int
    #: Selected cross nodes, in the order given, with no readable record naming them.
    missing: list[str]
    #: Skipped checks across all readable records, counted per skip kind and sorted by kind.
    by_kind: tuple[tuple[str, int], ...] = ()


def _kind_counts(counts: Mapping[str, int]) -> str:
    """``kind N`` for each skip kind, sorted by kind: ``boxing 3, delegated-live-scope 2``."""

    return ", ".join(f"{kind} {counts[kind]}" for kind in sorted(counts))


def cross_coverage_report(directory: Path, expected_nodes: Sequence[str]) -> CrossCoverage:
    """Describe the cross verdict records in `directory` against the nodes that should write them.

    Each node in `expected_nodes` runs ``cross/differential.py`` and must leave a record naming it.
    A record that cannot be read, and a node with no readable record, are counted rather than
    ignored: either may have been the one PARTIAL node, so the summary must not claim full
    coverage without it. Among the readable records, those that skipped checks are listed,
    including a record from a node that was not expected.
    """

    lines: list[str] = []
    skipped_total = 0
    unreadable = 0
    recorded_nodes: set[str] = set()
    by_kind: dict[str, int] = {}
    for path in sorted(directory.glob("*.json")):
        try:
            value: object = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, UnicodeError, json.JSONDecodeError) as exc:
            lines.append(f"  unreadable cross coverage record {path.name}: {exc}")
            unreadable += 1
            continue
        skipped = value.get("skipped") if isinstance(value, dict) else None
        if not isinstance(value, dict) or not isinstance(skipped, list):
            lines.append(f"  malformed cross coverage record {path.name}: {value!r}")
            unreadable += 1
            continue
        node = value.get("node")
        if isinstance(node, str):
            recorded_nodes.add(node)
        if not skipped:
            continue
        labels: list[str] = []
        kinds: dict[str, int] = {}
        for item in skipped:
            if isinstance(item, dict):
                labels.append(str(item.get("label")))
                kind = str(item.get("kind"))
            else:
                labels.append(repr(item))
                kind = "malformed"
            kinds[kind] = kinds.get(kind, 0) + 1
            by_kind[kind] = by_kind.get(kind, 0) + 1
        skipped_total += len(skipped)
        where = value.get("node") or "top level"
        lines.append(
            f"  {where} ({value.get('tool')}, {value.get('verdict')}): {value.get('checks')} "
            f"check(s) ran; {len(skipped)} skipped [{_kind_counts(kinds)}]: "
            f"{', '.join(labels)}"
        )
    missing = [node for node in expected_nodes if node not in recorded_nodes]
    for node in missing:
        lines.append(
            f"  {node}: no readable coverage record, so its coverage could not be confirmed"
        )
    return CrossCoverage(
        lines, skipped_total, unreadable, missing, tuple(sorted(by_kind.items()))
    )


def _workflow_command_data(text: str) -> str:
    """Escape `text` as the message of a GitHub Actions workflow command."""

    return text.replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")


def announce_partial(verdict: str, lines: Sequence[str], environ: Mapping[str, str]) -> None:
    """Surface a PARTIAL verdict where a hosted job would otherwise show only a green tick.

    A PARTIAL run exits 0, so GitHub Actions marks its step as passed. Under Actions this emits a
    warning annotation and appends the verdict and its per-node lines to the job summary. Outside
    Actions it does nothing. A summary that cannot be written is reported, not fatal: the caller
    still prints the verdict and exits 0.
    """

    detail = "\n".join([verdict, *lines])
    if environ.get("GITHUB_ACTIONS") == "true":
        print(f"::warning title=validate PARTIAL::{_workflow_command_data(detail)}", flush=True)
    summary_path = environ.get("GITHUB_STEP_SUMMARY")
    if not summary_path:
        return
    body = "\n".join(lines)
    try:
        with open(summary_path, "a", encoding="utf-8") as handle:
            handle.write(f"### validate: PARTIAL\n\n{verdict}\n\n```text\n{body}\n```\n")
    except OSError as exc:
        print(f"validate: cannot append the PARTIAL verdict to GITHUB_STEP_SUMMARY: {exc}",
              file=sys.stderr)


#: Cross tools whose differential reports through ``Report`` and therefore
#: emits per-check timings. The remaining tools (herdr-agent, agentctl) keep
#: their own counts and emit no per-check record; expecting one would be a
#: false inventory. Kept beside the selector so the boundary is explicit.
TIMING_CROSS_TOOLS = frozenset(
    {"dagrun", "cpuset-alloc", "tick-hub", "pr-landing-planner", "herdr-run"}
)

#: Command fragments that mark a gate as timing-capable. A selected gate
#: whose command carries one must leave a primary timing record; a missing
#: record is reported like a missing cross coverage record, not absorbed
#: into a smaller report.
TIMING_COMMAND_MARKERS = (
    "run_component_tests.py --component",
    "run_pytest_shard.py --shard",
    "-p run_pytest_shard",
    "check_dagrun_examples.py",
    "check_python_packages.py --project",
    "check_rust_packages.py --crate",
    "case_timings.py run-cargo",
    "playwright test",
    "--test-reporter=tap",
)


#: The five gate-exclusive cross differentials are DISABLED by default.
#: Owner decision (2026-11-10, reply 346, 3:23 PM ET), (one consuming-project name redacted): "Insane.
#: I hate tests that are real time sensitive. We fought this battle with
#: [redacted]. Disable those tests and have an agent work to bring them back
#: when they're robust. Each product lane must make its tests fast, high
#: signal, and robust to running on a hot machine (>75% cpu on 316 HW
#: threads). AND there should be good test selection so we are not running
#: tests for agent-util A when only B is modified. Anything else should be
#: disabled until we can improve it." #1249 cross-differentials-robust
#: tracks bringing them back without wall-clock budgets. Until then no
#: default, `--all`, or `--cross-all` run selects them (the exclusive
#: phase is empty); `--cross-differentials` runs them on demand.
CROSS_DIFFERENTIALS_DISABLED_NOTE = (
    "cross differentials disabled by default (owner reply 346, 2026-11-10; "
    "#1249 cross-differentials-robust); --cross-differentials runs them on demand"
)

#: DAG label for steps that run alone on the host when opted in with
#: `--cross-differentials`: after the main
#: phase, under one fresh `gate.py --exclusive` admission per gate, one step
#: at a time. These steps assert exact outcomes of small wall clocks inside
#: the tools they compare (agentctl's Herdr probes against a Python fake
#: Herdr), so sharing the box with another gate's steps — not the step
#: budgets — turns them red; exclusive reruns pass. Running the five in
#: parallel inside the one admission also diverged (a verified-idle-composer
#: probe timed out beside its four siblings), so the phase is serial within
#: the admission: still one queue slot per gate. See
#: validation/cross.dag.yaml. #1067 cross-differential-load-aware makes the
#: comparisons load-aware so a host may run them in the main phase instead
#: (``CROSS_MAIN_PHASE_ENV``); the label stays as the partition's marker in
#: both positions and never affects which steps are selected.
GATE_EXCLUSIVE_LABEL = "gate-exclusive"

#: Caller-environment switch for the five `gate-exclusive` differentials
#: (#1067 cross-differential-load-aware). Unset, or any value other than
#: ``"1"``, keeps today's exclusive phase, so landing the load-aware change
#: is safe on its own. ``AGENT_UTILS_CROSS_IN_MAIN_PHASE=1`` runs the kept
#: steps in the main (shared) phase instead; a host flips only after its own
#: paired timing record passes. Selection is identical in both positions:
#: a step whose component was not selected runs in neither phase.
CROSS_MAIN_PHASE_ENV = "AGENT_UTILS_CROSS_IN_MAIN_PHASE"

#: Marker set on the gate.py exclusive-phase invocation when this run
#: deliberately selected its gate-exclusive steps (the disabled five only
#: reach the phase via `--cross-differentials`): gate.py records it on
#: the ticket, so a deliberate phase is never reaped as a stale phase
#: from a tree that predates the disable (#1249).
CROSS_DIFFERENTIALS_ENV = "AGENT_UTILS_CROSS_DIFFERENTIALS"


def cross_in_main_phase(environ: Mapping[str, str] | None = None) -> bool:
    """Whether the caller opted the gate-exclusive differentials into the main phase."""
    return (os.environ if environ is None else environ).get(CROSS_MAIN_PHASE_ENV) == "1"


def partition_gate_exclusive(
    steps: dict[str, dict[str, object]],
    chosen: set[str],
    *,
    components: frozenset[str] | None = None,
    keep_all: bool = False,
    exclusive_phase: bool | None = None,
    differentials_enabled: bool = False,
) -> tuple[list[str], list[str]]:
    """Split chosen tags into the main phase and the gate-exclusive phase.

    Both lists keep graph order. A chosen step that depends on an exclusive
    step joins the exclusive phase too: running it in the main phase would
    pull its exclusive dependency in beside everything else.

    Unless `keep_all`, an exclusive step is kept only when one of its
    `component-*` labels names a selected component: the selection map
    (validation/components.json) says which paths can change each tool's
    behaviour, and a change that cannot affect a tool must not pay for its
    wall-clock-sensitive differential — above all not at exclusive-admission
    prices. `keep_all` is for `--cross-all`, the unclassified-path fail-safe
    (which selects every group), and cross-group triggers from outside
    `cross/` (workspace manifests, shared launchers) that can change every
    paired implementation at once. Since #1190, `--all` passes the
    fail-closed `cross_filter_for_all` components instead of keeping all.

    `exclusive_phase` chooses where the kept steps run, never which steps
    are kept: the component filter and the dropped-dependent cascade are
    identical in both positions, so a filtered-out step runs in neither
    phase. `None` (the default) reads ``CROSS_MAIN_PHASE_ENV``: unset keeps
    the exclusive phase; ``"1"`` puts every kept step in the main phase and
    returns an empty exclusive list (#1067 cross-differential-load-aware).
    Tests pass it explicitly so the ambient environment cannot flip them.

    Unless `differentials_enabled` (the `--cross-differentials` opt-in),
    every labelled step is dropped regardless of `keep_all`, `components`,
    or `exclusive_phase`: the five differentials are disabled by default
    (#1249 cross-differentials-robust; see the note above
    ``GATE_EXCLUSIVE_LABEL``), so the exclusive phase is empty.
    """
    if exclusive_phase is None:
        exclusive_phase = not cross_in_main_phase()

    def labels(tag: str) -> set[str]:
        raw = steps[tag].get("labels")
        return {label for label in raw if isinstance(label, str)} if isinstance(raw, list) else set()

    labelled = {tag for tag in chosen if GATE_EXCLUSIVE_LABEL in labels(tag)}
    exclusive = set(labelled)
    if not differentials_enabled:
        # Disabled by default (#1249): no caller selection — not the
        # component filter, not keep_all (--cross-all), not the main-phase
        # position — runs a labelled step. Only --cross-differentials does.
        exclusive = set()
    elif not keep_all and components is not None:
        # A filtered-out exclusive step is not selected at all: it must not
        # fall back into the main phase, where it would run beside every
        # other step — the exact condition the label exists to avoid.
        exclusive = {
            tag
            for tag in exclusive
            if any(
                label.removeprefix("component-") in components
                for label in labels(tag)
                if label.startswith("component-")
            )
        }
    dropped = labelled - exclusive
    changed = True
    while changed:
        changed = False
        for tag in chosen - dropped:
            raw_deps = steps[tag].get("deps")
            deps = raw_deps if isinstance(raw_deps, list) else []
            if any(dep in dropped for dep in deps if isinstance(dep, str)):
                # A step that depends on a filtered-out exclusive step cannot
                # run either: dagrun would pull the dependency in beside
                # everything else, or fail for its absence.
                dropped.add(tag)
                exclusive.discard(tag)
                changed = True
            elif tag not in exclusive and any(
                dep in exclusive for dep in deps if isinstance(dep, str)
            ):
                exclusive.add(tag)
                changed = True
    if not exclusive_phase:
        # Main-phase position (#1067): every kept step — labelled or not,
        # dependent or not — runs in the main phase in graph order. The
        # `dropped` set above is the whole selection story in both
        # positions, so a filtered-out step still runs nowhere.
        return ([tag for tag in steps if tag in chosen and tag not in dropped], [])
    return (
        [
            tag
            for tag in steps
            if tag in chosen and tag not in labelled and tag not in exclusive and tag not in dropped
        ],
        [tag for tag in steps if tag in exclusive],
    )


#: DAG label for real-time-sensitive steps DISABLED by default. Owner
#: decision (2026-11-10, reply 346, 3:23 PM ET), (one consuming-project
#: name redacted): "Insane. I hate tests that are real time sensitive.
#: We fought this battle with [redacted]. Disable those tests and have
#: an agent work to bring them back when they're robust. Each product
#: lane must make its tests fast, high signal, and robust to running on
#: a hot machine (>75% cpu on 316 HW threads). AND there should be good
#: test selection so we are not running tests for agent-util A when only
#: B is modified. Anything else should be disabled until we can improve
#: it." #1250 vibe-talk-timing-robust (vibe-talk/tests.dag.yaml) and
#: #1252 timeline-benchmark-optin (validation/timeline.dag.yaml) track
#: bringing each labelled step back as a behaviour-only check; the
#: comment above each labelled step names its issue. Until then no
#: default or `--all` run selects them; `--include-disabled` (or
#: ``AGENT_UTILS_INCLUDE_DISABLED=1``) runs them on demand. This is the
#: sibling of the cross-differential disable above (#1249), which keeps
#: its own label and opt-in because those steps also need an exclusive
#: phase; labelled steps here need only exclusion.
DISABLED_OPTIN_LABEL = "disabled-optin"

#: Caller-environment switch for the `disabled-optin` steps. Only ``"1"``
#: opts in, exactly like ``CROSS_MAIN_PHASE_ENV``.
INCLUDE_DISABLED_ENV = "AGENT_UTILS_INCLUDE_DISABLED"

DISABLED_OPTIN_NOTE = (
    "timing-sensitive steps disabled by default (owner reply 346, "
    "2026-11-10; #1250 vibe-talk-timing-robust, "
    "#1252 timeline-benchmark-optin); --include-disabled runs them on demand"
)


def include_disabled_steps(environ: Mapping[str, str] | None = None) -> bool:
    """Whether the caller opted the `disabled-optin` steps into the run."""
    return (os.environ if environ is None else environ).get(INCLUDE_DISABLED_ENV) == "1"


def drop_disabled_optin(
    steps: dict[str, dict[str, object]],
    chosen: set[str],
    *,
    include_disabled: bool = False,
) -> tuple[set[str], list[str]]:
    """Drop `disabled-optin` steps (and their dependents) unless opted in.

    Returns the kept tags and, in graph order, the dropped labelled tags
    (dependents dropped by cascade are not listed: the plan names the
    disabled checks themselves). A chosen step that depends on a dropped
    step is dropped too: dagrun would otherwise pull the disabled step
    back in as a dependency, or fail for its absence. Dependencies OF a
    disabled step (shared setup and build steps) are unaffected.
    """

    def labels(tag: str) -> set[str]:
        raw = steps[tag].get("labels")
        return {label for label in raw if isinstance(label, str)} if isinstance(raw, list) else set()

    labelled = [tag for tag in steps if tag in chosen and DISABLED_OPTIN_LABEL in labels(tag)]
    if include_disabled or not labelled:
        return set(chosen), []
    dropped = set(labelled)
    changed = True
    while changed:
        changed = False
        for tag in chosen - dropped:
            raw_deps = steps[tag].get("deps")
            deps = raw_deps if isinstance(raw_deps, list) else []
            if any(dep in dropped for dep in deps if isinstance(dep, str)):
                dropped.add(tag)
                changed = True
    return set(chosen) - dropped, labelled


def _is_timing_gate(command: str) -> bool:
    if "cross/differential.py" in command:
        tokens = command.split()
        tools = [
            tokens[index + 1]
            for index, token in enumerate(tokens[:-1])
            if token == "--tool"
        ]
        return any(tool in TIMING_CROSS_TOOLS for tool in tools)
    return any(marker in command for marker in TIMING_COMMAND_MARKERS)


def case_timing_report(
    directory: Path, expected_gates: Sequence[str] = ()
) -> tuple[list[str], list[str], list[str]]:
    """Verify the per-case timing records of one run and render the bounded report.

    Returns the report lines, the inventory problems, and the expected gates
    that left no record, separately: a mismatched record means per-case
    counts from this run cannot be trusted (a failure), while a missing one
    means coverage could not be confirmed (PARTIAL, like cross coverage).
    """

    verified = case_timings.verify_directory(directory, expected_gates=expected_gates)
    return (
        case_timings.render_report(verified),
        verified.problems,
        list(verified.missing_gates),
    )


def _node22_runtime_skips(chosen_tags: set[str]) -> list[str]:
    """--skip arguments for setup.node22 when Node 22 cannot be provisioned (#867).

    Provisioning is attempted first (it is cheap when already installed);
    only a failed attempt on a host whose own node is too old (or absent)
    turns the setup step into a runtime skip with the reason named, so its
    dependents dependency-skip instead of failing the gate.
    """

    if "setup.node22" not in chosen_tags:
        return []
    try:
        provisioned = subprocess.run(
            [str(REPO_ROOT / "scripts" / "provision-node22")],
            capture_output=True,
            text=True,
            timeout=300,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        provisioned = None
        provision_detail = str(exc)
    if provisioned is not None and provisioned.returncode == 0:
        return []
    if provisioned is not None:
        lines = (provisioned.stderr or provisioned.stdout).strip().splitlines()
        provision_detail = lines[-1] if lines else f"exit {provisioned.returncode}"
    try:
        host_node = subprocess.run(
            ["node", "--version"], capture_output=True, text=True, timeout=30
        ).stdout.strip()
    except OSError:
        host_node = ""
    major = None
    if host_node.startswith("v"):
        try:
            major = int(host_node[1:].split(".")[0])
        except ValueError:
            major = None
    if major is not None and major >= 20:
        return []
    reason = (
        f"host node {host_node or 'missing'} < 20; Node 22 not provisioned "
        f"({provision_detail})"
    )
    print(f"validate: runtime-skip setup.node22: {reason}")
    return [f"setup.node22={reason}"]


def run(
    selected: frozenset[str],
    components: frozenset[str],
    *,
    all_contract: bool,
    coverage_dir: Path | None = None,
    timing_dir: Path | None = None,
    host: Mapping[str, str] | None = None,
    keep_going: bool = False,
    gate_exclusive_phases: tuple[list[str], list[str]] | None = None,
    pytest_root: Path | None = None,
) -> int:
    """Execute one flattened dagrun graph for the selected contract.

    With `coverage_dir`, every cross differential node writes its verdict record there. With
    `timing_dir`, every timing-capable runner writes its normalized per-case record there
    (see ``scripts/case_timings.py``). `host` is the validate.env settings, added to the
    graph's environment only. With `keep_going`, a failed step no longer aborts the steps
    that do not depend on it (dagrun --keep-going), so one run reports every independent
    failure. `#362 ci-failure-masking`. Steps labelled `gate-exclusive` run after the
    main phase, one at a time under one fresh `gate.py --exclusive` admission, so no
    other gate's steps — and none of their own siblings — share the host with them —
    unless the caller opted them into the main phase (`CROSS_MAIN_PHASE_ENV`,
    #1067 cross-differential-load-aware), in which case the partition puts them in
    `main_tags` and this run is a single dagrun invocation.
    With `pytest_root`, every pytest process in the graph keeps its temp directories there
    (``pytest_temp_root``).
    """

    cpu_count = os.cpu_count() or 1
    max_cpus = os.environ.get("VALIDATE_MAX_CPUS", str(min(32, cpu_count)))
    max_steps = os.environ.get("VALIDATE_MAX_STEPS", str(min(24, cpu_count)))

    def dagrun_command(max_steps_override: str | None = None) -> list[str]:
        return [
            str(REPO_ROOT / "common" / "bin" / "dagrun"),
            "run",
            "--dag",
            str(REPO_ROOT / "validation.dag.yaml"),
            "--max-steps",
            max_steps_override or max_steps,
            "--max-cpus",
            max_cpus,
            "--planner",
            "critical-path",
            "--profile",
        ]

    extra = os.environ.get("VALIDATE_DAGRUN_FLAGS", os.environ.get("BOX_FLAGS", ""))
    env = dict(os.environ)
    env.update(host or {})
    # Every step in the graph -- pytest shards and the Rust test harnesses
    # alike -- inherits a throwaway global git config, so no test can write
    # the operator's real ~/.gitconfig (#1474 tests-rewrite-global-gitconfig).
    # The pytest conftest guard installs the same redirect per session and
    # fails the session if the real file's content changes. A caller-provided
    # GIT_CONFIG_GLOBAL is kept.
    if "GIT_CONFIG_GLOBAL" not in env:
        gitconfig_dir = Path(tempfile.mkdtemp(prefix="agent-utils-validate-gitconfig-"))
        gitconfig_sentinel = gitconfig_dir / "gitconfig"
        gitconfig_sentinel.write_bytes(
            b"[user]\n\tname = Agent Utils Tests\n"
            b"\temail = agent-utils-tests@example.invalid\n"
            b"[uploadpack]\n\tallowfilter = true\n"
        )
        gitconfig_sentinel.chmod(0o600)
        env["GIT_CONFIG_GLOBAL"] = str(gitconfig_sentinel)
    env.setdefault("GIT_CONFIG_NOSYSTEM", "1")
    # Tests must never resolve the live harness registry (#1499
    # tests-touch-live-registry): default agentctl to a per-run scratch
    # with auto-migrate off, unless the caller already chose a registry.
    if "AGENTCTL_REGISTRY" not in env:
        env["AGENTCTL_REGISTRY"] = str(
            Path(tempfile.mkdtemp(prefix="agent-utils-validate-agentctl-")) / "registry"
        )
    env.setdefault("AGENTCTL_LAYOUT_AUTO_MIGRATE", "0")
    if coverage_dir is not None:
        env[CROSS_COVERAGE_ENV] = str(coverage_dir)
    if pytest_root is not None:
        env[PYTEST_TEMPROOT_ENV] = str(pytest_root)
    if timing_dir is not None:
        env[case_timings.TIMING_DIR_ENV] = str(timing_dir)
        # An ambient nested marker would classify the whole run as an
        # alternate-environment rerun and empty the primary report.
        env.pop(case_timings.TIMING_NESTED_ENV, None)

    # `main` computes the phase split from the canonical graph and passes it
    # in; without it (direct callers, the graph-contract tests) the run is
    # the historical single dagrun invocation. "Phases given" must stay
    # distinguishable from "no phases": in the main-phase position the
    # exclusive list is empty by design, and falling back to `--labels`
    # would silently skip force-added differentials that carry no
    # `validation-cross` label while the caller still expects their
    # coverage records (#1067 cross-differential-load-aware).
    if gate_exclusive_phases is not None:
        phases_given = True
        main_tags, exclusive_tags = gate_exclusive_phases
    else:
        phases_given = False
        main_tags, exclusive_tags = [], []
    result = 0
    if exclusive_tags:
        # Steps labelled gate-exclusive run afterwards, one at a time under
        # ONE fresh exclusive gate admission (see partition_gate_exclusive):
        # one queue slot per gate, nothing from any other gate beside them.
        print(
            f"\nvalidate: {len(exclusive_tags)} gate-exclusive step(s) run alone "
            f"after the main phase, one at a time under one admission: {', '.join(exclusive_tags)}",
            flush=True,
        )
    if main_tags or not exclusive_tags:
        command = dagrun_command()
        if exclusive_tags or (phases_given and main_tags):
            command.extend(("--selected", ",".join(main_tags)))
        elif not all_contract:
            labels = {GROUP_LABELS[group] for group in selected}
            labels.update(f"component-{name}" for name in components)
            command.extend(("--labels", ",".join(sorted(labels))))
        for skip in _node22_runtime_skips(set(main_tags)):
            command.extend(("--skip", skip))
        if keep_going:
            command.append("--keep-going")
        command.extend(shlex.split(extra))
        print(f"\n=== flattened validation DAG ({len(command)} argv entries) ===", flush=True)
        done = subprocess.run(command, cwd=REPO_ROOT, check=False, env=env)
        result = done.returncode
    if exclusive_tags and (result == 0 or keep_going):
        # gate.py releases this run's own admission slot before queueing
        # (the parent is blocked here), then these steps run under the one
        # exclusive admission: no other gate's steps share the host while
        # they run. One admission per gate, however many steps it has.
        # --max-steps 1 keeps the steps serial inside the admission: run
        # together, the five still diverged (a verified-idle-composer probe
        # timed out beside its siblings), so alone means alone.
        command = [
            sys.executable,
            str(Path(__file__).with_name("gate.py")),
            "--exclusive",
            "--",
            *dagrun_command(max_steps_override="1"),
            "--selected",
            ",".join(exclusive_tags),
        ]
        if keep_going:
            command.append("--keep-going")
        command.extend(shlex.split(extra))
        print(
            f"\n=== gate-exclusive phase ({len(exclusive_tags)} step(s), one admission, serial) ===",
            flush=True,
        )
        # This run deliberately selected these steps (the disabled five
        # only reach here via --cross-differentials): mark the phase so
        # gate.py never reaps it as stale (#1249).
        phase_env = {**env, CROSS_DIFFERENTIALS_ENV: "1"}
        done = subprocess.run(command, cwd=REPO_ROOT, check=False, env=phase_env)
        if done.returncode != 0 and result == 0:
            result = done.returncode
    if result != 0:
        print("\nvalidate: FAILED in flattened validation DAG", file=sys.stderr)
    return result


def _restore_owner_access(path: Path) -> None:
    """Make a test-written tree removable without following its symlinks."""
    if path.is_symlink() or not path.exists():
        return
    path.chmod(path.stat().st_mode | stat.S_IRWXU)
    if path.is_dir():
        for child in path.iterdir():
            _restore_owner_access(child)


def remove_test_tree(path: Path) -> None:
    """Remove a tree tests wrote, including parts a permissions test made read-only."""
    if not path.exists():
        return
    try:
        shutil.rmtree(path)
    except OSError:
        _restore_owner_access(path)
        shutil.rmtree(path)


def sweep_stale_pytest_roots(now: float | None = None) -> list[Path]:
    """Remove the pytest temp roots killed runs left behind; returns what was removed.

    A root is stale once it is this user's, has stood untouched for
    ``STALE_PYTEST_ROOT_SECONDS``, and the process its name records is gone. A live run's
    root is never touched, whatever its age.
    """
    moment = time.time() if now is None else now
    removed: list[Path] = []
    for path in sorted(Path(tempfile.gettempdir()).glob(f"{PYTEST_ROOT_PREFIX}*")):
        try:
            metadata = path.lstat()
        except OSError:
            continue
        if (
            not stat.S_ISDIR(metadata.st_mode)
            or metadata.st_uid != os.getuid()
            or moment - metadata.st_mtime < STALE_PYTEST_ROOT_SECONDS
        ):
            continue
        owner = path.name[len(PYTEST_ROOT_PREFIX):].split("-", 1)[0]
        if owner.isdigit() and Path("/proc", owner).exists():
            continue
        try:
            remove_test_tree(path)
        except OSError as error:
            print(f"validate: could not remove stale pytest root {path}: {error}", file=sys.stderr)
            continue
        removed.append(path)
    return removed


@contextlib.contextmanager
def pytest_temp_root() -> Iterator[Path]:
    """A private root for one run's pytest temp directories, removed when the run ends.

    `#941 gate-pytest-basetemp-leak`. Every pytest process of the run, shards and nested
    sessions alike, numbers its basetemp under ``ROOT/pytest-of-USER`` (pytest's numbering
    and locking keep concurrent processes apart), so no run numbers, prunes or deletes
    another's. The root goes when the run ends, passed or failed, including directories a
    test made read-only. A SIGTERM ends the run through the same cleanup; a root that a
    harder kill left behind is removed by a later run once it is stale.
    """
    sweep_stale_pytest_roots()
    root = Path(tempfile.mkdtemp(prefix=f"{PYTEST_ROOT_PREFIX}{os.getpid()}-"))

    def terminate(signum: int, _frame: FrameType | None) -> None:
        raise SystemExit(128 + signum)

    previous = signal.signal(signal.SIGTERM, terminate)
    try:
        yield root
    finally:
        signal.signal(signal.SIGTERM, previous)
        try:
            remove_test_tree(root)
        except OSError as error:
            print(f"validate: could not remove pytest root {root}: {error}", file=sys.stderr)


def report(
    selected: frozenset[str],
    because: dict[str, str],
    components: frozenset[str],
    component_because: dict[str, str],
    paths: list[str],
) -> None:
    print(f"validate: {len(paths)} changed path(s)")
    for group in sorted(ALL_GROUPS):
        if group in selected:
            print(f"  RUN   {group:<11} — {WHY[group]} [{because.get(group, '?')}]")
        elif components:
            print(
                f"  narrow {group:<11} — no all-component sweep; matching component nodes "
                "may still run below"
            )
        else:
            print(f"  skip  {group:<11} — nothing changed that it can observe")
    for component in sorted(components):
        print(
            f"  RUN   component:{component:<20} — owned tool and reverse-dependency checks "
            f"[{component_because.get(component, '?')}]"
        )
    if selected <= ALWAYS and not components:
        print("  (nothing selected by path: every changed path is prose or configuration)")


def selected_graph(
    selected: frozenset[str], components: frozenset[str], *, all_contract: bool
) -> tuple[dict[str, dict[str, object]], set[str]]:
    """Load the canonical graph with the public loader and select what `run` would run.

    Returns every step by tag, in graph order, and the tags selected by the same labels `run`
    passes to dagrun, closed over dependencies.
    """

    completed = subprocess.run(
        (
            str(REPO_ROOT / "common" / "bin" / "dagrun"),
            "json",
            "--dag",
            str(REPO_ROOT / "validation.dag.yaml"),
        ),
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    if completed.returncode != 0:
        raise SystemExit(f"validate: cannot load canonical DAG:\n{completed.stderr.strip()}")
    raw_value: object = json.loads(completed.stdout)
    if not isinstance(raw_value, dict) or not isinstance(raw_value.get("steps"), list):
        raise SystemExit("validate: canonical DAG JSON has no steps list")
    steps: dict[str, dict[str, object]] = {}
    for index, value in enumerate(raw_value["steps"]):
        if not isinstance(value, dict):
            raise SystemExit(f"validate: canonical DAG step {index} is not an object")
        record: dict[str, object] = {}
        for key, item in value.items():
            if not isinstance(key, str):
                raise SystemExit(f"validate: canonical DAG step {index} has a non-string key")
            record[key] = item
        group, job = record.get("group"), record.get("job")
        if not isinstance(group, str) or not isinstance(job, str):
            raise SystemExit(f"validate: canonical DAG step {index} has no string group/job")
        steps[f"{group}.{job}"] = record

    chosen = set(steps)
    if not all_contract:
        labels = {GROUP_LABELS[group] for group in selected} | {
            f"component-{component}" for component in components
        }
        chosen = set()
        for tag, step in steps.items():
            raw_labels = step.get("labels")
            if not isinstance(raw_labels, list) or not all(
                isinstance(label, str) for label in raw_labels
            ):
                raise SystemExit(f"validate: canonical DAG step {tag} has invalid labels")
            if labels.intersection(label for label in raw_labels if isinstance(label, str)):
                chosen.add(tag)
        pending = list(chosen)
        while pending:
            tag = pending.pop()
            raw_deps = steps[tag].get("deps")
            if not isinstance(raw_deps, list) or not all(
                isinstance(dep, str) for dep in raw_deps
            ):
                raise SystemExit(f"validate: canonical DAG step {tag} has invalid dependencies")
            for dependency in (dep for dep in raw_deps if isinstance(dep, str)):
                if dependency not in steps:
                    raise SystemExit(
                        f"validate: canonical DAG step {tag} names missing dependency {dependency}"
                    )
                if dependency not in chosen:
                    chosen.add(dependency)
                    pending.append(dependency)
    return steps, chosen


def selected_cross_nodes(
    selected: frozenset[str],
    components: frozenset[str],
    *,
    all_contract: bool,
    chosen_tags: set[str] | None = None,
) -> list[str]:
    """Tags of the selected nodes that run ``cross/differential.py``, in graph order.

    Each of them writes a coverage record when it reaches a verdict, so after a passing run a
    missing record means that node's coverage cannot be confirmed. `chosen_tags`
    overrides the graph selection when the caller has already applied the
    gate-exclusive phase split, so the expectation names exactly the nodes
    that will run.
    """

    steps, chosen = selected_graph(selected, components, all_contract=all_contract)
    if chosen_tags is not None:
        chosen = set(chosen_tags)
    return [
        tag
        for tag, step in steps.items()
        if tag in chosen
        and isinstance(command := step.get("cmd"), str)
        and "cross/differential.py" in command
    ]


def selected_timing_gates(
    selected: frozenset[str],
    components: frozenset[str],
    *,
    all_contract: bool,
    chosen_tags: set[str] | None = None,
) -> list[str]:
    """Tags of the selected timing-capable nodes, in graph order.

    Each of them emits a normalized per-case record when it runs, so after a
    passing run a missing record means that gate's cases cannot be confirmed.
    `chosen_tags` overrides the graph selection when the caller has already
    applied the gate-exclusive phase split.
    """

    steps, chosen = selected_graph(selected, components, all_contract=all_contract)
    if chosen_tags is not None:
        chosen = set(chosen_tags)
    return [
        tag
        for tag, step in steps.items()
        if tag in chosen
        and isinstance(command := step.get("cmd"), str)
        and _is_timing_gate(command)
    ]


def report_selected_graph(
    selected: frozenset[str],
    components: frozenset[str],
    *,
    all_contract: bool,
    chosen_tags: set[str] | None = None,
) -> None:
    """Print the exact flattened nodes and resources selected by the public graph loader.

    CI consumes the ``NEED resource:*`` lines when provisioning optional runtimes. Deriving them
    from the selected graph prevents a component label from activating (for example) a browser
    gate while a second, coarser group-name heuristic incorrectly skips browser installation.
    `chosen_tags` overrides the graph selection when the caller has already
    applied the gate-exclusive phase split, so the plan names exactly the
    nodes that will run.
    """

    steps, chosen = selected_graph(selected, components, all_contract=all_contract)
    if chosen_tags is not None:
        chosen = set(chosen_tags)
    resources = sorted(
        {
            resource
            for tag in chosen
            for resource, amount in _step_resources(steps[tag]).items()
            if amount > 0
        }
    )
    for resource in resources:
        print(f"  NEED  resource:{resource}")
    for tag in steps:
        if tag in chosen:
            print(f"  STEP  {tag}")


def _step_resources(step: dict[str, object]) -> dict[str, int]:
    """Return one canonical JSON step's checked resource demand."""

    hint = step.get("hint")
    if not isinstance(hint, dict):
        raise SystemExit("validate: canonical DAG step has no hint object")
    raw_resources = hint.get("resources")
    if not isinstance(raw_resources, dict):
        raise SystemExit("validate: canonical DAG step has no resources object")
    resources: dict[str, int] = {}
    for key, value in raw_resources.items():
        if not isinstance(key, str) or not isinstance(value, int) or isinstance(value, bool):
            raise SystemExit("validate: canonical DAG step has an invalid resource demand")
        resources[key] = value
    return resources


def self_test() -> int:
    """Check mapping and rename semantics offline, without running the build."""
    failures: list[str] = []
    manifest = load_manifest()

    synthetic_steps: dict[str, dict[str, object]] = {
        "cross.solo": {"labels": [GATE_EXCLUSIVE_LABEL], "deps": []},
        "cross.dependent": {"labels": [], "deps": ["cross.solo"]},
        "cross.plain": {"labels": [], "deps": []},
    }
    main_tags, exclusive_tags = partition_gate_exclusive(
        synthetic_steps,
        {"cross.solo", "cross.dependent", "cross.plain"},
        exclusive_phase=True,
        differentials_enabled=True,
    )
    if main_tags != ["cross.plain"] or exclusive_tags != ["cross.solo", "cross.dependent"]:
        failures.append(
            "gate-exclusive partition: a dependent of an exclusive step must "
            f"join the exclusive phase; got main={main_tags} exclusive={exclusive_tags}"
        )

    main_tags, exclusive_tags = partition_gate_exclusive(
        synthetic_steps,
        {"cross.solo", "cross.dependent", "cross.plain"},
        exclusive_phase=False,
        differentials_enabled=True,
    )
    if main_tags != ["cross.solo", "cross.dependent", "cross.plain"] or exclusive_tags != []:
        failures.append(
            "gate-exclusive partition (main-phase position, #1067): every kept "
            "step must run in the main phase in graph order; got "
            f"main={main_tags} exclusive={exclusive_tags}"
        )

    scoped_steps: dict[str, dict[str, object]] = {
        "cross.mine": {"labels": [GATE_EXCLUSIVE_LABEL, "component-agentctl"], "deps": []},
        "cross.theirs": {"labels": [GATE_EXCLUSIVE_LABEL, "component-herdr-run"], "deps": []},
        "cross.plain": {"labels": [], "deps": []},
        "cross.blocked": {"labels": [], "deps": ["cross.theirs"]},
    }
    scoped_main, scoped_exclusive = partition_gate_exclusive(
        scoped_steps,
        {"cross.mine", "cross.theirs", "cross.plain", "cross.blocked"},
        components=frozenset({"agentctl"}),
        exclusive_phase=True,
        differentials_enabled=True,
    )
    if scoped_main != ["cross.plain"] or scoped_exclusive != ["cross.mine"]:
        failures.append(
            "gate-exclusive partition: a step whose component was not selected "
            "must be dropped entirely, not run in the main phase; got "
            f"main={scoped_main} exclusive={scoped_exclusive}"
        )

    scoped_main, scoped_exclusive = partition_gate_exclusive(
        scoped_steps,
        {"cross.mine", "cross.theirs", "cross.plain", "cross.blocked"},
        components=frozenset({"agentctl"}),
        exclusive_phase=False,
        differentials_enabled=True,
    )
    if scoped_main != ["cross.mine", "cross.plain"] or scoped_exclusive != []:
        failures.append(
            "gate-exclusive partition (main-phase position, #1067): selection "
            "must not change with the phase — a step whose component was not "
            "selected, and its dependents, must still run nowhere; got "
            f"main={scoped_main} exclusive={scoped_exclusive}"
        )

    # #1249: disabled by default — no keep_all, component, or phase
    # selection runs a labelled step or its dependent; exclusive is empty.
    disabled_partitions = [
        partition_gate_exclusive(
            synthetic_steps, {"cross.solo", "cross.dependent", "cross.plain"},
            exclusive_phase=True,
        ),
        partition_gate_exclusive(
            synthetic_steps, {"cross.solo", "cross.dependent", "cross.plain"},
            exclusive_phase=False,
        ),
        partition_gate_exclusive(
            synthetic_steps, {"cross.solo", "cross.dependent", "cross.plain"},
            exclusive_phase=True, keep_all=True,
        ),
        partition_gate_exclusive(
            synthetic_steps, {"cross.solo", "cross.dependent", "cross.plain"},
            exclusive_phase=True, keep_all=True,
            components=frozenset({"agentctl", "herdr-run"}),
        ),
    ]
    for off_main, off_exclusive in disabled_partitions:
        if off_main != ["cross.plain"] or off_exclusive != []:
            failures.append(
                "cross differentials disabled by default (#1249): labelled "
                "steps and their dependents must run nowhere and the "
                f"exclusive phase must be empty; got main={off_main} "
                f"exclusive={off_exclusive}"
            )

    # #1250/#1252: steps labelled disabled-optin (and their dependents)
    # run nowhere by default; the opt-in restores them unchanged. Their
    # own dependencies (shared setup) stay selected either way.
    timing_steps: dict[str, dict[str, object]] = {
        "timing.setup": {"labels": [], "deps": []},
        "timing.solo": {"labels": [DISABLED_OPTIN_LABEL], "deps": ["timing.setup"]},
        "timing.dependent": {"labels": [], "deps": ["timing.solo"]},
        "timing.plain": {"labels": [], "deps": ["timing.setup"]},
    }
    timing_chosen = {"timing.setup", "timing.solo", "timing.dependent", "timing.plain"}
    kept_off, skipped_off = drop_disabled_optin(timing_steps, timing_chosen)
    if kept_off != {"timing.setup", "timing.plain"} or skipped_off != ["timing.solo"]:
        failures.append(
            "disabled-optin by default (#1250/#1252): labelled steps and "
            "their dependents must be dropped and named, shared setup kept; "
            f"got kept={sorted(kept_off)} skipped={skipped_off}"
        )
    kept_on, skipped_on = drop_disabled_optin(
        timing_steps, timing_chosen, include_disabled=True
    )
    if kept_on != timing_chosen or skipped_on != []:
        failures.append(
            "disabled-optin opt-in (#1250/#1252): --include-disabled must "
            f"restore every step; got kept={sorted(kept_on)} skipped={skipped_on}"
        )
    if include_disabled_steps({INCLUDE_DISABLED_ENV: "1"}) is not True or (
        include_disabled_steps({}) is not False
    ) or include_disabled_steps({INCLUDE_DISABLED_ENV: "0"}) is not False:
        failures.append(
            "disabled-optin switch (#1250/#1252): only "
            f"{INCLUDE_DISABLED_ENV}=1 may opt in"
        )

    if cross_in_main_phase({CROSS_MAIN_PHASE_ENV: "1"}) is not True or (
        cross_in_main_phase({}) is not False
    ) or cross_in_main_phase({CROSS_MAIN_PHASE_ENV: "0"}) is not False:
        failures.append(
            "gate-exclusive switch (#1067): only "
            f"{CROSS_MAIN_PHASE_ENV}=1 may opt into the main phase"
        )

    for shared_paths, want_shared in (
        (["cross/dagrun/cli.py"], False),
        (["cross/dagrun/cli.py", "rs/Cargo.toml"], True),
        (["cross/differential.py"], True),
        (["rs/Cargo.toml"], True),
        (["examples/demo.py"], True),
    ):
        shared_selected, _, _, _ = selection_for(shared_paths, manifest=manifest)
        if _cross_shared_trigger(shared_paths, shared_selected) != want_shared:
            failures.append(
                "gate-exclusive selection: shared cross inputs must keep every "
                f"differential while another tool's cross files must not; paths={shared_paths}"
            )

    def expect(
        paths: list[str], want: set[str], why: str, want_components: set[str] | None = None
    ) -> None:
        """`want` is what the PATHS select. Always-on groups are added here and checked below."""
        got, _, components, _ = selection_for(paths, manifest=manifest)
        expected = want | ALWAYS
        if set(got) != expected:
            failures.append(
                f"{why}\n    paths={paths}\n    want={sorted(expected)}\n    got ={sorted(got)}"
            )
        expected_components = want_components or set()
        if set(components) != expected_components:
            failures.append(
                f"{why} (components)\n    paths={paths}\n"
                f"    want={sorted(expected_components)}\n    got ={sorted(components)}"
            )

    expect(["vibe-talk/src/ops.rs"], {VIBE_TALK}, "a vibe-talk change must not drag in the workspace contract")
    expect(["vibe-talk/web/voice.js", "vibe-talk/README.md"], {VIBE_TALK, VIBE_TALK_PAGE}, "a page file plus a vibe-talk doc selects both the page group and the full vibe-talk group")
    expect(["vibe-talk/web/src/present/card.js"], {VIBE_TALK_PAGE}, "a page-only change selects the page group, not the operator and formatting checks")
    expect(["vibe-talk/web/voice.js", "rs/tick-hub/src/lib.rs"], {VIBE_TALK_PAGE, RUST}, "a cross-area change is the union of both areas", {"tick-hub"})
    expect(
        ["vibe-talk/scripts/smoke-agent.py"],
        {VIBE_TALK},
        "vibe-talk's own gate type-checks its Python without selecting unrelated workspaces",
    )
    expect(
        ["rs/tick-hub/src/lib.rs"],
        {RUST},
        "a Rust tool change needs its static gate and component closure",
        {"tick-hub"},
    )
    expect(
        ["py/dagrun/sizing.py"],
        {PYTHON},
        "a Python tool change needs its static gate and reverse dependencies",
        {"dagrun", "experiment-runner"},
    )
    expect(
        ["rs/testrun-core/src/proccpu.rs"],
        {RUST},
        "the library dagrun re-exports re-tests dagrun and everything that depends on it",
        {"dagrun", "experiment-runner"},
    )
    expect(
        ["cross/differential.py"],
        {CROSS},
        "the differential harness also selects its direct environment-regression tests",
        {"repository-infrastructure"},
    )
    expect(
        ["common/docs/herdr-run/README.template.md"],
        {DOCS},
        "embedded docs select their owning component",
        {"herdr-run"},
    )
    expect(
        ["rs/gh-paced/src/wrapper.rs"],
        {RUST},
        "the Rust-only gh-paced tool selects the Rust gate and the crate tests, not everything",
        {"gh-paced"},
    )
    expect(
        ["common/docs/gh-paced/USER_GUIDE.md"],
        {DOCS, RUST},
        "gh-paced compiles its guides into the binary, so a guide edit re-tests the crate",
        {"gh-paced"},
    )
    expect(
        ["rs/agent-usage/src/burn.rs"],
        {RUST},
        "the Rust-only agent-usage tool selects the Rust gate and the crate tests, not everything",
        {"agent-usage"},
    )
    expect(
        ["common/docs/agent-usage/USER_GUIDE.md"],
        {DOCS, RUST},
        "agent-usage compiles its guides into the binary, so a guide edit re-tests the crate",
        {"agent-usage"},
    )
    expect(
        ["rs/agent-teams/src/ledger.rs"],
        {RUST},
        "the Rust-only agent-teams tool selects the Rust gate and the crate tests, not everything",
        {"agent-teams"},
    )
    expect(
        ["common/docs/agent-teams/USER_GUIDE.md"],
        {DOCS, RUST},
        "agent-teams compiles its guides into the binary, so a guide edit re-tests the crate",
        {"agent-teams"},
    )
    expect(
        ["skills/agentctl/SKILL.md"],
        {DOCS},
        "the bundled agentctl skill selects its owning component",
        {"agentctl", "agent-teams"},
    )
    expect(
        ["skills/herdr-run/SKILL.md"],
        set(),
        "the bundled herdr-run skill selects the tests that read it",
        {"herdr-run"},
    )
    expect(
        ["skills/parallel-experiment-runner/SKILL.md"],
        set(),
        "the bundled experiment-runner skill selects the tests that read it",
        {"experiment-runner"},
    )
    expect(
        ["skills/dagrun/SKILL.md", "skills/cpuset-alloc/SKILL.md"],
        set(),
        "bundled dagrun-family skills select their owning component",
        {"dagrun", "experiment-runner"},
    )
    expect(
        ["skills/pr-landing-planner/SKILL.md", "skills/pr-landing-operations/SKILL.md"],
        set(),
        "bundled landing skills select their owning component; the merge queue builds on the planner",
        {"planner", "merge-queue"},
    )
    expect(
        ["rs/merge-queue/src/plan.rs"],
        {RUST},
        "the Rust-only merge-queue tool selects the Rust gate and the crate tests, not everything",
        {"merge-queue"},
    )
    expect(
        ["rs/change-classifier/testdata/vectors.json"],
        {RUST},
        "the shared change classifier re-tests itself and the merge queue that embeds it",
        {"change-classifier", "merge-queue"},
    )
    expect(
        ["common/docs/merge-queue/USER_GUIDE.md"],
        {DOCS, RUST},
        "merge-queue compiles its guides into the binary, so a guide edit re-tests the crate",
        {"merge-queue"},
    )
    expect(
        ["skills/tick-hub/SKILL.md", "skills/wrkslots/SKILL.md", "skills/wrkviz/SKILL.md"],
        set(),
        "remaining bundled tool skills select their owning components",
        {"tick-hub", "wrkslots", "wrkviz"},
    )
    expect(["AGENTS.md"], set(), "the agent guide is embedded nowhere")
    expect(["ai_docs/note.md", "reviews/x.md"], set(), "prose selects nothing")
    expect(
        ["ai_docs/transient/audit.py", "reviews/check.py"],
        {PYTHON},
        "Python audit helpers are inputs to repository-wide strict typing",
    )
    expect(
        ["skills/new-skill/helper.py"],
        {PYTHON},
        "Python code cannot inherit a surrounding prose-only exemption",
    )
    expect(
        ["reviews/types.pyi"],
        {PYTHON},
        "Python typing stubs cannot inherit a surrounding prose-only exemption",
    )
    expect(
        ["common/docs/example.py"],
        {DOCS, PYTHON},
        "a Python file under embedded docs owes both generation and strict typing",
    )
    expect([".minibeads/issues/agent-utils-1.md"], set(), "issue metadata is not executable input")
    expect(
        ["README.md", "RELATED_WORK.md", "common/README.md", "scripts/README.md"],
        set(),
        "repository overview documents are not packaged inputs",
    )
    expect(
        ["py/README.md"],
        {PYTHON_PACKAGES},
        "the Python package index selects only Python artifact checks",
    )
    expect(
        ["rs/README.md"],
        {RUST_PACKAGES},
        "the Rust package index selects only Rust artifact checks",
    )
    expect(
        ["LICENSE"],
        {PACKAGES},
        "the repository license is copied into every published artifact",
    )
    expect(
        ["rs/Cargo.toml", "rs/Cargo.lock"],
        {RUST, RUST_WORKSPACE_TESTS, CROSS, RUST_PACKAGES},
        "Rust workspace manifests and locks affect builds, runtime parity, and package archives",
    )
    expect(
        ["rs/bin/cargo-runner"],
        set(ALL_GROUPS),
        "the shared Rust launcher affects every tool and must select the complete portable contract",
        {"repository-infrastructure"},
    )
    expect(
        ["rustfmt.toml"],
        {RUST, VIBE_TALK},
        "the repository formatting policy selects both Rust workspaces",
    )
    expect(
        ["py/tests/__init__.py", "py/tests/conftest.py"],
        {PYTHON, PYTHON_COMPONENT_TESTS},
        "shared pytest infrastructure selects typing and Python component tests without Rust",
    )
    expect(
        ["py/pyproject.toml"],
        {PYTHON, PYTHON_COMPONENT_TESTS, PYTEST_SHARDS, PYTHON_PACKAGES},
        "the Python workspace manifest affects Python checks, tests, and package archives",
        {"repository-infrastructure"},
    )
    expect(
        ["scripts/run_component_tests.py"],
        {PYTHON, PYTHON_COMPONENT_TESTS},
        "the component runner selects every component test without Rust",
        {"repository-infrastructure"},
    )
    expect(
        ["scripts/run_pytest_shard.py"],
        {PYTHON, PYTEST_SHARDS},
        "the sharder selects exactly its component and lifecycle consumers",
        {"repository-infrastructure"},
    )
    expect(
        ["scripts/pid_namespace_init.py"],
        {PYTHON, PYTHON_NAMESPACE_TESTS},
        "the namespace launcher selects only mapped-namespace lifecycle consumers",
        {"repository-infrastructure"},
    )
    expect(
        ["scripts/check_python_packages.py"],
        {PYTHON, PYTHON_PACKAGES},
        "the Python package checker selects no Rust package gates",
        {"repository-infrastructure"},
    )
    expect(
        ["scripts/check_rust_packages.py"],
        {PYTHON, RUST_PACKAGES},
        "the Rust package checker selects no Python package gates",
        {"repository-infrastructure"},
    )
    expect(
        ["scripts/check_standalone.py"],
        {PYTHON, PYTHON_PACKAGES},
        "the standalone-subset checker selects no Rust package gates",
        {"repository-infrastructure"},
    )
    expect(
        ["scripts/embed_userguides.py"],
        {DOCS, PACKAGES, PYTHON},
        "the docs generator selects docs, artifacts, typing, and its owned tests",
        {"repository-infrastructure"},
    )
    expect(
        ["py/tests/test_tickhub_cli.py"],
        {PYTHON},
        "a test edit selects its owning component rather than every Python tool",
        {"tick-hub"},
    )
    expect(["scripts/something_new.py"], set(ALL_GROUPS), "an unclassified script must select EVERYTHING")
    expect(["brand_new_toplevel/x"], set(ALL_GROUPS), "an unclassified top level must select EVERYTHING")
    expect(["AGENTS.md.generated"], set(ALL_GROUPS), "an exact-file exemption must not bleed into a prefix")
    expect(
        ["scripts/embed_userguides.py.backup"],
        set(ALL_GROUPS),
        "an exact script rule must not exempt a similarly named file",
    )
    expect(["ai_docs_extra/note.md"], set(ALL_GROUPS), "a directory rule must stop at its boundary")
    expect(
        ["scripts/with-node22"],
        {TIMELINE_BROWSER},
        "the pinned Node launcher is an input to the browser contract",
    )
    expect(
        ["scripts/provision-node-packages"],
        {TIMELINE_BROWSER, VIBE_TALK},
        "the npm provisioning step is an input to both suites that install packages",
    )
    expect(
        ["scripts/provision-node22"],
        {TIMELINE_BROWSER},
        "the pinned Node 22 release is an input to the browser contract",
    )
    expect(
        [".github/workflows/cross-dagrun.yml"],
        set(ALL_GROUPS),
        "a workflow edit runs the complete graph and every conditional compatibility lane",
        {"repository-infrastructure"},
    )
    expect(
        [".github/workflows/pages.yml"],
        set(),
        "the Pages deploy workflow runs only the website checker, which hygiene always runs",
        {"repository-infrastructure"},
    )
    expect(
        [".github/workflows/pages.yml.disabled"],
        set(ALL_GROUPS),
        "the Pages exemption names one exact file and must not bleed into other workflows",
        {"repository-infrastructure"},
    )
    expect(
        ["site/index.html", "site/style.css", "site/.nojekyll"],
        set(),
        "website files are checked by the always-on hygiene group and owe nothing else",
    )
    expect(["site.html"], set(ALL_GROUPS), "the site/ directory rule must stop at its boundary")
    expect(
        [".gitmodules", "ext/minibeads"],
        set(),
        "a submodule pin is validated in its own repository and owes only the hygiene group",
    )
    expect(
        ["ext/another-project"],
        set(ALL_GROUPS),
        "a new submodule is unclassified until it is routed on purpose",
    )
    expect(
        ["site/demos/vibe-talk/record_demo.py", "site/demos/vibe-talk/gchat_bridge_mock.py"],
        {PYTHON},
        "the website's demo recorders are inputs to repository-wide strict typing",
    )
    expect(
        [
            "site/demos/vibe-talk/README.md",
            "site/demos/vibe-talk/vibe-talk.toml.template",
            "site/demos/vibe-talk/fonts.conf",
            "site/media/vibe-talk/live-message.gif",
        ],
        set(),
        "demo prose, configuration and published clips are checked by the always-on hygiene group",
    )
    expect(
        ["py/tests/js/browser/new-benchmark.test.cjs"],
        {TIMELINE_BROWSER, PYTHON},
        "a new browser test selects both its runtime and the graph inventory guard",
        {"repository-infrastructure", "wrkviz"},
    )
    expect(
        ["vibe-talk/tests/js/new-page.test.mjs"],
        {VIBE_TALK},
        "a new vibe page suite selects the service and the graph inventory guard",
        {"repository-infrastructure"},
    )
    expect(
        ["vibe-talk/src/ops.rs", "rs/tick-hub/src/lib.rs"],
        {VIBE_TALK, RUST},
        "a change spanning two areas is the UNION, never the smaller of the two",
        {"tick-hub"},
    )
    expect(
        ["scripts/check_client_names.py"],
        {PYTHON},
        "a hygiene implementation change also runs its owned infrastructure tests",
        {"repository-infrastructure"},
    )
    expect(
        ["scripts/check_documented_defaults.py"],
        {PYTHON},
        "a documented-default implementation change also runs its owned tests",
        {"repository-infrastructure"},
    )
    expect(
        ["scripts/check_site.py"],
        {PYTHON},
        "a website-checker change runs strict typing and the infrastructure tests",
        {"repository-infrastructure"},
    )
    expect(
        ["scripts/check_issue_numbers.py"],
        {PYTHON},
        "an issue-number guard change also runs its owned infrastructure tests",
        {"repository-infrastructure"},
    )
    expect(
        ["scripts/check_issue_parse.py"],
        {PYTHON},
        "an issue-parse guard change also runs its owned infrastructure tests",
        {"repository-infrastructure"},
    )
    expect(
        ["scripts/check_no_any.py"],
        {PYTHON, CROSS, VIBE_TALK},
        "the no-Any checker selects every validation mode that invokes it",
        {"repository-infrastructure"},
    )
    expect([], set(), "no changes selects nothing by path")

    # ALWAYS is the property the table above cannot state, because it folds it in silently.
    for paths, why in (
        ([], "no changes at all"),
        (["ai_docs/note.md"], "a prose path that selects nothing else"),
        (["AGENTS.md"], "a file mapped to the empty set"),
        (["rs/tick-hub/src/lib.rs"], "an ordinary source change"),
    ):
        got, because = groups_for(paths, manifest=manifest)
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

    # Unknown paths run everything at execution time, but repository files should not remain
    # permanently unknown: that would turn the fail-safe fallback into an undocumented routing
    # policy and make selective-validation claims impossible to audit.
    inventory = subprocess.run(
        ("git", "ls-files", "--cached", "--others", "--exclude-standard"),
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    if inventory.returncode != 0:
        failures.append(f"cannot inventory repository paths: {inventory.stderr.strip()}")
    else:
        unclassified: list[str] = []
        for path in sorted(line for line in inventory.stdout.splitlines() if line):
            _selected, path_because, _components, _component_because = selection_for(
                [path], manifest=manifest
            )
            if any("unclassified path" in reason for reason in path_because.values()):
                unclassified.append(path)
        if unclassified:
            failures.append(
                "every repository path must have explicit validation ownership; "
                f"unclassified={unclassified}"
            )

    # A rename is both a deletion from the old validation boundary and an addition to the new
    # one. Git's default --name-only output collapses it to the destination and can thereby skip
    # the source's tests, so exercise the real Git behavior rather than merely inspecting argv.
    with tempfile.TemporaryDirectory(prefix="agent-utils-validate-rename-") as raw:
        repository = Path(raw)
        (repository / "old-component").mkdir()
        old_path = repository / "old-component" / "source.py"
        old_path.write_text("before\n", encoding="utf-8")
        setup_commands = (
            ("git", "init", "-q"),
            ("git", "add", "old-component/source.py"),
            (
                "git",
                "-c",
                "user.name=validation self-test",
                "-c",
                "user.email=validation-self-test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "-qm",
                "fixture",
            ),
        )
        for command in setup_commands:
            done = subprocess.run(command, cwd=repository, capture_output=True, text=True, check=False)
            if done.returncode != 0:
                failures.append(
                    f"rename-safety fixture setup failed: {' '.join(command)}: {done.stderr.strip()}"
                )
                break
        else:
            (repository / "new-component").mkdir()
            moved = subprocess.run(
                ("git", "mv", "old-component/source.py", "new-component/source.py"),
                cwd=repository,
                capture_output=True,
                text=True,
                check=False,
            )
            if moved.returncode != 0:
                failures.append(f"rename-safety fixture move failed: {moved.stderr.strip()}")
            else:
                committed = subprocess.run(
                    (
                        "git",
                        "-c",
                        "user.name=validation self-test",
                        "-c",
                        "user.email=validation-self-test@example.invalid",
                        "-c",
                        "commit.gpgsign=false",
                        "-c",
                        "core.hooksPath=/dev/null",
                        "commit",
                        "-qm",
                        "rename fixture",
                    ),
                    cwd=repository,
                    capture_output=True,
                    text=True,
                    check=False,
                )
                if committed.returncode != 0:
                    failures.append(
                        f"rename-safety fixture commit failed: {committed.stderr.strip()}"
                    )
                else:
                    expected = ["new-component/source.py", "old-component/source.py"]
                    committed_paths = changed_paths("HEAD~1", repo_root=repository)
                    if committed_paths != expected:
                        failures.append(
                            "changed_paths must retain both sides of a committed rename\n"
                            f"    want={expected}\n    got ={committed_paths}"
                        )

    good = parse_host_env(
        "# host\n\nAGENT_UTILS_NPM = /usr/bin/npm\nnpm_config_registry=https://r.example/\n", "t"
    )
    if good != {"AGENT_UTILS_NPM": "/usr/bin/npm", "npm_config_registry": "https://r.example/"}:
        failures.append(f"validate.env: a valid file parsed as {good!r}")
    for bad, why in (
        ("PATH=/tmp/evil\n", "an unknown key"),
        ("AGENT_UTILS_NPM\n", "a line without '='"),
        ("AGENT_UTILS_NPM=a\nAGENT_UTILS_NPM=b\n", "a repeated key"),
        ("npm_config_registry=\n", "an empty value"),
    ):
        try:
            parse_host_env(bad, "t")
        except HostEnvError:
            continue
        failures.append(f"validate.env: {why} was accepted instead of refused")
    if host_env_path({"XDG_CONFIG_HOME": "/x", "HOME": "/h"}) != Path("/x/agent-utils/validate.env"):
        failures.append("validate.env: XDG_CONFIG_HOME was not honoured")
    if host_env_path({"XDG_CONFIG_HOME": "", "HOME": "/h"}) != Path("/h/.config/agent-utils/validate.env"):
        failures.append("validate.env: an empty XDG_CONFIG_HOME did not fall back to ~/.config")
    with tempfile.TemporaryDirectory(prefix="validate-host-env-") as config:
        if host_env({"XDG_CONFIG_HOME": config}) != {}:
            failures.append("validate.env: a missing file added settings")
        (Path(config) / "agent-utils").mkdir()
        (Path(config) / "agent-utils" / "validate.env").write_text(
            "AGENT_UTILS_NPM=/opt/npm\nnpm_config_registry=https://r.example/\n", encoding="utf-8"
        )
        added = host_env({"XDG_CONFIG_HOME": config, "AGENT_UTILS_NPM": "/caller/npm"})
        if added != {"npm_config_registry": "https://r.example/"}:
            failures.append(f"validate.env: the caller's own setting did not win: {added!r}")

    for failure in failures:
        print(f"FAIL  {failure}", file=sys.stderr)
    if failures:
        print(f"\n{len(failures)} self-test failure(s)", file=sys.stderr)
        return 1
    print("validate --self-test: PASSED")
    return 0


#: Local-pass record for the gate's local preflight (#1262
#: gate-local-preflight): a successful real run writes one JSON file per
#: working-tree hash in this directory, which `gate.py` checks before
#: queueing a full gate. Override with $AGENT_UTILS_LOCAL_PREFLIGHT_DIR.
LOCAL_PREFLIGHT_DIR_ENV = "AGENT_UTILS_LOCAL_PREFLIGHT_DIR"


def _working_tree_hash(root: Path) -> str | None:
    """Hash of the working tree (staged, unstaged, and untracked), or None.

    Computed with a throwaway index, so the caller's real index is never
    touched. `gate.py` computes the identical hash for the identical tree."""
    try:
        with tempfile.TemporaryDirectory(prefix="validate-tree-") as tmp:
            env = {**os.environ, "GIT_INDEX_FILE": str(Path(tmp) / "index")}
            def git(*argv: str) -> str:
                done = subprocess.run(
                    ["git", "-C", str(root), *argv],
                    capture_output=True, text=True, env=env, check=False,
                )
                if done.returncode != 0:
                    raise OSError(done.stderr.strip())
                return done.stdout.strip()
            git("read-tree", "HEAD")
            git("add", "-A")
            return git("write-tree")
    except Exception:
        return None


def record_local_validate_pass(root: Path | None = None) -> None:
    """Record this successful run as a local pass for its tree (#1262).

    Best effort: a record failure must never change the run's result."""
    try:
        root = root or REPO_ROOT
        tree = _working_tree_hash(root)
        if tree is None:
            return
        override = os.environ.get(LOCAL_PREFLIGHT_DIR_ENV)
        directory = (
            Path(override) if override
            else Path("~/.local/state/agent-utils/local-validate-passes").expanduser()
        )
        directory.mkdir(parents=True, exist_ok=True)
        (directory / f"{tree}.json").write_text(json.dumps({
            "tree": tree, "finished_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        }))
    except Exception:
        pass


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--base", default="origin/main", help="compare against this ref (default: origin/main)")
    parser.add_argument(
        "--all",
        action="store_true",
        help="run the entire portable, current-toolchain contract, selecting nothing "
        "(the gate-exclusive cross steps are still path-filtered, #1190; see --cross-all)",
    )
    parser.add_argument(
        "--cross-all",
        action="store_true",
        help="keep every gate-exclusive cross differential regardless of path selection "
        "(#1190), except the five disabled by default (#1249 cross-differentials-robust): "
        "with --all it is what release/nightly runs use (`make validate-all`); without --all "
        "it adds the kept steps on top of path selection. It never selects the disabled "
        "differentials on its own. Example: python3 scripts/validate.py --all --cross-all",
    )
    parser.add_argument(
        "--cross-differentials",
        action="store_true",
        help="explicit opt-in: run the five gate-exclusive cross differentials on demand "
        "(#1249 cross-differentials-robust). They are disabled by default (owner reply 346, "
        "2026-11-10) and are selected by no default, --all, or --cross-all run. With this "
        "flag they run in the exclusive phase, or in the main phase when "
        "AGENT_UTILS_CROSS_IN_MAIN_PHASE=1. Example: python3 scripts/validate.py "
        "--cross-differentials --all",
    )
    parser.add_argument(
        "--include-disabled",
        action="store_true",
        help="explicit opt-in: run the real-time-sensitive steps labelled disabled-optin on "
        "demand (#1250 vibe-talk-timing-robust, #1252 timeline-benchmark-optin). They are "
        "disabled by default (owner reply 346, 2026-11-10) and are selected by no default "
        "or --all run; AGENT_UTILS_INCLUDE_DISABLED=1 is the same opt-in. Example: "
        "python3 scripts/validate.py --include-disabled --all",
    )
    parser.add_argument("--list", action="store_true", help="print the plan and exit without running")
    parser.add_argument(
        "--keep-going",
        action="store_true",
        help="after a failed step, keep running every step that does not depend on it (dagrun "
        "--keep-going), so one run reports every independent failure; default: stop at the "
        "first failure, for fast feedback while editing",
    )
    parser.add_argument("--self-test", action="store_true", help="check the mapping offline, then exit")
    args = parser.parse_args(argv)

    if args.self_test:
        return self_test()

    selected: frozenset[str]
    because: dict[str, str]
    components: frozenset[str]
    component_because: dict[str, str]
    paths: list[str]
    if args.all:
        selected, because, components, component_because, paths = (
            ALL_GROUPS,
            {g: "--all" for g in ALL_GROUPS},
            frozenset(),
            {},
            [],
        )
        print("validate: --all, running the entire portable, current-toolchain contract")
    else:
        paths = changed_paths(args.base)
        selected, because, components, component_because = selection_for(paths)
        report(selected, because, components, component_because, paths)

    all_contract = args.all or selected == ALL_GROUPS
    graph_steps, graph_chosen = selected_graph(
        selected, components, all_contract=all_contract
    )
    # Steps labelled `disabled-optin` are disabled by default in every
    # mode (owner reply 346; see DISABLED_OPTIN_LABEL): drop them (and
    # their dependents) before the gate-exclusive partition, and name
    # them in the plan so the skip is never silent. Only the explicit
    # opt-in runs them.
    graph_chosen, disabled_skipped = drop_disabled_optin(
        graph_steps,
        graph_chosen,
        include_disabled=args.include_disabled or include_disabled_steps(),
    )
    if disabled_skipped:
        print(
            f"validate: {DISABLED_OPTIN_NOTE}; skipped "
            f"[{', '.join(disabled_skipped)}]",
            flush=True,
        )
    # The gate-exclusive steps are component-selected (see
    # partition_gate_exclusive), so a cross-group trigger from another tool's
    # files under cross/ does not select them. A cross-group trigger from
    # outside cross/ — workspace manifests, shared launchers, examples — can
    # change every paired implementation at once, so it keeps them all, as
    # does the unclassified-path fail-safe (which selects every group). The
    # shared differential runner (cross/differential.py, owned by
    # repository-infrastructure in the map) is named explicitly for the same
    # reason.
    #
    # Since #1190, --all path-filters these steps too: the main phase still
    # runs the entire contract, but a gate-exclusive step whose component no
    # changed path (against --base) selects is skipped and named below,
    # fail-closed per cross_filter_for_all. Since #1249 the five are disabled
    # by default in either mode; only --cross-differentials runs them.
    cross_components = components
    cross_filtered = False
    cross_note: str | None = None
    if args.all and not args.cross_all:
        try:
            all_paths = changed_paths(args.base)
        except SystemExit as exc:  # undeterminable paths: fail closed
            cross_note = f"changed paths unavailable ({exc.code}) — keeping all"
        else:
            filtered_components, cross_note = cross_filter_for_all(all_paths)
            if filtered_components is not None:
                cross_components = filtered_components
                cross_filtered = True
    cross_shared_trigger = _cross_shared_trigger(paths, selected)
    if (
        (cross_shared_trigger or args.cross_all or args.cross_differentials)
        and not all_contract
    ):
        labelled: set[str] = set()
        for tag, step in graph_steps.items():
            raw_labels = step.get("labels")
            if isinstance(raw_labels, list) and GATE_EXCLUSIVE_LABEL in {
                label for label in raw_labels if isinstance(label, str)
            }:
                labelled.add(tag)
        graph_chosen = graph_chosen | labelled
    phases = partition_gate_exclusive(
        graph_steps,
        graph_chosen,
        components=cross_components,
        keep_all=args.cross_differentials
        or args.cross_all
        or cross_shared_trigger
        or (all_contract and not cross_filtered),
        differentials_enabled=args.cross_differentials,
    )
    if not args.cross_differentials:
        print(f"validate: {CROSS_DIFFERENTIALS_DISABLED_NOTE}", flush=True)
    final_chosen = set(phases[0]) | set(phases[1])
    if args.all:
        labelled_all = []
        for tag in sorted(graph_chosen):
            raw_labels = graph_steps[tag].get("labels")
            if isinstance(raw_labels, list) and GATE_EXCLUSIVE_LABEL in {
                label for label in raw_labels if isinstance(label, str)
            }:
                labelled_all.append(tag)
        kept = [tag for tag in labelled_all if tag in final_chosen]
        skipped = [tag for tag in labelled_all if tag not in final_chosen]
        if not args.cross_differentials:
            print(
                f"validate: --all cross filter (#1190 cross-path-filter): keeping "
                f"{len(kept)} of {len(labelled_all)} gate-exclusive step(s)"
                + (
                    f"; skipped [{', '.join(skipped)}] — disabled by default "
                    "(#1249 cross-differentials-robust; "
                    "--cross-differentials runs them on demand)"
                    if skipped
                    else "; skipped none"
                ),
                flush=True,
            )
        elif args.cross_all:
            print(
                f"validate: --cross-all, keeping all {len(kept)} gate-exclusive "
                "step(s) (#1190 cross-path-filter)",
                flush=True,
            )
        elif cross_filtered:
            print(
                f"validate: --all cross filter (#1190 cross-path-filter): keeping "
                f"{len(kept)} of {len(labelled_all)} gate-exclusive step(s)"
                + (f" [{', '.join(kept)}]" if kept else "")
                + (
                    f"; skipped [{', '.join(skipped)}] — no changed path (vs {args.base}) "
                    "selects their component; --cross-differentials runs them on demand"
                    if skipped
                    else "; skipped none"
                ),
                flush=True,
            )
        else:
            print(
                f"validate: --all cross filter (#1190 cross-path-filter): keeping all "
                f"{len(kept)} gate-exclusive step(s) — {cross_note}",
                flush=True,
            )
    if cross_in_main_phase():
        print(
            f"validate: {CROSS_MAIN_PHASE_ENV}=1, gate-exclusive differentials "
            "run in the main phase (#1067 cross-differential-load-aware)",
            flush=True,
        )
    if args.list:
        report_selected_graph(
            selected,
            components,
            all_contract=all_contract,
            chosen_tags=final_chosen,
        )
        return 0
    try:
        host = host_env(os.environ)
    except HostEnvError as error:
        print(f"validate: {error}", file=sys.stderr)
        return 2
    # Every selected cross node must leave a record; one that does not cannot be confirmed.
    expected = selected_cross_nodes(
        selected, components, all_contract=all_contract, chosen_tags=final_chosen
    )
    expected_timing = selected_timing_gates(
        selected, components, all_contract=all_contract, chosen_tags=final_chosen
    )
    with (
        tempfile.TemporaryDirectory(prefix="validate-cross-coverage-") as coverage,
        tempfile.TemporaryDirectory(prefix="validate-case-timings-") as timings,
        pytest_temp_root() as pytest_root,
    ):
        code = run(
            selected,
            components,
            all_contract=all_contract,
            coverage_dir=Path(coverage),
            timing_dir=Path(timings),
            host=host,
            keep_going=args.keep_going,
            gate_exclusive_phases=phases,
            pytest_root=pytest_root,
        )
        if code != 0:
            return code
        coverage_state = cross_coverage_report(Path(coverage), expected)
        timing_lines, timing_problems, timing_missing = case_timing_report(
            Path(timings), expected_timing
        )
    print()
    for line in timing_lines:
        print(line)
    if timing_problems:
        for problem in timing_problems[:50]:
            print(f"validate: case timings: {problem}", file=sys.stderr)
        if len(timing_problems) > 50:
            print(
                f"validate: case timings: ... and {len(timing_problems) - 50} more",
                file=sys.stderr,
            )
        print(
            "\nvalidate: FAILED - per-case timing records do not match their "
            "collected inventories",
            file=sys.stderr,
        )
        return 1
    if timing_missing:
        for gate in timing_missing:
            print(
                f"  {gate}: no primary timing record, so its per-case coverage "
                "could not be confirmed"
            )
    if (
        coverage_state.skipped
        or coverage_state.unreadable
        or coverage_state.missing
        or timing_missing
    ):
        # Every selected node passed, but a cross node that declared itself partial passes with
        # checks that never ran. Say so here, where "OK" would otherwise be the last word.
        unconfirmed = coverage_state.unreadable or coverage_state.missing or timing_missing
        state = "PARTIAL or could not be confirmed" if unconfirmed else "PARTIAL"
        print(f"\nvalidate: cross-language coverage was {state}:")
        for line in coverage_state.lines:
            print(line)
        reasons: list[str] = []
        if coverage_state.skipped:
            reasons.append(
                f"{coverage_state.skipped} cross check(s) were skipped and are UNVERIFIED "
                f"({_kind_counts(dict(coverage_state.by_kind))})"
            )
        if coverage_state.unreadable:
            reasons.append(f"{coverage_state.unreadable} coverage record(s) could not be read")
        if coverage_state.missing:
            reasons.append(
                f"{len(coverage_state.missing)} selected cross node(s) left no readable "
                "coverage record"
            )
        if timing_missing:
            reasons.append(
                f"{len(timing_missing)} selected timing gate(s) left no primary "
                "timing record"
            )
        verdict = f"validate: PARTIAL - every selected node passed, but {'; '.join(reasons)}"
        announce_partial(verdict, coverage_state.lines, os.environ)
        print(f"\n{verdict}")
        record_local_validate_pass()
        return 0
    print("\nvalidate: OK")
    record_local_validate_pass()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
