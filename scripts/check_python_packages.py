#!/usr/bin/env python3
"""Build, inspect, and smoke every independently published Python tool.

The check is intentionally artifact-first. Each project is copied out of the
working tree, built as both a wheel and source distribution with package-index
access disabled, rebuilt from that source archive, installed alone into a fresh
virtual environment without dependencies, and started with socket access
blocked. This catches source-tree success that masks missing modules, leaked
sibling packages, import-time networking, or omitted resources.

The build environment must already provide the PEP 517 backend named by each
project. No backend or runtime dependency is downloaded by this script.
"""

from __future__ import annotations

import ast
import configparser
import email.parser
import email.policy
import importlib
import importlib.metadata
import os
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import tarfile
import venv
import zipfile
from dataclasses import dataclass
from pathlib import Path
from typing import Protocol

REPO_ROOT = Path(__file__).resolve().parent.parent
PY_ROOT = REPO_ROOT / "py"


@dataclass(frozen=True)
class Project:
    directory: str
    distribution: str
    package: str
    commands: tuple[str, ...]
    resources: tuple[str, ...]
    required_dependencies: tuple[str, ...]


PROJECTS: tuple[Project, ...] = (
    Project(
        directory="safe_ci_dag_runner",
        distribution="safe-ci-dag-runner",
        package="safe_ci_dag_runner",
        commands=("safe-ci-dag-runner", "cpuset-alloc"),
        resources=("README.md", "USER_GUIDE.md", "py.typed"),
        required_dependencies=("pyyaml",),
    ),
    Project(
        directory="tick_hub",
        distribution="tick-hub",
        package="tick_hub",
        commands=("tick-hub",),
        resources=(
            "README.md",
            "USER_GUIDE.md",
            "py.typed",
            "examples/tick-hub-ops.json",
            "examples/tick-hub-ops.yaml",
            "examples/tick-hub-state.yaml",
        ),
        required_dependencies=("pyyaml",),
    ),
    Project(
        directory="pr_landing_planner",
        distribution="pr-landing-planner",
        package="pr_landing_planner",
        commands=("pr-landing-planner",),
        resources=(
            "README.md",
            "USER_GUIDE.md",
            "py.typed",
            "examples/flaky-signatures.yaml",
            "examples/pr-landing-demo.yaml",
            "check_outcome.py",
        ),
        required_dependencies=("pyyaml",),
    ),
    Project(
        directory="agent_team_timeline",
        distribution="agent-team-timeline",
        package="agent_team_timeline",
        commands=("agent-team-timeline",),
        resources=(
            "README.md",
            "USER_GUIDE.md",
            "py.typed",
            "static/index.html",
            "static/app.js",
            "static/timeline-core.js",
            "static/style.css",
            "static/vendor/README.md",
            "static/vendor/markdown-it-15.0.0.min.js",
            "static/vendor/markdown-it-LICENSE.txt",
        ),
        required_dependencies=(),
    ),
)

_DIST_SEP = re.compile(r"[-_.]+")
_FOREIGN_DOC_TERMS = re.compile(
    r"\b(?:cargo|crate|crates\.io|rust|rustc|rustup)\b", re.IGNORECASE
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
    (
        "development-history language",
        re.compile(
            r"\b(?:prototype|roadmap|formerly|previously|planned|not yet|legacy|historical|"
            r"predates?|follow-on|stub)\b",
            re.IGNORECASE,
        ),
    ),
    (
        "implementation-provenance language",
        re.compile(
            r"\b(?:ported|parity|cross-language|both builds?)\b|"
            r"\b(?:direct|generic|typed)?\s*port of\b",
            re.IGNORECASE,
        ),
    ),
)
_REQUIREMENT_NAME = re.compile(r"^\s*([A-Za-z0-9][A-Za-z0-9._-]*)")
_PUBLIC_SCAN_IGNORED = {"__pycache__", "build", "dist"}

_PublicDocNode = ast.Module | ast.ClassDef | ast.FunctionDef | ast.AsyncFunctionDef


class CheckError(RuntimeError):
    """One package failed its artifact contract."""


class _WheelArchive(Protocol):
    """The small, stable archive surface used by wheel inspection."""

    def namelist(self) -> list[str]: ...

    def read(self, name: str) -> bytes: ...


