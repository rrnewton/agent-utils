"""Per-project ``.herdr-run.yaml``: which workspace, which tab, and what may run.

The tool is generic; the POLICY is per-project. A project states its desired Herdr workspace name,
its tab-name schema, and its command allowlist in one tracked file next to its source, so the
allowlist is reviewable in that project's history rather than baked into this shared utility.

Every field has a working default, so a project with no config file at all still gets the intended
conservative behaviour (workspace ``agent-cmds``, one tab per agent, only ``git``/``gh``, optionally
prefixed with ``with-proxy``). PyYAML is therefore an OPTIONAL dependency: it is imported lazily and
only when a config file actually exists, which keeps ``--help`` working on a bare host (the
``make check-deps`` contract).
"""

from __future__ import annotations

import os
from dataclasses import dataclass, field, replace

from herdr_run.errors import ConfigError
from herdr_run.jsonx import as_mapping, as_sequence

__all__ = ["Config", "load_config", "find_config_file", "CONFIG_FILENAMES"]

#: Accepted config basenames, in search order, looked up from the working directory upward.
CONFIG_FILENAMES: tuple[str, ...] = (".herdr-run.yaml", ".herdr-run.yml")

#: Programs allowed by default. Deliberately tiny: this is a sandbox door, not a shell.
_DEFAULT_ALLOW: tuple[str, ...] = ("git", "gh")

#: Wrapper programs that may precede an allowlisted program. ``with-proxy`` is the Meta forward-proxy
#: wrapper; it takes a command and execs it, so allowing it as a PREFIX (never as a program in its
#: own right) keeps ``with-proxy git push`` expressible without widening the allowlist to "anything
#: with-proxy can exec".
_DEFAULT_PREFIXES: tuple[str, ...] = ("with-proxy",)

#: Options that make an otherwise-allowlisted program run arbitrary code, matched among the GLOBAL
#: options that precede the subcommand. ``git -c core.pager=...`` / ``git -c alias.x='!sh'`` /
#: ``git --exec-path=/tmp/evil`` all turn "run git" into "run anything".
#:
#: Defense in depth, NOT the security boundary. The boundary is: the program name is allowlisted and
#: the argv is re-quoted so it cannot escape into the shell. A determined caller with `git` can still
#: reach a lot; see the Threat model section of the user guide.
_DEFAULT_DENY_GLOBAL: dict[str, tuple[str, ...]] = {
    "git": ("-c", "--config-env", "--exec-path", "--namespace"),
    "gh": (),
}

#: Subcommands (the first non-option token) that define or execute arbitrary code.
_DEFAULT_DENY_SUBCOMMAND: dict[str, tuple[str, ...]] = {
    "git": ("filter-branch", "daemon", "instaweb"),
    "gh": ("alias", "extension", "ext", "codespace", "cs"),
}

#: Options that are dangerous wherever they appear. ``--upload-pack``/``--receive-pack`` name a
#: program to execute for the "remote" side, which for a local path is simply local execution.
_DEFAULT_DENY_ANYWHERE: tuple[str, ...] = ("--upload-pack", "--receive-pack")

#: Global options that CONSUME the following token as their value, when written space-separated.
#: Needed to find the subcommand correctly: in ``git -C /tmp/repo log``, ``/tmp/repo`` is a value,
#: not the subcommand, and misreading it would point every subcommand-level rule at the wrong token.
_DEFAULT_VALUE_OPTIONS: dict[str, tuple[str, ...]] = {
    "git": ("-C", "-c", "--git-dir", "--work-tree", "--namespace", "--exec-path", "--config-env"),
    "gh": ("-R", "--repo"),
}

#: Process names accepted as "this pane is sitting at a shell prompt".
_DEFAULT_SHELLS: tuple[str, ...] = ("bash", "zsh", "sh", "dash", "fish", "ksh")


