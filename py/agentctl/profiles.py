"""Strict, project-local launch profiles for :mod:`agentctl`.

Profiles are launch input, not session state.  They live in the ignored private
``.agentctl/profiles.json`` file below the selected working directory and are
read without evaluating shell syntax.
"""
from __future__ import annotations

import json
import os
import re
import stat
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import cast

from agentctl.errors import AgentDeliveryError

SCHEMA = "agentctl-profiles/v1"
PROFILE_PATH = Path(".agentctl/profiles.json")
_NAME = re.compile(r"[a-z][a-z0-9-]{0,31}\Z")
_ENVIRONMENT_NAME = re.compile(r"[A-Za-z_][A-Za-z0-9_]*\Z")
_SECRET_NAME = re.compile(r"(?:TOKEN|SECRET|PASSWORD|PASSWD|CREDENTIAL|API_KEY|PRIVATE_KEY)", re.I)
_SECRET_OPTION = re.compile(r"^--?(?:api[-_]?key|token|secret|password|passwd|credential)(?:=|$)", re.I)
_EFFORTS = frozenset(("none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"))
_HARNESSES = frozenset(("codex", "claude", "muse", "agy"))
_MODES = frozenset(("interactive", "headless"))
_MUSE_EXEC_OWNED_OPTIONS = frozenset(
    ("--", "--api-key-stdin", "--json", "--session-id", "--prompt-file")
)
_MAX_CONFIG_BYTES = 256 * 1024
_SYSTEM_GIT = Path("/usr/bin/git")


class _DuplicateKey(ValueError):
    pass


@dataclass(frozen=True)
class LaunchProfile:
    """One validated profile; environment values are intentionally not printable."""

    name: str
    harness: str
    mode: str
    model: str | None
    reasoning_effort: str | None
    argv: tuple[str, ...]
    environment: tuple[str, ...]

    def public(self) -> dict[str, object]:
        """Return discovery metadata without argument or environment values."""
        return {
            "name": self.name,
            "harness": self.harness,
            "mode": self.mode,
            "model": self.model,
            "reasoning_effort": self.reasoning_effort,
            "argv_count": len(self.argv),
            "environment": [entry.partition("=")[0] for entry in self.environment],
        }


def profile_path(cwd: str | Path) -> Path:
    """Return the only implicit profile location for *cwd*."""
    return Path(cwd).expanduser().resolve() / PROFILE_PATH


def _open_private_regular_file(path: Path) -> int:
    """Open and pin a same-user, single-link file below a private directory."""
    try:
        parent = path.parent.lstat()
    except OSError as exc:
        raise AgentDeliveryError(f"cannot inspect profile config {path}: {exc}") from exc
    if stat.S_ISLNK(parent.st_mode) or not stat.S_ISDIR(parent.st_mode):
        raise AgentDeliveryError(f"profile config parent must be a real directory: {path.parent}")
    uid = os.getuid()
    if parent.st_uid != uid or parent.st_mode & 0o077:
        raise AgentDeliveryError(f"profile config must be private (directory 0700, file 0600): {path}")
    try:
        descriptor = os.open(
            path, os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW | os.O_NONBLOCK
        )
    except OSError as exc:
        raise AgentDeliveryError(f"cannot open profile config {path}: {exc}") from exc
    metadata = os.fstat(descriptor)
    if (not stat.S_ISREG(metadata.st_mode) or metadata.st_nlink != 1
            or metadata.st_uid != uid or metadata.st_mode & 0o077):
        os.close(descriptor)
        raise AgentDeliveryError(
            f"profile config must be a same-user private single-link regular file: {path}"
        )
    return descriptor


def _unique_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        if key in result:
            raise _DuplicateKey(f"duplicate key {key!r}")
        result[key] = value
    return result


