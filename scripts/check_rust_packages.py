#!/usr/bin/env python3
"""Build and inspect every independently publishable Rust crate.

The repository workspace is convenient for development, but it can hide a
crate that would be incomplete when uploaded by itself.  This check asks Cargo
to create the exact registry archive for each public crate, then inspects that
archive for its binaries, library, license, README, and embedded user guide.
It also keeps installer-facing documentation specific to the Rust package.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
import tarfile
import tempfile
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
RS_ROOT = REPO_ROOT / "rs"


@dataclass(frozen=True)
class Crate:
    name: str
    bins: tuple[str, ...]
    library: str


CRATES: tuple[Crate, ...] = (
    Crate("safe-ci-dag-runner", ("safe-ci-dag-runner", "cpuset-alloc"), "safe_ci_dag_runner"),
    Crate("tick-hub", ("tick-hub",), "tick_hub"),
    Crate("pr-landing-planner", ("pr-landing-planner",), "pr_landing_planner"),
)

_FOREIGN_DOC_TERMS = re.compile(
    r"\bpython(?:[0-9]+(?:\.[0-9]+)*)?\b|\b(?:pip|pypi)\b|(?:^|[ (`])py/",
    re.IGNORECASE | re.MULTILINE,
)
_COMMON_DOC_TERMS: tuple[tuple[str, re.Pattern[str]], ...] = (
    ("suite name", re.compile(r"agent-utils", re.IGNORECASE)),
    ("unrelated project", re.compile(r"\b(?:DeepScry|Hermit)\b", re.IGNORECASE)),
    ("source-tree docs path", re.compile(r"common/docs/", re.IGNORECASE)),
    ("source-tree script", re.compile(r"scripts/embed_userguides\.py", re.IGNORECASE)),
    (
        "source-tree path",
        re.compile(r"(?:^|[ (`])(?:py|rs|scripts|cross)/", re.IGNORECASE | re.MULTILINE),
    ),
    ("unexpanded template syntax", re.compile(r"\{\{|\}\}")),
)


class CheckError(RuntimeError):
    """A crate failed its publishable-artifact contract."""


def _check_public_rustdoc() -> None:
    """Keep every published crate's public rustdoc standalone."""

    for crate in CRATES:
        root = RS_ROOT / crate.name
        for path in sorted((root / "src").rglob("*.rs")):
            in_block_doc = False
            for line_number, line in enumerate(
                path.read_text(encoding="utf-8").splitlines(), start=1
            ):
                stripped = line.lstrip()
                line_doc = stripped.startswith(("//!", "///"))
                if stripped.startswith(("/*!", "/**")):
                    in_block_doc = True
                if line_doc or in_block_doc:
                    errors = _doc_violations(crate, stripped)
                    if errors:
                        raise CheckError(
                            f"{crate.name}: rustdoc {path.relative_to(root)}:{line_number} "
                            f"is not standalone: {'; '.join(errors)}"
                        )
                if in_block_doc and "*/" in stripped:
                    in_block_doc = False


def _doc_violations(crate: Crate, text: str) -> list[str]:
    errors: list[str] = []
    foreign = _FOREIGN_DOC_TERMS.search(text)
    if foreign is not None:
        errors.append(f"foreign-package term {foreign.group(0)!r}")
    for description, pattern in _COMMON_DOC_TERMS:
        match = pattern.search(text)
        if match is not None:
            errors.append(f"{description} {match.group(0)!r}")
    for sibling in CRATES:
        if sibling == crate:
            continue
        for name in (sibling.name, sibling.library):
            match = re.search(re.escape(name), text, re.IGNORECASE)
            if match is not None:
                errors.append(f"sibling package {match.group(0)!r}")
                break
    return errors


def _run(
    argv: list[str],
    *,
    cwd: Path = REPO_ROOT,
    env: dict[str, str] | None = None,
    timeout: int = 300,
) -> subprocess.CompletedProcess[str]:
    try:
        result = subprocess.run(
            argv,
            cwd=cwd,
            env=env,
            text=True,
            capture_output=True,
            check=False,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired as exc:
        raise CheckError(f"command timed out after {timeout}s: {' '.join(argv)}") from exc
    if result.returncode != 0:
        raise CheckError(
            f"command failed ({result.returncode}): {' '.join(argv)}\n"
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}"
        )
    return result


def _metadata(crate: Crate) -> tuple[str, set[str], set[str]]:
    manifest = RS_ROOT / crate.name / "Cargo.toml"
    result = _run(
        [
            "cargo",
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--manifest-path",
            str(manifest),
        ]
    )
    payload: object = json.loads(result.stdout)
    if not isinstance(payload, dict):
        raise CheckError(f"{crate.name}: cargo metadata root is not an object")
    packages = payload.get("packages")
    if not isinstance(packages, list):
        raise CheckError(f"{crate.name}: cargo metadata has no package list")
    wanted_manifest = str(manifest.resolve())
    matches = [
        package
        for package in packages
        if isinstance(package, dict) and package.get("manifest_path") == wanted_manifest
    ]
    if len(matches) != 1:
        raise CheckError(f"{crate.name}: expected metadata for exactly one matching package")
    package = matches[0]
    if package.get("name") != crate.name:
        raise CheckError(f"{crate.name}: package metadata has name {package.get('name')!r}")
    version = package.get("version")
    if not isinstance(version, str) or not version:
        raise CheckError(f"{crate.name}: package metadata has no version")
    targets = package.get("targets")
    if not isinstance(targets, list):
        raise CheckError(f"{crate.name}: package metadata has no targets")
    bins: set[str] = set()
    libraries: set[str] = set()
    for target in targets:
        if not isinstance(target, dict):
            continue
        name = target.get("name")
        kinds = target.get("kind")
        if not isinstance(name, str) or not isinstance(kinds, list):
            continue
        if "bin" in kinds:
            bins.add(name)
        if "lib" in kinds:
            libraries.add(name)
    return version, bins, libraries