@dataclass(frozen=True)
class Config:
    """Resolved configuration. Immutable; CLI flags produce a modified copy via :func:`override`."""

    #: Herdr workspace LABEL (not id) that holds this project's command tabs.
    workspace: str = "agent-cmds"

    #: Tab-label schema. ``{agent}`` expands to the invoking agent name, ``{project}`` to the
    #: basename of the project root. Default: one tab per agent, e.g. ``release-agent``.
    tab_name: str = "{agent}"

    #: Working directory for commands, relative to the project root when not absolute.
    #: ``None`` means "the project root itself".
    cwd: str | None = None

    allow: tuple[str, ...] = _DEFAULT_ALLOW
    prefixes: tuple[str, ...] = _DEFAULT_PREFIXES
    deny_global: dict[str, tuple[str, ...]] = field(default_factory=lambda: dict(_DEFAULT_DENY_GLOBAL))
    deny_subcommand: dict[str, tuple[str, ...]] = field(
        default_factory=lambda: dict(_DEFAULT_DENY_SUBCOMMAND)
    )
    deny_anywhere: tuple[str, ...] = _DEFAULT_DENY_ANYWHERE
    value_options: dict[str, tuple[str, ...]] = field(
        default_factory=lambda: dict(_DEFAULT_VALUE_OPTIONS)
    )

    #: Where run spools (stdout/stderr/exit-code files) and the audit log live, relative to the
    #: project root. MUST be a git-ignored path: it holds command OUTPUT, not source. The default
    #: matches the sibling tools' convention (``.safe-ci-dag-runner/``, ``.tick-hub/``) so one
    #: ``.herdr-run/`` line in .gitignore covers it in any adopting project.
    spool_dir: str = ".herdr-run"

    #: Seconds to wait for the command's exit-code file after launching it.
    timeout_seconds: float = 900.0

    #: Seconds to wait for the pane to become idle before giving up with :class:`PaneBusy`.
    ready_timeout_seconds: float = 0.0

    #: Readiness policy. ``both`` (default) requires the process signal to say idle AND the prompt
    #: signal to not veto. ``process`` drops the prompt veto. See :mod:`herdr_run.readiness`.
    readiness: str = "both"

    #: Explicit prompt tail (the literal text a clean prompt line ends with, e.g. ``"$ "``). When
    #: ``None`` it is inferred from the shell rc file; inference failure means the prompt signal
    #: ABSTAINS rather than guesses.
    prompt_tail: str | None = None

    shells: tuple[str, ...] = _DEFAULT_SHELLS

    #: Remote used by the ``doctor`` self-test. It only needs to be a reachable repository that
    #: the sandbox blocks and the pane does not.
    probe_remote: str = "https://github.com/git/git"

    #: How herdr control calls reach the server. ``direct`` was measured to work from inside the
    #: agent jail (the server's unix socket is reachable); ``systemd-run`` brokers each call through
    #: a transient user unit for hosts where it is not.
    broker: str = "direct"

    #: Absolute path of the config file this came from, or ``None`` for pure defaults.
    source_path: str | None = None

    #: Project root: the directory holding the config file, else the starting directory.
    project_root: str = "."


def find_config_file(start: str) -> str | None:
    """Search ``start`` and each ancestor for a config file, returning the first hit.

    Nearest-wins, like ``.gitignore`` or ``.editorconfig``: a slot worktree can carry its own policy
    without the parent's file leaking in, and a subdirectory inherits its project's file.
    """
    current = os.path.abspath(start)
    while True:
        for name in CONFIG_FILENAMES:
            candidate = os.path.join(current, name)
            if os.path.isfile(candidate):
                return candidate
        parent = os.path.dirname(current)
        if parent == current:
            return None
        current = parent


def _str_tuple(raw: object, what: str) -> tuple[str, ...]:
    items = as_sequence(raw, what)
    out: list[str] = []
    for item in items:
        if not isinstance(item, str):
            raise ConfigError(f"{what}: every entry must be a string, got {type(item).__name__}")
        out.append(item)
    return tuple(out)


def _str_tuple_map(raw: object, what: str) -> dict[str, tuple[str, ...]]:
    mapping = as_mapping(raw, what)
    return {key: _str_tuple(value, f"{what}.{key}") for key, value in mapping.items()}


def _number(raw: object, what: str) -> float:
    if isinstance(raw, bool) or not isinstance(raw, (int, float)):
        raise ConfigError(f"{what}: must be a number, got {type(raw).__name__}")
    value = float(raw)
    if value < 0:
        raise ConfigError(f"{what}: must not be negative")
    return value


def _text(raw: object, what: str) -> str:
    if not isinstance(raw, str):
        raise ConfigError(f"{what}: must be a string, got {type(raw).__name__}")
    return raw


def _choice(raw: object, what: str, allowed: tuple[str, ...]) -> str:
    value = _text(raw, what)
    if value not in allowed:
        raise ConfigError(f"{what}: must be one of {', '.join(allowed)}; got {value!r}")
    return value


