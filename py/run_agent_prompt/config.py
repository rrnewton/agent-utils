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