def _require_ignored(cwd: Path, path: Path) -> None:
    try:
        relative = path.relative_to(cwd)
    except ValueError as exc:
        raise AgentDeliveryError("profile config must remain below the selected working directory") from exc
    try:
        git = Path(os.path.realpath(_SYSTEM_GIT))
        metadata = git.stat()
        if (not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != 0
                or metadata.st_mode & (stat.S_IWGRP | stat.S_IWOTH)
                or not os.access(git, os.X_OK)):
            raise AgentDeliveryError(f"refusing unsafe Git executable: {git}")
        checked = subprocess.run(
            [str(git), "-C", str(cwd), "check-ignore", "--quiet", "--", str(relative)],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=5,
        )
    except AgentDeliveryError:
        raise
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise AgentDeliveryError(f"cannot verify that profile config is ignored: {exc}") from exc
    if checked.returncode != 0:
        raise AgentDeliveryError(
            f"profile config is not ignored by Git: {relative}; add .agentctl/ to .gitignore"
        )


def _text(value: object, field: str, *, optional: bool = False) -> str | None:
    if value is None and optional:
        return None
    if not isinstance(value, str) or not value or "\0" in value:
        raise AgentDeliveryError(f"profile field {field} must be a nonempty NUL-free string")
    return value


def _value_option(value: str, short: str, long: str) -> bool:
    """Recognize both split and attached spellings of a value-taking option."""
    return (value == short or value == long or value.startswith(f"{long}=")
            or (not value.startswith("--") and value.startswith(short)
                and len(value) > len(short)))


def _codex_config_key(argv: tuple[str, ...], index: int) -> str | None:
    """Return the key assigned by one Codex ``-c``/``--config`` occurrence."""
    item = argv[index]
    assignment: str | None = None
    if item in ("-c", "--config"):
        if index + 1 < len(argv):
            assignment = argv[index + 1]
    elif item.startswith("--config="):
        assignment = item.removeprefix("--config=")
    elif not item.startswith("--") and item.startswith("-c") and len(item) > 2:
        assignment = item[2:].removeprefix("=")
    if assignment is None or "=" not in assignment:
        return None
    return assignment.split("=", 1)[0].strip()


def validate_raw_harness_arguments(
    harness: str, argv: tuple[str, ...] | list[str], *, label: str,
    structured_model: bool = False, structured_effort: bool = False,
    structured_resume: bool = False,
) -> None:
    """Keep structured model and effort settings authoritative over raw argv."""
    arguments = tuple(argv)
    option_keys = [item.split("=", 1)[0] for item in arguments if item.startswith("-")]
    config_keys = {
        key for index in range(len(arguments))
        if (key := _codex_config_key(arguments, index)) is not None
    } if harness == "codex" else set()
    has_codex_config = harness == "codex" and any(
        _value_option(item, "-c", "--config") for item in arguments
    )
    if (any(_value_option(item, "-m", "--model") for item in arguments)
            or "model" in config_keys):
        raise AgentDeliveryError(f"{label} must set model with the model field")
    if (any(key in ("--reasoning-effort", "--effort") for key in option_keys)
            or any(item.startswith("model_reasoning_effort=") for item in arguments)
            or "model_reasoning_effort" in config_keys):
        raise AgentDeliveryError(
            f"{label} must set reasoning effort with the reasoning_effort field"
        )
    if (harness == "codex" and (structured_model or structured_effort)
            and (has_codex_config
                 or any(_value_option(item, "-p", "--profile") for item in arguments))):
        raise AgentDeliveryError(
            f"{label} cannot combine structured model or reasoning effort "
            "with raw Codex config or profile arguments"
        )
    duplicate_resume = (
        harness == "codex" and "resume" in arguments
    ) or (
        harness == "claude" and any(
            _value_option(item, "-r", "--resume")
            or item in ("-c", "--continue")
            for item in arguments
        )
    )
    if structured_resume and duplicate_resume:
        raise AgentDeliveryError(
            f"{label} cannot repeat the structured resume selector in raw arguments"
        )