def parse_config(document: object, *, source_path: str | None, project_root: str) -> Config:
    """Build a :class:`Config` from an already-decoded YAML document.

    Split out from :func:`load_config` so the whole validation surface is testable without touching
    the filesystem or requiring PyYAML.
    """
    if document is None:
        return Config(source_path=source_path, project_root=project_root)

    what = source_path or "<config>"
    mapping = as_mapping(document, what)

    known = {
        "workspace",
        "tab_name",
        "cwd",
        "allow",
        "prefixes",
        "deny_global",
        "deny_subcommand",
        "deny_anywhere",
        "value_options",
        "spool_dir",
        "timeout_seconds",
        "ready_timeout_seconds",
        "readiness",
        "prompt_tail",
        "shells",
        "broker",
        "probe_remote",
    }
    unknown = sorted(set(mapping) - known)
    if unknown:
        # Reject rather than ignore: a typo'd `allowlist:` key silently falling back to the default
        # allowlist is exactly the kind of quiet policy failure this tool must not have.
        raise ConfigError(f"{what}: unknown key(s): {', '.join(unknown)}. Known keys: {', '.join(sorted(known))}")

    config = Config(source_path=source_path, project_root=project_root)
    if "workspace" in mapping:
        config = replace(config, workspace=_text(mapping["workspace"], f"{what}.workspace"))
    if "tab_name" in mapping:
        config = replace(config, tab_name=_text(mapping["tab_name"], f"{what}.tab_name"))
    if "cwd" in mapping:
        raw_cwd = mapping["cwd"]
        config = replace(config, cwd=None if raw_cwd is None else _text(raw_cwd, f"{what}.cwd"))
    if "allow" in mapping:
        config = replace(config, allow=_str_tuple(mapping["allow"], f"{what}.allow"))
    if "prefixes" in mapping:
        config = replace(config, prefixes=_str_tuple(mapping["prefixes"], f"{what}.prefixes"))
    if "deny_global" in mapping:
        config = replace(config, deny_global=_str_tuple_map(mapping["deny_global"], f"{what}.deny_global"))
    if "deny_subcommand" in mapping:
        config = replace(
            config, deny_subcommand=_str_tuple_map(mapping["deny_subcommand"], f"{what}.deny_subcommand")
        )
    if "deny_anywhere" in mapping:
        config = replace(config, deny_anywhere=_str_tuple(mapping["deny_anywhere"], f"{what}.deny_anywhere"))
    if "value_options" in mapping:
        config = replace(config, value_options=_str_tuple_map(mapping["value_options"], f"{what}.value_options"))
    if "spool_dir" in mapping:
        config = replace(config, spool_dir=_text(mapping["spool_dir"], f"{what}.spool_dir"))
    if "timeout_seconds" in mapping:
        config = replace(config, timeout_seconds=_number(mapping["timeout_seconds"], f"{what}.timeout_seconds"))
    if "ready_timeout_seconds" in mapping:
        config = replace(
            config,
            ready_timeout_seconds=_number(mapping["ready_timeout_seconds"], f"{what}.ready_timeout_seconds"),
        )
    if "readiness" in mapping:
        config = replace(config, readiness=_choice(mapping["readiness"], f"{what}.readiness", ("both", "process")))
    if "prompt_tail" in mapping:
        raw_tail = mapping["prompt_tail"]
        config = replace(config, prompt_tail=None if raw_tail is None else _text(raw_tail, f"{what}.prompt_tail"))
    if "shells" in mapping:
        config = replace(config, shells=_str_tuple(mapping["shells"], f"{what}.shells"))
    if "probe_remote" in mapping:
        config = replace(config, probe_remote=_text(mapping["probe_remote"], f"{what}.probe_remote"))
    if "broker" in mapping:
        config = replace(config, broker=_choice(mapping["broker"], f"{what}.broker", ("direct", "systemd-run")))

    if not config.allow:
        raise ConfigError(f"{what}.allow: refusing an EMPTY allowlist — no command could ever run")
    return config


def load_config(*, explicit_path: str | None, start_dir: str) -> Config:
    """Resolve the effective config: explicit path, else nearest ancestor file, else defaults."""
    path = explicit_path
    if path is not None:
        if not os.path.isfile(path):
            raise ConfigError(f"config file not found: {path}")
        path = os.path.abspath(path)
    else:
        path = find_config_file(start_dir)

    if path is None:
        return Config(source_path=None, project_root=os.path.abspath(start_dir))

    project_root = os.path.dirname(path)
    try:
        with open(path, encoding="utf-8") as handle:
            text = handle.read()
    except OSError as exc:
        raise ConfigError(f"cannot read config file {path}: {exc}") from exc

    try:
        import yaml
    except ModuleNotFoundError as exc:  # pragma: no cover - environment-dependent
        raise ConfigError(
            f"{path} exists but PyYAML is not installed. Install it with "
            "'python3 -m pip install pyyaml>=6', or delete the config file to fall back to "
            "built-in defaults."
        ) from exc

    try:
        document: object = yaml.safe_load(text)
    except yaml.YAMLError as exc:
        raise ConfigError(f"{path}: invalid YAML: {exc}") from exc

    return parse_config(document, source_path=path, project_root=project_root)
