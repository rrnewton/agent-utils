#!/usr/bin/env python3
"""Keep testrun-core's resolved dependency tree to its small, fixed set.

testrun-core is vendored byte-for-byte by consumers whose build systems reject YAML
parsers, extra build scripts and procedural macros. This check resolves the crate's
normal and build dependency closure exactly as ``cargo build -p testrun-core`` would
(``cargo tree``), reads each package's targets (``cargo metadata``), and fails if:

- any package in the closure is outside ``ALLOWED`` (a new crate needs a decision here);
- any package name mentions YAML;
- any package is a procedural macro other than ``serde_derive``;
- testrun-core itself has a build script, a procedural macro target, or any feature;
- any package other than the base crates in ``BUILD_SCRIPT_ALLOWED`` has a build script.

"No build script" means none in testrun-core itself and none beyond the base crates
the crate is allowed to use: serde, serde_core, serde_json, libc and zmij each ship one,
so a closure with no build script at all is impossible with those dependencies.

``--self-test`` checks the rules against synthetic trees without running Cargo.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
MANIFEST = REPO_ROOT / "rs" / "Cargo.toml"
SUBJECT = "testrun-core"
# serde without its derive feature, serde_json with default features, libc, and what
# those pull in. serde_derive is the one procedural macro a consumer accepts, but it
# is not used today; enabling it would also pull its own compile-time dependencies.
ALLOWED = frozenset({SUBJECT, "serde", "serde_core", "serde_json", "libc", "itoa", "memchr", "zmij"})
# These base crates ship build scripts of their own; consumers already build them.
BUILD_SCRIPT_ALLOWED = frozenset({"serde", "serde_core", "serde_json", "libc", "zmij"})
PROC_MACRO_ALLOWED = frozenset({"serde_derive"})
_TREE_LINE = re.compile(r"^(?P<name>[A-Za-z0-9_-]+) v(?P<version>\S+)(?P<rest>.*)$")


@dataclass(frozen=True)
class Package:
    """One resolved package: its name, version and the target kinds it builds."""

    name: str
    version: str
    kinds: frozenset[str]
    features: frozenset[str] = frozenset()


def violations(packages: list[Package]) -> list[str]:
    """Return every rule the resolved closure breaks; empty means the tree is acceptable."""

    errors: list[str] = []
    names = {package.name for package in packages}
    if SUBJECT not in names:
        errors.append(f"{SUBJECT} is missing from its own dependency tree")
    for package in sorted(packages, key=lambda item: (item.name, item.version)):
        label = f"{package.name} v{package.version}"
        if "yaml" in package.name.lower():
            errors.append(f"{label}: YAML crates are not allowed")
        if "proc-macro" in package.kinds and package.name not in PROC_MACRO_ALLOWED:
            errors.append(f"{label}: procedural macros other than serde_derive are not allowed")
        if package.name not in ALLOWED:
            errors.append(f"{label}: not in the allowed dependency set {sorted(ALLOWED)}")
        if "custom-build" in package.kinds and package.name not in BUILD_SCRIPT_ALLOWED:
            errors.append(
                f"{label}: build scripts are not allowed (only testrun-core's allowed base crates "
                f"{sorted(BUILD_SCRIPT_ALLOWED)} may have one, and testrun-core itself may not)"
            )
        if package.name == SUBJECT and package.features:
            errors.append(f"{label}: declares features {sorted(package.features)}; it must have none")
    return errors


def _run(argv: list[str]) -> str:
    result = subprocess.run(argv, cwd=REPO_ROOT, text=True, capture_output=True, check=False)
    if result.returncode != 0:
        raise SystemExit(
            f"check-testrun-core-deps: command failed ({result.returncode}): {' '.join(argv)}\n"
            f"{result.stderr}"
        )
    return result.stdout


def resolved_packages() -> list[Package]:
    """Resolve the closure with cargo tree and attach target kinds from cargo metadata."""

    tree = _run([
        "cargo", "tree", "--manifest-path", str(MANIFEST), "--offline", "--locked",
        "-p", SUBJECT, "-e", "normal,build", "--prefix", "none", "--format", "{p}",
    ])
    wanted: set[tuple[str, str]] = set()
    for line in tree.splitlines():
        line = line.strip()
        if not line:
            continue
        match = _TREE_LINE.match(line)
        if match is None:
            raise SystemExit(f"check-testrun-core-deps: cannot parse cargo tree line {line!r}")
        wanted.add((match["name"], match["version"]))
    payload: object = json.loads(_run([
        "cargo", "metadata", "--manifest-path", str(MANIFEST), "--offline", "--locked",
        "--format-version", "1",
    ]))
    if not isinstance(payload, dict) or not isinstance(payload.get("packages"), list):
        raise SystemExit("check-testrun-core-deps: cargo metadata has no package list")
    found: dict[tuple[str, str], Package] = {}
    for raw in payload["packages"]:
        if not isinstance(raw, dict):
            continue
        key = (str(raw.get("name")), str(raw.get("version")))
        if key not in wanted:
            continue
        kinds: set[str] = set()
        for target in raw.get("targets", []):
            if isinstance(target, dict):
                kinds.update(str(kind) for kind in target.get("kind", []))
        features = raw.get("features", {})
        found[key] = Package(
            key[0], key[1], frozenset(kinds),
            frozenset(features) if isinstance(features, dict) else frozenset(),
        )
    missing = sorted(wanted - set(found))
    if missing:
        raise SystemExit(f"check-testrun-core-deps: cargo metadata lacks {missing}")
    return list(found.values())


def self_test() -> None:
    """Prove each rule rejects what it should and accepts the expected tree."""

    def pkg(name: str, *kinds: str, features: tuple[str, ...] = ()) -> Package:
        return Package(name, "1.0.0", frozenset(kinds or ("lib",)), frozenset(features))

    good = [
        pkg(SUBJECT), pkg("serde", "lib", "custom-build"), pkg("serde_core", "lib", "custom-build"),
        pkg("serde_json", "lib", "custom-build"), pkg("libc", "lib", "custom-build"),
        pkg("itoa"), pkg("memchr"), pkg("zmij", "lib", "custom-build"),
    ]
    assert violations(good) == [], violations(good)
    cases: list[tuple[list[Package], str]] = [
        ([*good, pkg("serde_norway")], "not in the allowed"),
        ([*good, pkg("unsafe-libyaml-norway")], "YAML crates are not allowed"),
        ([*good, pkg("serde_yaml")], "YAML crates are not allowed"),
        ([*good, pkg("thiserror-impl", "proc-macro")], "procedural macros other than serde_derive"),
        ([pkg(SUBJECT, "lib", "custom-build"), *good[1:]], "build scripts are not allowed"),
        ([*good, pkg("memchr", "lib", "custom-build")], "build scripts are not allowed"),
        ([pkg(SUBJECT, features=("default",)), *good[1:]], "it must have none"),
        (good[1:], "missing from its own dependency tree"),
    ]
    for packages, expected in cases:
        found = violations(packages)
        assert any(expected in error for error in found), (expected, found)
    # serde_derive passes the procedural-macro rule but still needs an allowlist decision.
    derive = violations([*good, pkg("serde_derive", "proc-macro")])
    assert derive and all("procedural macros" not in error for error in derive), derive
    print(f"check-testrun-core-deps: self-test ok ({len(cases) + 2} cases)")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--self-test", action="store_true", help="check the rules on synthetic trees only")
    args = parser.parse_args(argv)
    if args.self_test:
        self_test()
        return 0
    packages = resolved_packages()
    errors = violations(packages)
    if errors:
        for error in errors:
            print(f"check-testrun-core-deps: {error}", file=sys.stderr)
        print(
            "check-testrun-core-deps: the rule: testrun-core has no build script; its closure is "
            f"exactly {sorted(ALLOWED)}; no crate named like YAML; no proc-macro but serde_derive; "
            f"build scripts only in {sorted(BUILD_SCRIPT_ALLOWED)}, which ship their own.",
            file=sys.stderr,
        )
        return 1
    names = ", ".join(sorted(f"{package.name} v{package.version}" for package in packages))
    print(f"check-testrun-core-deps: ok - {len(packages)} packages: {names}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