def _profile(name: str, raw: object) -> LaunchProfile:
    if not _NAME.fullmatch(name):
        raise AgentDeliveryError(f"invalid profile name {name!r}; use lowercase letters, digits, and hyphens")
    if not isinstance(raw, dict):
        raise AgentDeliveryError(f"profile {name!r} must be an object")
    allowed = {"harness", "mode", "model", "reasoning_effort", "argv", "env"}
    unknown = sorted(set(raw) - allowed)
    if unknown:
        raise AgentDeliveryError(f"profile {name!r} has unknown fields: {', '.join(unknown)}")
    harness = _text(raw.get("harness"), f"{name}.harness")
    mode = _text(raw.get("mode"), f"{name}.mode")
    assert harness is not None and mode is not None
    if harness not in _HARNESSES:
        raise AgentDeliveryError(f"profile {name!r} has unsupported harness {harness!r}")
    if mode not in _MODES:
        raise AgentDeliveryError(f"profile {name!r} has unsupported mode {mode!r}")
    supported = (
        harness in ("codex", "claude", "muse")
        if mode == "interactive"
        else harness in ("codex", "agy", "muse")
    )
    if not supported:
        raise AgentDeliveryError(
            f"profile {name!r} has unsupported harness/mode combination "
            f"{harness!r}/{mode!r}"
        )
    model = _text(raw.get("model"), f"{name}.model", optional=True)
    effort = _text(raw.get("reasoning_effort"), f"{name}.reasoning_effort", optional=True)
    if effort is not None and effort not in _EFFORTS:
        raise AgentDeliveryError(f"profile {name!r} has unsupported reasoning effort {effort!r}")
    raw_argv = raw.get("argv", [])
    if not isinstance(raw_argv, list) or any(not isinstance(item, str) or not item or "\0" in item for item in raw_argv):
        raise AgentDeliveryError(f"profile {name!r} argv must be an array of nonempty NUL-free strings")
    argv = tuple(cast(list[str], raw_argv))
    if mode == "headless" and harness != "muse" and argv:
        raise AgentDeliveryError(
            f"profile {name!r} headless {harness} does not support raw argv"
        )
    if mode == "headless" and harness != "muse" and effort is not None:
        raise AgentDeliveryError(
            f"profile {name!r} headless {harness} does not support reasoning_effort"
        )
    for index, item in enumerate(argv):
        if _SECRET_OPTION.match(item) or (index and _SECRET_OPTION.match(argv[index - 1])):
            raise AgentDeliveryError(f"profile {name!r} argv appears to contain a secret; use the harness credential store")
    # Structured and runner-owned settings never enter raw argv. This removes
    # order-dependent precedence and keeps the headless session/prompt binding
    # under agentctl's control.
    validate_raw_harness_arguments(
        harness, argv, label=f"profile {name!r}",
        structured_model=model is not None, structured_effort=effort is not None,
    )
    if harness == "muse" and mode == "headless":
        validate_muse_headless_arguments(argv, profile=name)
    raw_env = raw.get("env", {})
    if not isinstance(raw_env, dict):
        raise AgentDeliveryError(f"profile {name!r} env must be an object")
    if mode == "headless" and raw_env:
        raise AgentDeliveryError(
            f"profile {name!r} headless mode does not support environment entries"
        )
    environment: list[str] = []
    for key in sorted(raw_env):
        value = raw_env[key]
        if not isinstance(key, str) or _ENVIRONMENT_NAME.fullmatch(key) is None or "\0" in key:
            raise AgentDeliveryError(f"profile {name!r} has an invalid environment variable name")
        if _SECRET_NAME.search(key):
            raise AgentDeliveryError(f"profile {name!r} environment {key!r} appears secret; use the harness credential store")
        if not isinstance(value, str) or "\0" in value:
            raise AgentDeliveryError(f"profile {name!r} environment values must be NUL-free strings")
        environment.append(f"{key}={value}")
    return LaunchProfile(name, harness, mode, model, effort, argv, tuple(environment))