def _package(crate: Crate, version: str, target_root: Path) -> Path:
    manifest = RS_ROOT / crate.name / "Cargo.toml"
    _run(
        [
            "cargo",
            "package",
            "--allow-dirty",
            "--locked",
            "--offline",
            "--target-dir",
            str(target_root),
            "--manifest-path",
            str(manifest),
        ]
    )
    archive = target_root / "package" / f"{crate.name}-{version}.crate"
    if not archive.is_file():
        raise CheckError(f"{crate.name}: Cargo reported success but {archive} is missing")
    return archive


def _inspect(crate: Crate, version: str, archive: Path) -> str:
    prefix = f"{crate.name}-{version}/"
    with tarfile.open(archive, mode="r:gz") as package:
        names = set(package.getnames())
        required = {
            f"{prefix}Cargo.toml",
            f"{prefix}Cargo.toml.orig",
            f"{prefix}LICENSE",
            f"{prefix}README.md",
            f"{prefix}src/embedded_userguide.md",
        }
        missing = sorted(required - names)
        if missing:
            raise CheckError(f"{crate.name}: registry archive is missing {missing}")

        manifest_member = package.extractfile(f"{prefix}Cargo.toml.orig")
        if manifest_member is None:
            raise CheckError(f"{crate.name}: could not read Cargo.toml.orig")
        manifest_text = manifest_member.read().decode("utf-8")
        manifest_foreign = _FOREIGN_DOC_TERMS.search(manifest_text)
        if manifest_foreign is not None:
            raise CheckError(
                f"{crate.name}: Cargo.toml contains foreign-package term "
                f"{manifest_foreign.group(0)!r}"
            )

        documents: dict[str, str] = {}
        for relative in ("README.md", "src/embedded_userguide.md"):
            member = package.extractfile(f"{prefix}{relative}")
            if member is None:
                raise CheckError(f"{crate.name}: could not read {relative} from registry archive")
            text = member.read().decode("utf-8")
            source_text = (RS_ROOT / crate.name / relative).read_text(encoding="utf-8")
            if text != source_text:
                raise CheckError(
                    f"{crate.name}: registry {relative} differs from its generated source"
                )
            errors = _doc_violations(crate, text)
            if errors:
                raise CheckError(
                    f"{crate.name}: {relative} is not standalone: {'; '.join(errors)}"
                )
            documents[relative] = text
        return documents["src/embedded_userguide.md"]


def _smoke(crate: Crate, version: str, userguide: str, target_root: Path) -> None:
    bindir = target_root / "debug"
    suffix = ".exe" if os.name == "nt" else ""
    env = dict(os.environ)
    env["NO_COLOR"] = "1"
    run_root = target_root / "smoke-cwd"
    run_root.mkdir()

    for binary in crate.bins:
        executable = bindir / f"{binary}{suffix}"
        if not executable.is_file():
            raise CheckError(f"{crate.name}: verified binary is missing: {executable}")
        invocations = [("--help",)]
        if binary != "cpuset-alloc":
            invocations.extend([("--version",), ("--userguide",)])
        for args in invocations:
            result = _run(
                [str(executable), *args],
                cwd=run_root,
                env=env,
                timeout=30,
            )
            combined = result.stdout + result.stderr
            if args == ("--version",) and version not in combined:
                raise CheckError(
                    f"{binary} --version did not report {version!r}: {combined!r}"
                )
            if args == ("--userguide",) and (result.stdout != userguide or result.stderr):
                raise CheckError(f"{binary} --userguide differs from packaged user guide")


def main() -> int:
    try:
        _check_public_rustdoc()
        index_text = (RS_ROOT / "README.md").read_text(encoding="utf-8")
        index_match = _FOREIGN_DOC_TERMS.search(index_text)
        if index_match is not None:
            raise CheckError(
                "rs/README.md contains foreign-package term "
                f"{index_match.group(0)!r}"
            )
        with tempfile.TemporaryDirectory(prefix="agent-utils-rust-packages-") as raw:
            package_root = Path(raw)
            for crate in CRATES:
                version, bins, libraries = _metadata(crate)
                if bins != set(crate.bins):
                    raise CheckError(
                        f"{crate.name}: binary targets {sorted(bins)}, "
                        f"expected {sorted(crate.bins)}"
                    )
                if libraries != {crate.library}:
                    raise CheckError(
                        f"{crate.name}: library targets {sorted(libraries)}, "
                        f"expected {[crate.library]}"
                    )
                target_root = package_root / crate.name
                archive = _package(crate, version, target_root)
                userguide = _inspect(crate, version, archive)
                _smoke(crate, version, userguide, target_root)
                print(
                    f"check_rust_packages: ok {crate.name} {version} "
                    f"({', '.join(crate.bins)})"
                )
    except (
        CheckError,
        json.JSONDecodeError,
        OSError,
        subprocess.SubprocessError,
        tarfile.TarError,
        UnicodeError,
    ) as error:
        print(f"check_rust_packages: FAIL: {error}", file=sys.stderr)
        return 1
    print(f"check_rust_packages: all {len(CRATES)} crates passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