@dataclass(frozen=True)
class _WheelMetadata:
    """The normalized metadata fields the artifact contract consumes."""

    name: str
    version: str
    requires_python: str
    requirements: tuple[str, ...]
    description: str


def _public_doc_nodes(tree: ast.Module) -> list[_PublicDocNode]:
    nodes: list[_PublicDocNode] = [tree]

    def add_public_members(body: list[ast.stmt]) -> None:
        for node in body:
            if not isinstance(node, (ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)):
                continue
            if node.name.startswith("_"):
                continue
            nodes.append(node)
            if isinstance(node, ast.ClassDef):
                add_public_members(node.body)

    add_public_members(tree.body)
    return nodes


def _check_public_docstrings() -> None:
    """Keep every published package's introspection docs standalone."""

    for project in PROJECTS:
        package = PY_ROOT / project.directory
        for path in sorted(package.rglob("*.py")):
            relative = path.relative_to(package)
            if any(
                part in _PUBLIC_SCAN_IGNORED or part.endswith(".egg-info")
                for part in relative.parts
            ):
                continue
            tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
            for node in _public_doc_nodes(tree):
                docstring = ast.get_docstring(node, clean=False)
                if docstring is None:
                    name = getattr(node, "name", "<module>")
                    line = getattr(node, "lineno", 1)
                    raise CheckError(
                        f"{project.distribution}: public API {relative}:{line} "
                        f"({name}) has no docstring"
                    )
                errors = _doc_violations(project, docstring)
                if errors:
                    name = getattr(node, "name", "<module>")
                    line = getattr(node, "lineno", 1)
                    raise CheckError(
                        f"{project.distribution}: public docstring "
                        f"{relative}:{line} "
                        f"({name}) is not standalone: {'; '.join(errors)}"
                    )


def _normalized_distribution(value: str) -> str:
    return _DIST_SEP.sub("-", value).lower()


def _require_build_backend() -> None:
    """Fail early with an actionable error when the offline backend is absent."""

    try:
        importlib.import_module("setuptools.build_meta")
        setuptools_version = importlib.metadata.version("setuptools")
    except (ImportError, importlib.metadata.PackageNotFoundError) as exc:
        raise CheckError(
            "setuptools.build_meta is unavailable; install setuptools>=77 in the "
            "interpreter running this offline package check"
        ) from exc
    major_match = re.match(r"([0-9]+)", setuptools_version)
    if major_match is None or int(major_match.group(1)) < 77:
        raise CheckError(
            f"setuptools {setuptools_version!r} is too old; this repository requires setuptools>=77"
        )


def _requirement_name(requirement: str) -> str:
    match = _REQUIREMENT_NAME.match(requirement)
    return _normalized_distribution(match.group(1)) if match is not None else ""


def _doc_violations(project: Project, text: str) -> list[str]:
    errors: list[str] = []
    foreign = _FOREIGN_DOC_TERMS.search(text)
    if foreign is not None:
        errors.append(f"foreign-language term {foreign.group(0)!r}")
    for description, pattern in _COMMON_DOC_TERMS:
        match = pattern.search(text)
        if match is not None:
            errors.append(f"{description} {match.group(0)!r}")
    for sibling in PROJECTS:
        if sibling == project:
            continue
        for name in (sibling.distribution, sibling.package):
            match = re.search(re.escape(name), text, re.IGNORECASE)
            if match is not None:
                errors.append(f"sibling package {match.group(0)!r}")
                break
    return errors


def _offline_env() -> dict[str, str]:
    env = dict(os.environ)
    env.update(
        {
            "PIP_DISABLE_PIP_VERSION_CHECK": "1",
            "PIP_NO_INDEX": "1",
            "PIP_NO_INPUT": "1",
            "PYTHONDONTWRITEBYTECODE": "1",
            "NO_COLOR": "1",
        }
    )
    return env