def load_profiles(cwd: str | Path, *, absent_ok: bool = False) -> tuple[Path, dict[str, LaunchProfile]]:
    """Load and validate the ignored private profile file for *cwd*."""
    root = Path(cwd).expanduser().resolve()
    path = profile_path(root)
    try:
        path.lstat()
    except FileNotFoundError:
        if absent_ok:
            return path, {}
        raise AgentDeliveryError(f"profile config does not exist: {path}") from None
    except OSError as exc:
        raise AgentDeliveryError(f"cannot inspect profile config {path}: {exc}") from exc
    descriptor = _open_private_regular_file(path)
    try:
        _require_ignored(root, path)
        chunks: list[bytes] = []
        remaining = _MAX_CONFIG_BYTES + 1
        while remaining:
            chunk = os.read(descriptor, remaining)
            if not chunk:
                break
            chunks.append(chunk)
            remaining -= len(chunk)
        data = b"".join(chunks)
    except Exception:
        os.close(descriptor)
        raise
    os.close(descriptor)
    if len(data) > _MAX_CONFIG_BYTES:
        raise AgentDeliveryError(
            f"profile config exceeds {_MAX_CONFIG_BYTES} bytes: {path}"
        )
    try:
        raw: object = json.loads(data.decode("utf-8"), object_pairs_hook=_unique_object)
    except (UnicodeError, json.JSONDecodeError, RecursionError, _DuplicateKey) as exc:
        raise AgentDeliveryError(f"cannot read profile config {path}: {exc}") from exc
    if not isinstance(raw, dict) or set(raw) != {"schema", "profiles"}:
        raise AgentDeliveryError("profile config must contain exactly schema and profiles")
    if raw.get("schema") != SCHEMA or not isinstance(raw.get("profiles"), dict):
        raise AgentDeliveryError(f"profile config must use schema {SCHEMA!r} and an object of profiles")
    profiles = {name: _profile(name, value) for name, value in cast(dict[str, object], raw["profiles"]).items()}
    return path, dict(sorted(profiles.items()))


def profile_arguments(profile: LaunchProfile) -> tuple[str, ...]:
    """Translate structured settings to literal harness argv."""
    args = list(reasoning_arguments(profile.harness, profile.reasoning_effort))
    args.extend(profile.argv)
    return tuple(args)


def validate_muse_headless_arguments(
    argv: tuple[str, ...] | list[str], *, profile: str | None = None,
) -> None:
    """Keep Muse exec's subcommand, session identity, and prompt runner-owned."""
    label = f"profile {profile!r}" if profile is not None else "Muse headless launch"
    # Structured settings are compiled to two-token pairs before Sessions sees
    # them. Other option values must remain in one literal ``--key=value``
    # element, so an unowned positional can never replace the runner's prompt.
    seen_singletons: set[str] = set()
    index = 0
    while index < len(argv):
        item = argv[index]
        key = item.split("=", 1)[0]
        if key in _MUSE_EXEC_OWNED_OPTIONS or item in ("exec", "resume"):
            raise AgentDeliveryError(f"{label} cannot override runner-owned option {key!r}")
        if item in ("--model", "--reasoning-effort", "--effort"):
            if item in seen_singletons:
                raise AgentDeliveryError(
                    f"{label} repeats singleton option {item!r}; precedence must be explicit"
                )
            if index + 1 >= len(argv) or argv[index + 1].startswith("-"):
                raise AgentDeliveryError(f"{label} option {item!r} requires one value")
            seen_singletons.add(item)
            index += 2
            continue
        if not item.startswith("-"):
            raise AgentDeliveryError(
                f"{label} argv must use --option=value for options with values; positional arguments are reserved for the prompt"
            )
        if key in ("--model", "--reasoning-effort", "--effort"):
            if key in seen_singletons:
                raise AgentDeliveryError(
                    f"{label} repeats singleton option {key!r}; precedence must be explicit"
                )
            seen_singletons.add(key)
        index += 1


def reasoning_arguments(harness: str, effort: str | None) -> tuple[str, ...]:
    """Translate one explicit effort without inventing permission policy."""
    if effort is None:
        return ()
    if effort not in _EFFORTS:
        raise AgentDeliveryError(f"unsupported reasoning effort {effort!r}")
    if harness == "codex":
        return ("--config", f"model_reasoning_effort={effort}")
    if harness == "claude":
        return ("--effort", effort)
    if harness == "muse":
        return ("--reasoning-effort", effort)
    raise AgentDeliveryError(f"reasoning effort is not supported for harness {harness!r}")
