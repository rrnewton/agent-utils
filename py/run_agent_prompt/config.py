"""Optional TOML config: per-harness model pins and the effort default.

The file is ``~/.config/agent-utils/run-agent-prompt.toml`` unless
``--config`` names another. Every key is optional:

    effort = "low"

    [muse]
    model = "..."
    effort = "high"   # optional per-harness override

    [claude]
    model = "..."

    [codex]
    model = "..."

Explicit CLI flags override the file. A model left unset everywhere
runs the harness default and is recorded from the harness's own output
where it reports one.
"""

from __future__ import annotations

import os
import sys
from dataclasses import dataclass, field
from pathlib import Path

HARNESSES = ("muse", "claude", "codex")


def default_config_path() -> Path:
    """The default config path under the XDG config home."""

    base = os.environ.get("XDG_CONFIG_HOME")
    root = Path(base) if base else Path.home() / ".config"
    return root / "agent-utils" / "run-agent-prompt.toml"


@dataclass(frozen=True)
class Config:
    """Resolved config-file values (all optional)."""

    effort: str | None = None
    models: dict[str, str] = field(default_factory=dict)
    efforts: dict[str, str] = field(default_factory=dict)


def load_config(path: Path | None) -> Config:
    """Load the config at *path* (or the default path when None).

    A missing default file is an empty config; a missing explicitly
    named file, malformed TOML, or a non-string value is an error.
    """

    explicit = path is not None
    path = path or default_config_path()
    if not path.exists():
        if explicit:
            raise ValueError(f"config file does not exist: {path}")
        return Config()
    if sys.version_info >= (3, 11):
        import tomllib
    else:  # Python 3.10: the tomli package provides the same API.
        import tomli as tomllib

    try:
        with path.open("rb") as handle:
            document = tomllib.load(handle)
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise ValueError(f"could not read config {path}: {error}") from error
    if not isinstance(document, dict):
        raise ValueError(f"config {path} is not a TOML table")

    def text(table: dict[str, object], key: str, where: str) -> str | None:
        value = table.get(key)
        if value is None:
            return None
        if not isinstance(value, str) or not value:
            raise ValueError(f"config {where}.{key} must be a non-empty string")
        return value

    models: dict[str, str] = {}
    efforts: dict[str, str] = {}
    for harness in HARNESSES:
        table = document.get(harness)
        if table is None:
            continue
        if not isinstance(table, dict):
            raise ValueError(f"config [{harness}] must be a table")
        if (model := text(table, "model", harness)) is not None:
            models[harness] = model
        if (effort := text(table, "effort", harness)) is not None:
            efforts[harness] = effort
    return Config(effort=text(document, "effort", "top level"), models=models, efforts=efforts)


#: Schema of a recovery profile file (`REGISTRY/recovery-profiles.toml`).
PROFILES_SCHEMA = "agentctl-recovery-profiles/v1"


@dataclass(frozen=True)
class Profile:
    """One ordered harness/model profile for recovery judgement."""

    name: str
    harness: str
    model: str | None = None
    effort: str | None = None
    timeout_seconds: float | None = None
    command: str | None = None
    disabled: bool = False


def load_profiles(path: Path) -> list[Profile]:
    """Load the ordered recovery profiles at *path*.

    The file order is the attempt order; there is no hidden built-in
    order when a file is present. A missing file, malformed TOML, an
    unknown key or harness, a duplicate name, or a non-absolute
    command is an error: recovery judgement must refuse, not guess.
    """

    if sys.version_info >= (3, 11):
        import tomllib
    else:  # Python 3.10: the tomli package provides the same API.
        import tomli as tomllib

    try:
        with path.open("rb") as handle:
            document = tomllib.load(handle)
    except OSError as error:
        raise ValueError(f"could not read profiles {path}: {error}") from error
    except tomllib.TOMLDecodeError as error:
        raise ValueError(f"could not read profiles {path}: {error}") from error
    if not isinstance(document, dict):
        raise ValueError(f"profiles {path} is not a TOML table")
    unknown = set(document) - {"schema", "profile"}
    if unknown:
        raise ValueError(f"profiles {path}: unknown top-level keys {sorted(unknown)}")
    if document.get("schema") != PROFILES_SCHEMA:
        raise ValueError(f"profiles {path}: schema must be {PROFILES_SCHEMA}")
    tables = document.get("profile")
    if not isinstance(tables, list) or not tables:
        raise ValueError(f"profiles {path}: at least one [[profile]] is required")

    def profile_text(table: dict[str, object], key: str, where: str) -> str | None:
        value = table.get(key)
        if value is None:
            return None
        if not isinstance(value, str) or not value:
            raise ValueError(f"profiles {path}: {where}.{key} must be a non-empty string")
        return value

    profiles: list[Profile] = []
    names: set[str] = set()
    allowed = {"name", "harness", "model", "effort", "timeout_seconds", "command", "disabled"}
    for table in tables:
        if not isinstance(table, dict):
            raise ValueError(f"profiles {path}: each [[profile]] must be a table")
        unknown = set(table) - allowed
        if unknown:
            raise ValueError(f"profiles {path}: unknown profile keys {sorted(unknown)}")
        name = profile_text(table, "name", "profile")
        harness = profile_text(table, "harness", "profile")
        if name is None or harness is None:
            raise ValueError(f"profiles {path}: each profile needs name and harness")
        if name in names:
            raise ValueError(f"profiles {path}: duplicate profile name {name}")
        names.add(name)
        if harness not in HARNESSES:
            raise ValueError(f"profiles {path}: unknown harness {harness}")
        command = profile_text(table, "command", name)
        if command is not None and not Path(command).is_absolute():
            raise ValueError(f"profiles {path}: profile {name} command must be absolute")
        timeout = table.get("timeout_seconds")
        if timeout is not None and (
            isinstance(timeout, bool)
            or not isinstance(timeout, (int, float))
            or timeout <= 0
            or timeout != timeout
            or timeout in (float("inf"), float("-inf"))
            or timeout > 3600
        ):
            raise ValueError(f"profiles {path}: profile {name} timeout_seconds must be positive")
        disabled = table.get("disabled", False)
        if not isinstance(disabled, bool):
            raise ValueError(f"profiles {path}: profile {name} disabled must be a boolean")
        profiles.append(
            Profile(
                name=name,
                harness=harness,
                model=profile_text(table, "model", name),
                effort=profile_text(table, "effort", name),
                timeout_seconds=float(timeout) if timeout is not None else None,
                command=command,
                disabled=disabled,
            )
        )
    return profiles