def _run(
    argv: list[str],
    *,
    env: dict[str, str],
    cwd: Path | None = None,
    timeout: int = 120,
) -> subprocess.CompletedProcess[str]:
    try:
        proc = subprocess.run(
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
    if proc.returncode != 0:
        command = " ".join(argv)
        raise CheckError(
            f"command failed ({proc.returncode}): {command}\n"
            f"stdout:\n{proc.stdout}\nstderr:\n{proc.stderr}"
        )
    return proc


def _copy_project(project: Project, destination: Path) -> Path:
    source = PY_ROOT / project.directory
    manifest = source / "pyproject.toml"
    if not manifest.is_file():
        raise CheckError(f"{source}: missing pyproject.toml")
    for relative in ("README.md", "USER_GUIDE.md", "LICENSE"):
        linked = source / relative
        if not linked.is_symlink() or not linked.resolve().is_file():
            raise CheckError(
                f"{project.distribution}: source {relative} must be a valid authoritative symlink"
            )
    manifest_text = manifest.read_text(encoding="utf-8")
    foreign = _FOREIGN_DOC_TERMS.search(manifest_text)
    if foreign is not None:
        raise CheckError(
            f"{project.distribution}: pyproject.toml contains foreign-language term "
            f"{foreign.group(0)!r}"
        )
    target = destination / project.directory
    shutil.copytree(
        source,
        target,
        ignore=shutil.ignore_patterns(
            "__pycache__", "*.pyc", "*.pyo", "*.egg-info", "build", "dist"
        ),
    )
    return target


def _build_wheel(project: Project, source: Path, wheel_root: Path) -> Path:
    before = set(wheel_root.glob("*.whl"))
    _run(
        [
            sys.executable,
            "-m",
            "pip",
            "wheel",
            "--no-deps",
            "--no-build-isolation",
            "--wheel-dir",
            str(wheel_root),
            str(source),
        ],
        env=_offline_env(),
    )
    created = set(wheel_root.glob("*.whl")) - before
    if len(created) != 1:
        raise CheckError(
            f"{project.distribution}: expected exactly one wheel, found {sorted(created)}"
        )
    return created.pop()


def _build_sdist(project: Project, source: Path, sdist_root: Path) -> Path:
    before = set(sdist_root.glob("*.tar.gz"))
    code = (
        "import sys; from setuptools.build_meta import build_sdist; "
        "print(build_sdist(sys.argv[1]))"
    )
    _run(
        [sys.executable, "-c", code, str(sdist_root)],
        env=_offline_env(),
        cwd=source,
    )
    created = set(sdist_root.glob("*.tar.gz")) - before
    if len(created) != 1:
        raise CheckError(
            f"{project.distribution}: expected exactly one sdist, found {sorted(created)}"
        )
    return created.pop()


def _inspect_sdist(project: Project, source: Path, archive: Path) -> None:
    with tarfile.open(archive, mode="r:gz") as package:
        members = package.getmembers()
        links = sorted(member.name for member in members if member.issym() or member.islnk())
        if links:
            raise CheckError(
                f"{project.distribution}: sdist contains repository links instead of files: {links}"
            )
        roots = {member.name.split("/", 1)[0] for member in members if member.name}
        if len(roots) != 1:
            raise CheckError(f"{project.distribution}: sdist has unexpected roots {sorted(roots)}")
        prefix = f"{next(iter(roots))}/"
        by_name = {member.name: member for member in members}
        expected = {
            f"{prefix}pyproject.toml",
            f"{prefix}PKG-INFO",
            f"{prefix}LICENSE",
            *(f"{prefix}{resource}" for resource in project.resources),
        }
        for path in source.rglob("*.py"):
            relative = path.relative_to(source)
            if any(
                part in _PUBLIC_SCAN_IGNORED or part.endswith(".egg-info")
                for part in relative.parts
            ):
                continue
            expected.add(f"{prefix}{relative.as_posix()}")
        missing = sorted(expected - set(by_name))
        if missing:
            raise CheckError(f"{project.distribution}: sdist is missing {missing}")
        for document in ("LICENSE", "README.md", "USER_GUIDE.md"):
            name = f"{prefix}{document}"
            member = by_name[name]
            if not member.isfile():
                raise CheckError(f"{project.distribution}: sdist {document} is not a regular file")
            extracted = package.extractfile(member)
            if extracted is None:
                raise CheckError(f"{project.distribution}: could not read sdist {document}")
            artifact_bytes = extracted.read()
            source_bytes = (source / document).read_bytes()
            if artifact_bytes != source_bytes:
                raise CheckError(
                    f"{project.distribution}: sdist {document} differs from its authoritative source"
                )


def _metadata(archive: _WheelArchive) -> tuple[str, _WheelMetadata]:
    paths = [name for name in archive.namelist() if name.endswith(".dist-info/METADATA")]
    if len(paths) != 1:
        raise CheckError(f"wheel has {len(paths)} METADATA files (expected one)")
    raw = archive.read(paths[0])
    parsed = email.parser.BytesParser(policy=email.policy.default).parsebytes(raw)
    if not isinstance(parsed, email.message.EmailMessage):
        raise CheckError("could not parse wheel METADATA")
    metadata = _WheelMetadata(
        name=str(parsed.get("Name", "")),
        version=str(parsed.get("Version", "")),
        requires_python=str(parsed.get("Requires-Python", "")),
        requirements=tuple(str(value) for value in parsed.get_all("Requires-Dist", [])),
        description=parsed.get_content(),
    )
    return paths[0].removesuffix("METADATA"), metadata


def _entry_points(archive: _WheelArchive, dist_info: str) -> dict[str, str]:
    path = f"{dist_info}entry_points.txt"
    try:
        text = archive.read(path).decode("utf-8")
    except KeyError as exc:
        raise CheckError("wheel is missing entry_points.txt") from exc
    parser = configparser.ConfigParser(interpolation=None)
    parser.read_string(text)
    return dict(parser.items("console_scripts")) if parser.has_section("console_scripts") else {}


def _inspect_wheel(project: Project, wheel: Path) -> tuple[str, str]:
    with zipfile.ZipFile(wheel) as archive:
        names = set(archive.namelist())
        linked_members = sorted(
            info.filename
            for info in archive.infolist()
            if stat.S_IFMT(info.external_attr >> 16) == stat.S_IFLNK
        )
        if linked_members:
            raise CheckError(
                f"{project.distribution}: wheel contains repository links instead of files: "
                f"{linked_members}"
            )
        dist_info, metadata = _metadata(archive)

        actual_name = metadata.name
        if _normalized_distribution(actual_name) != _normalized_distribution(
            project.distribution
        ):
            raise CheckError(
                f"{wheel.name}: metadata name {actual_name!r}, expected {project.distribution!r}"
            )
        version = metadata.version
        if not version:
            raise CheckError(f"{project.distribution}: empty metadata version")
        requires_python = metadata.requires_python
        if not requires_python:
            raise CheckError(f"{project.distribution}: metadata has no Requires-Python")

        requirements = list(metadata.requirements)
        requirement_names = {_requirement_name(requirement) for requirement in requirements}
        runtime_requirement_names = {
            _requirement_name(requirement)
            for requirement in requirements
            if "extra ==" not in requirement.lower()
        }
        missing_requirements = sorted(
            set(project.required_dependencies) - runtime_requirement_names
        )
        unexpected_requirements = sorted(
            runtime_requirement_names - set(project.required_dependencies)
        )
        if missing_requirements or unexpected_requirements:
            raise CheckError(
                f"{project.distribution}: runtime dependencies "
                f"{sorted(runtime_requirement_names)}, "
                f"expected {sorted(project.required_dependencies)}"
            )
        sibling_requirements = sorted(
            sibling.distribution
            for sibling in PROJECTS
            if sibling != project
            and _normalized_distribution(sibling.distribution) in requirement_names
        )
        if sibling_requirements:
            raise CheckError(
                f"{project.distribution}: metadata depends on sibling package(s) "
                f"{sibling_requirements}"
            )

        description_errors = _doc_violations(project, metadata.description)
        if description_errors:
            raise CheckError(
                f"{project.distribution}: wheel metadata README is not standalone: "
                f"{'; '.join(description_errors)}"
            )

        expected_entries = set(project.commands)
        entries = _entry_points(archive, dist_info)
        if set(entries) != expected_entries:
            raise CheckError(
                f"{project.distribution}: console scripts {sorted(entries)}, "
                f"expected {sorted(expected_entries)}"
            )
        for command, target in entries.items():
            if not target.startswith(f"{project.package}."):
                raise CheckError(
                    f"{project.distribution}: {command} points outside its package: {target}"
                )

        package_prefix = f"{project.package}/"
        for resource in project.resources:
            path = f"{package_prefix}{resource}"
            if path not in names:
                raise CheckError(f"{project.distribution}: wheel is missing {path}")

        source_package = PY_ROOT / project.directory
        source_modules: dict[str, Path] = {}
        for source_path in source_package.rglob("*.py"):
            relative = source_path.relative_to(source_package)
            if any(
                part in _PUBLIC_SCAN_IGNORED or part.endswith(".egg-info")
                for part in relative.parts
            ):
                continue
            source_modules[f"{package_prefix}{relative.as_posix()}"] = source_path
        missing_modules = sorted(set(source_modules) - names)
        if missing_modules:
            raise CheckError(
                f"{project.distribution}: wheel is missing source modules {missing_modules}"
            )
        changed_modules = sorted(
            path
            for path, source_path in source_modules.items()
            if archive.read(path) != source_path.read_bytes()
        )
        if changed_modules:
            raise CheckError(
                f"{project.distribution}: wheel modules differ from differential-tested source: "
                f"{changed_modules}"
            )

        documents: dict[str, str] = {}
        for document in ("README.md", "USER_GUIDE.md"):
            path = f"{package_prefix}{document}"
            artifact_text = archive.read(path).decode("utf-8")
            source_text = (PY_ROOT / project.directory / document).read_text(encoding="utf-8")
            if artifact_text != source_text:
                raise CheckError(
                    f"{project.distribution}: wheel {document} differs from its authoritative source"
                )
            errors = _doc_violations(project, artifact_text)
            if errors:
                raise CheckError(
                    f"{project.distribution}: wheel {document} is not standalone: "
                    f"{'; '.join(errors)}"
                )
            documents[document] = artifact_text
        if metadata.description != documents["README.md"]:
            raise CheckError(
                f"{project.distribution}: metadata long description differs from packaged README.md"
            )

        foreign_packages = {
            other.package
            for other in PROJECTS
            if other.package != project.package
            and any(name.startswith(f"{other.package}/") for name in names)
        }
        if foreign_packages:
            raise CheckError(
                f"{project.distribution}: wheel leaks sibling packages {sorted(foreign_packages)}"
            )
        if any(name == "ci_hub_check_outcome.py" for name in names):
            raise CheckError("pr-landing-planner authority leaked as an unpackaged top-level shim")

        license_paths = [
            name
            for name in names
            if name.startswith(dist_info)
            and name.lower().endswith(("/license", "/license.txt", "/copying"))
        ]
        if not license_paths:
            raise CheckError(f"{project.distribution}: wheel contains no license file")
        authoritative_license = (REPO_ROOT / "LICENSE").read_bytes()
        if not any(archive.read(path) == authoritative_license for path in license_paths):
            raise CheckError(
                f"{project.distribution}: wheel license differs from the authoritative license"
            )
        return version, documents["USER_GUIDE.md"]


_SITE_CUSTOMIZE = '''\
"""Network-denial guard for artifact startup checks."""
import socket
import urllib.request

def _blocked(*args, **kwargs):
    raise RuntimeError("network access is forbidden during package smoke tests")

class _OfflineSocket(socket.socket):
    def connect(self, *args, **kwargs):
        return _blocked(*args, **kwargs)
    def connect_ex(self, *args, **kwargs):
        _blocked(*args, **kwargs)
        return 1

socket.socket = _OfflineSocket
socket.create_connection = _blocked
urllib.request.urlopen = _blocked
'''


def _venv_paths(root: Path) -> tuple[Path, Path]:
    bindir = root / ("Scripts" if os.name == "nt" else "bin")
    python = bindir / ("python.exe" if os.name == "nt" else "python")
    return bindir, python


def _smoke_wheel(
    project: Project,
    wheel: Path,
    version: str,
    userguide: str,
    smoke_root: Path,
) -> None:
    environment_root = smoke_root / project.directory
    venv.EnvBuilder(with_pip=True, clear=True).create(environment_root)
    bindir, python = _venv_paths(environment_root)
    env = _offline_env()
    run_root = smoke_root / "work" / project.directory
    run_root.mkdir(parents=True)

    guard = environment_root / "offline_guard"
    guard.mkdir()
    (guard / "sitecustomize.py").write_text(_SITE_CUSTOMIZE, encoding="utf-8")
    env["PYTHONPATH"] = str(guard)

    _run(
        [
            str(python),
            "-m",
            "pip",
            "install",
            "--no-index",
            "--no-deps",
            str(wheel),
        ],
        env=env,
        cwd=run_root,
    )

    sibling_assertions = "; ".join(
        f"assert importlib.util.find_spec({other.package!r}) is None"
        for other in PROJECTS
        if other.package != project.package
    )
    version_code = (
        "import importlib.util; from pathlib import Path; "
        "from importlib.metadata import version; "
        f"import {project.package} as package; "
        f"assert version({project.distribution!r}) == {version!r}; "
        f"assert package.__version__ == {version!r}; "
        f"assert Path(package.__file__).resolve().is_relative_to(Path({str(environment_root)!r}).resolve()); "
        f"{sibling_assertions}"
    )
    _run([str(python), "-c", version_code], env=env, cwd=run_root)

    module_invocations = (("--help",), ("--version",), ("--userguide",))
    for args in module_invocations:
        result = _run(
            [str(python), "-m", project.package, *args],
            env=env,
            cwd=run_root,
            timeout=30,
        )
        combined = result.stdout + result.stderr
        if "Traceback (most recent call last)" in combined:
            raise CheckError(f"python -m {project.package} {' '.join(args)} emitted a traceback")
        if args == ("--version",) and version not in combined:
            raise CheckError(
                f"python -m {project.package} --version did not report {version!r}: {combined!r}"
            )
        if args == ("--userguide",) and (result.stdout != userguide or result.stderr):
            raise CheckError(
                f"python -m {project.package} --userguide differs from packaged USER_GUIDE.md"
            )

    for command in project.commands:
        executable = bindir / command
        if not executable.is_file():
            raise CheckError(f"{project.distribution}: installed command missing: {executable}")
        invocations: list[list[str]] = [["--help"], ["--version"]]
        if command != "cpuset-alloc":
            invocations.append(["--userguide"])
        for command_args in invocations:
            result = _run(
                [str(executable), *command_args], env=env, cwd=run_root, timeout=30
            )
            combined = result.stdout + result.stderr
            if "Traceback (most recent call last)" in combined:
                raise CheckError(f"{command} {' '.join(command_args)} emitted a traceback")
            if command_args == ["--version"] and version not in combined:
                raise CheckError(f"{command} --version did not report {version!r}: {combined!r}")
            if command_args == ["--userguide"] and (
                result.stdout != userguide or result.stderr
            ):
                raise CheckError(f"{command} --userguide differs from packaged USER_GUIDE.md")


def main() -> int:
    try:
        _require_build_backend()
        _check_public_docstrings()
        index_text = (PY_ROOT / "README.md").read_text(encoding="utf-8")
        foreign = _FOREIGN_DOC_TERMS.search(index_text)
        if foreign is not None:
            raise CheckError(
                "py/README.md contains foreign-language term "
                f"{foreign.group(0)!r}"
            )
        with tempfile.TemporaryDirectory(prefix="agent-utils-python-packages-") as raw:
            root = Path(raw)
            build_root = root / "sources"
            source_wheel_root = root / "source-wheels"
            sdist_root = root / "sdists"
            sdist_wheel_root = root / "sdist-wheels"
            smoke_root = root / "venvs"
            build_root.mkdir()
            source_wheel_root.mkdir()
            sdist_root.mkdir()
            sdist_wheel_root.mkdir()
            smoke_root.mkdir()

            for project in PROJECTS:
                source = _copy_project(project, build_root)
                source_wheel = _build_wheel(project, source, source_wheel_root)
                source_result = _inspect_wheel(project, source_wheel)
                sdist = _build_sdist(project, source, sdist_root)
                _inspect_sdist(project, source, sdist)
                sdist_wheel = _build_wheel(project, sdist, sdist_wheel_root)
                sdist_result = _inspect_wheel(project, sdist_wheel)
                if sdist_result != source_result:
                    raise CheckError(
                        f"{project.distribution}: source and sdist wheels disagree: "
                        f"{source_result!r} != {sdist_result!r}"
                    )
                version, userguide = sdist_result
                _smoke_wheel(project, sdist_wheel, version, userguide, smoke_root)
                print(
                    f"check_python_packages: ok {project.distribution} {version} "
                    f"wheel + sdist ({', '.join(project.commands)})"
                )
    except (
        CheckError,
        OSError,
        UnicodeError,
        configparser.Error,
        subprocess.SubprocessError,
        tarfile.TarError,
        zipfile.BadZipFile,
    ) as exc:
        print(f"check_python_packages: FAIL: {exc}", file=sys.stderr)
        return 1
    print(f"check_python_packages: all {len(PROJECTS)} distributions passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
