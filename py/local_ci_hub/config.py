"""Project configuration for local-ci-hub (local-ci-hub.toml)."""
from __future__ import annotations


import socket
from dataclasses import dataclass, field
from pathlib import Path

import sys

if sys.version_info >= (3, 11):
    import tomllib
else:  # Python 3.10: the tomli package provides the same API
    import tomli as tomllib


class ConfigError(ValueError):
    """Raised when local-ci-hub.toml is invalid or incomplete."""

    pass


def short_host() -> str:
    """Return the short (unqualified) host name for ledger sharding."""
    return socket.gethostname().split(".")[0]


@dataclass
class Config:
    """A validated local-ci-hub project configuration."""
    path: Path
    root: Path
    name: str = "example"
    validate_command: list[str] = field(default_factory=lambda: ["make", "validate"])
    cells: list[str] = field(default_factory=list)
    admission_slots: int = 2
    ledger_dir: Path = Path("ci-ledger")
    team: str = "team"
    publish: str = "none"  # none | git
    provider: str = "none"  # none | github
    workflows: list[str] = field(default_factory=list)
    qualifying: list[str] = field(default_factory=lambda: ["local-ledger-pass"])
    probes: list[dict[str, object]] = field(default_factory=list)
    state_dir: Path = Path("ignored/local-ci-hub")
    evidence_root: Path | None = None
    require_full_evidence: bool = True
    release_payload: dict[str, object] | None = None
    digest_command: list[str] | None = None
    repos: list[dict[str, object]] = field(default_factory=list)

    @property
    def ledger_file(self) -> Path:
        """See the module documentation for this operation."""
        p = self.ledger_dir
        return p if p.suffix == ".jsonl" else p / "ledger.jsonl"

    @property
    def spool_dir(self) -> Path:
        """See the module documentation for this operation."""
        return self.state_dir / "spool"


def _resolve(root: Path, value: str) -> Path:
    p = Path(value)
    return p if p.is_absolute() else root / p


def load(path: Path | None, root: Path | None = None) -> Config:
    """Load and validate local-ci-hub.toml, or local-only defaults."""
    if path is None:
        base = root or Path.cwd()
        for cand in (base / "local-ci-hub.toml", base / "ci" / "local-ci-hub.toml"):
            if cand.exists():
                path = cand
                break
    if path is None or not Path(path).exists():
        # Defaults for a toy/local-only project; doctor reports what is implicit.
        base = root or Path.cwd()
        return Config(path=Path("local-ci-hub.toml"), root=base,
                      ledger_dir=base / "ci-ledger", state_dir=base / "ignored/local-ci-hub")
    path = Path(path)
    root = root or path.parent
    try:
        data = tomllib.loads(path.read_text())
    except Exception as exc:
        raise ConfigError(f"invalid TOML in {path}: {exc}") from exc
    proj = data.get("project", {})
    val = data.get("validate", {})
    led = data.get("ledger", {})
    rem = data.get("remote", {})
    rec = data.get("receipt", {})
    health = data.get("health", {})
    loc = led.get("location", {})
    ledger_raw = loc.get("path") or loc.get("repo_path") or "ci-ledger"
    cfg = Config(
        path=path, root=root,
        name=str(proj.get("name", "example")),
        repos=list(proj.get("repos", [])),
        validate_command=list(val.get("command", ["make", "validate"])),
        cells=list(val.get("cells", [])),
        admission_slots=int(val.get("admission_slots", 2)),
        ledger_dir=_resolve(root, str(ledger_raw)),
        team=str(led.get("team", "team")),
        publish=str(led.get("publish", "none")),
        provider=str(rem.get("provider", "none")),
        workflows=list(rem.get("workflows", [])),
        qualifying=list(rec.get("qualifying", ["local-ledger-pass"])),
        probes=list(health.get("probes", [])),
        state_dir=_resolve(root, str(data["land"]["state_dir"])) if data.get("land", {}).get("state_dir") else Path.home() / ".local" / "state" / "local-ci-hub",
        evidence_root=_resolve(root, str(led["evidence_root"])) if led.get("evidence_root") else None,
        require_full_evidence=bool(rec.get("require_full_evidence", True)),
        release_payload=dict(rec["release_payload"]) if rec.get("release_payload") else None,
        digest_command=list(rec["digest_command"]) if rec.get("digest_command") else None,
    )
    unknown = set(cfg.qualifying) - {"local-ledger-pass", "remote-pass"}
    if unknown:
        raise ConfigError(f"unknown qualifying rules: {sorted(unknown)}")
    if cfg.provider == "none" and "remote-pass" in cfg.qualifying:
        raise ConfigError("qualifying includes remote-pass but remote.provider is none")
    return cfg
