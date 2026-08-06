"""Per-project ``.herdr-run.yaml``: which workspace, which tab, and what may run.

The tool is generic; the POLICY is per-project. A project states its desired Herdr workspace name,
its tab-name schema, and its command allowlist in one tracked file next to its source, so the
allowlist is reviewable in that project's history rather than baked into this shared utility.

Every field has a working default, so a project with no config file at all still gets the intended
conservative behaviour (workspace ``agent-cmds``, one tab per agent, and only ``git``/``gh``,
optionally prefixed with ``with-proxy``). The declared YAML dependency is imported lazily only when
a config file exists, which keeps bootstrap help useful even in a damaged installation.
"""

from __future__ import annotations

import math
import os
import string
from dataclasses import dataclass, field, replace

from herdr_run.errors import ConfigError
from herdr_run.jsonx import as_mapping, as_sequence
from herdr_run.retention import MAX_RETENTION_DAYS
from herdr_run.yamlcore import core_load

__all__ = [
    "CONFIG_FILENAMES",
    "MAX_RETENTION_DAYS",
    "MAX_TIMEOUT_SECONDS",
    "Config",
    "find_config_file",
    "load_config",
]

#: Accepted config basenames, in search order, looked up from the working directory upward.
CONFIG_FILENAMES: tuple[str, ...] = (".herdr-run.yaml", ".herdr-run.yml")

#: Largest command/readiness timeout accepted from either configuration or the CLI. Keeping this
#: comfortably below platform ``Instant``/``time.monotonic`` limits means a finite value can never
#: overflow into an accidental infinite wait in either implementation.
MAX_TIMEOUT_SECONDS = 31_536_000.0

#: Programs allowed by default. Deliberately tiny: this is a sandbox door, not a shell. Cargo is
#: NOT a default: even dependency-oriented subcommands can execute configured rustc wrappers,
#: credential providers, or fetch helpers. Projects may explicitly opt it in, accepting that trust
#: widening; :data:`_DEFAULT_ALLOW_SUBCOMMAND` still fails closed against compiling subcommands.
_DEFAULT_ALLOW: tuple[str, ...] = ("git", "gh")

#: Wrapper programs that may precede an allowlisted program. A wrapper takes a command and execs it,
#: so allowing it as a PREFIX (never as a program in its own right) keeps wrapped commands
#: expressible without widening the allowlist to "anything this wrapper can exec".
_DEFAULT_PREFIXES: tuple[str, ...] = ("with-proxy",)

#: Options that make an otherwise-allowlisted program run arbitrary code. Git's are matched among
#: GLOBAL options preceding its subcommand; Cargo accepts its global options on either side, so its
#: entries are matched everywhere. ``git -c core.pager=...`` / ``git -c alias.x='!sh'`` /
#: ``git --exec-path=/tmp/evil`` all turn "run git" into "run anything".
#:
#: Defense in depth for cooperative callers, not a same-user security boundary. A determined caller
#: with `git` can still reach a lot; see the trust model in the user guide.
_DEFAULT_DENY_GLOBAL: dict[str, tuple[str, ...]] = {
    "git": ("-c", "--config-env", "--exec-path", "--namespace"),
    "gh": (),
    # These are Cargo's CLI configuration-mutation surfaces: `--config` can inject source
    # replacement, compilers, wrappers, and credential providers; `-Z` unlocks unstable behavior.
    # They are unnecessary for the dependency-oriented commands admitted after explicit opt-in.
    # Ordinary selection/output flags (`--manifest-path`, `--target`, `--registry`, feature and
    # lock flags) do not themselves name a process or mutate Cargo configuration. Ambient Cargo
    # configuration can still execute helpers, which is why Cargo is not in `_DEFAULT_ALLOW`.
    "cargo": ("--config", "-Z"),
}

#: Per-program subcommand ALLOWLIST. When a program appears here its subcommand MUST be in the
#: list; anything else -- including no subcommand at all -- is refused. This limits an explicit
#: ``cargo`` opt-in, but does not make Cargo a no-code-execution boundary: Cargo may invoke ambient
#: rustc wrappers, credential providers, or fetch helpers even during dependency resolution.
#:
#: WHY: the pane runs OUTSIDE the sandbox, and `cargo build`/`test`/`run` execute build scripts and
#: proc macros from freshly downloaded third-party crates. Allowing those would run untrusted code
#: outside the very confinement this tool exists to cross in a controlled way. The subcommands below
#: are dependency-resolution-oriented and do not request compilation. A project that deliberately
#: accepts Cargo's ambient helper execution can fetch through the door and then build in-jail with
#: `--offline` against the warm cache.
#:
#: A deny-list is the wrong shape here: `build`, `test`, `run`, `bench`, `install`, `rustc`,
#: `clippy`, `doc`, `miri` and every third-party `cargo-*` subcommand execute code, and a new one
#: can appear at any time.
_DEFAULT_ALLOW_SUBCOMMAND: dict[str, tuple[str, ...]] = {
    "cargo": ("fetch", "update", "generate-lockfile", "vendor", "metadata", "tree", "search"),
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
    "git": (
        "-C",
        "-c",
        "--git-dir",
        "--work-tree",
        "--namespace",
        "--exec-path",
        "--config-env",
    ),
    "gh": ("-R", "--repo"),
    "cargo": ("--manifest-path", "--config", "-Z", "-p", "--package", "--target-dir"),
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
    #: ``None`` leaves command execution at the caller's current directory; session bootstrap uses
    #: the project root when it needs a concrete initial directory.
    cwd: str | None = None

    allow: tuple[str, ...] = _DEFAULT_ALLOW
    prefixes: tuple[str, ...] = _DEFAULT_PREFIXES
    deny_global: dict[str, tuple[str, ...]] = field(
        default_factory=lambda: dict(_DEFAULT_DENY_GLOBAL)
    )
    deny_subcommand: dict[str, tuple[str, ...]] = field(
        default_factory=lambda: dict(_DEFAULT_DENY_SUBCOMMAND)
    )
    deny_anywhere: tuple[str, ...] = _DEFAULT_DENY_ANYWHERE
    allow_subcommand: dict[str, tuple[str, ...]] = field(
        default_factory=lambda: dict(_DEFAULT_ALLOW_SUBCOMMAND)
    )
    value_options: dict[str, tuple[str, ...]] = field(
        default_factory=lambda: dict(_DEFAULT_VALUE_OPTIONS)
    )

    #: Where run spools (stdout/stderr/exit-code files) and the audit log live, relative to the
    #: project root. MUST be a git-ignored path: it holds command OUTPUT, not source. Add the
    #: default ``.herdr-run/`` directory to each adopting project's ``.gitignore``.
    spool_dir: str = ".herdr-run"

    #: Seconds to wait for the command's exit-code file after launching it.
    timeout_seconds: float = 900.0

    #: Days of run spools to keep. Pruned when a new run is written; see :mod:`herdr_run.retention`.
    retention_days: int = 4

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

    #: How herdr control calls reach the server. ``direct`` uses the server's Unix socket;
    #: ``systemd-run`` brokers each call through a transient user unit for hosts where that socket
    #: is not reachable from the caller.
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
            raise ConfigError(
                f"{what}: every entry must be a string, got {type(item).__name__}"
            )
        out.append(_policy_name(item, f"{what}[{len(out)}]"))
    return tuple(out)


def _str_tuple_map(raw: object, what: str) -> dict[str, tuple[str, ...]]:
    mapping = as_mapping(raw, what)
    return {
        _policy_name(key, f"{what} key"): _str_tuple(value, f"{what}.{key}")
        for key, value in mapping.items()
    }


def _number(raw: object, what: str) -> float:
    if isinstance(raw, bool) or not isinstance(raw, (int, float)):
        raise ConfigError(f"{what}: must be a number, got {type(raw).__name__}")
    try:
        value = float(raw)
    except (OverflowError, ValueError) as exc:
        raise ConfigError(f"{what}: must be a finite number") from exc
    if not math.isfinite(value):
        raise ConfigError(f"{what}: must be finite")
    if value < 0:
        raise ConfigError(f"{what}: must not be negative")
    if value > MAX_TIMEOUT_SECONDS:
        raise ConfigError(f"{what}: must not exceed {MAX_TIMEOUT_SECONDS:g} seconds")
    return value


def _nonnegative_integer(raw: object, what: str) -> int:
    if isinstance(raw, bool) or not isinstance(raw, int) or raw < 0:
        raise ConfigError(
            f"{what}: must be a non-negative integer, got {type(raw).__name__}"
        )
    if raw > MAX_RETENTION_DAYS:
        raise ConfigError(f"{what}: must not exceed {MAX_RETENTION_DAYS} days")
    return raw


def _text(raw: object, what: str) -> str:
    if not isinstance(raw, str):
        raise ConfigError(f"{what}: must be a string, got {type(raw).__name__}")
    if any(
        ord(char) < 32 or 127 <= ord(char) <= 159 or 0xD800 <= ord(char) <= 0xDFFF
        for char in raw
    ):
        raise ConfigError(
            f"{what}: control characters and Unicode surrogates are not allowed"
        )
    return raw


def _policy_name(raw: object, what: str) -> str:
    value = _text(raw, what)
    if not value:
        raise ConfigError(f"{what}: must not be empty")
    return value


def render_tab_name(schema: str, *, agent: str, project: str) -> str:
    """Render a tab schema restricted to literal text plus ``{agent}``/``{project}``."""
    try:
        chunks = list(string.Formatter().parse(schema))
    except ValueError as exc:
        raise ConfigError(f"tab_name schema {schema!r} is malformed: {exc}") from exc
    for _literal, field_name, format_spec, conversion in chunks:
        if field_name is None:
            continue
        if (
            field_name not in ("agent", "project")
            or format_spec
            or conversion is not None
        ):
            raise ConfigError(
                f"tab_name schema {schema!r} may use only plain {{agent}} and {{project}} placeholders"
            )
    try:
        rendered = schema.format(agent=agent, project=project)
    except (KeyError, IndexError, AttributeError, ValueError) as exc:
        raise ConfigError(f"tab_name schema {schema!r} is invalid: {exc}") from exc
    return _policy_name(rendered, "rendered tab_name")


def _choice(raw: object, what: str, allowed: tuple[str, ...]) -> str:
    value = _text(raw, what)
    if value not in allowed:
        raise ConfigError(f"{what}: must be one of {', '.join(allowed)}; got {value!r}")
    return value


def _parse_config(
    document: object, *, source_path: str | None, project_root: str
) -> Config:
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
        "allow_subcommand",
        "deny_anywhere",
        "value_options",
        "spool_dir",
        "timeout_seconds",
        "retention_days",
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
        raise ConfigError(
            f"{what}: unknown key(s): {', '.join(unknown)}. Known keys: {', '.join(sorted(known))}"
        )

    config = Config(source_path=source_path, project_root=project_root)
    if "workspace" in mapping:
        config = replace(
            config, workspace=_policy_name(mapping["workspace"], f"{what}.workspace")
        )
    if "tab_name" in mapping:
        config = replace(
            config, tab_name=_policy_name(mapping["tab_name"], f"{what}.tab_name")
        )
    if "cwd" in mapping:
        raw_cwd = mapping["cwd"]
        config = replace(
            config, cwd=None if raw_cwd is None else _text(raw_cwd, f"{what}.cwd")
        )
    if "allow" in mapping:
        config = replace(config, allow=_str_tuple(mapping["allow"], f"{what}.allow"))
    if "prefixes" in mapping:
        config = replace(
            config, prefixes=_str_tuple(mapping["prefixes"], f"{what}.prefixes")
        )
    if "deny_global" in mapping:
        config = replace(
            config,
            deny_global=_str_tuple_map(mapping["deny_global"], f"{what}.deny_global"),
        )
    if "deny_subcommand" in mapping:
        config = replace(
            config,
            deny_subcommand=_str_tuple_map(
                mapping["deny_subcommand"], f"{what}.deny_subcommand"
            ),
        )
    if "allow_subcommand" in mapping:
        config = replace(
            config,
            allow_subcommand=_str_tuple_map(mapping["allow_subcommand"], f"{what}.allow_subcommand"),
        )
    if "deny_anywhere" in mapping:
        config = replace(
            config,
            deny_anywhere=_str_tuple(mapping["deny_anywhere"], f"{what}.deny_anywhere"),
        )
    if "value_options" in mapping:
        config = replace(
            config,
            value_options=_str_tuple_map(
                mapping["value_options"], f"{what}.value_options"
            ),
        )
    if "spool_dir" in mapping:
        config = replace(
            config, spool_dir=_text(mapping["spool_dir"], f"{what}.spool_dir")
        )
    if "timeout_seconds" in mapping:
        config = replace(
            config,
            timeout_seconds=_number(
                mapping["timeout_seconds"], f"{what}.timeout_seconds"
            ),
        )
    if "retention_days" in mapping:
        config = replace(
            config,
            retention_days=_nonnegative_integer(
                mapping["retention_days"], f"{what}.retention_days"
            ),
        )
    if "ready_timeout_seconds" in mapping:
        config = replace(
            config,
            ready_timeout_seconds=_number(
                mapping["ready_timeout_seconds"], f"{what}.ready_timeout_seconds"
            ),
        )
    if "readiness" in mapping:
        config = replace(
            config,
            readiness=_choice(
                mapping["readiness"], f"{what}.readiness", ("both", "process")
            ),
        )
    if "prompt_tail" in mapping:
        raw_tail = mapping["prompt_tail"]
        config = replace(
            config,
            prompt_tail=(
                None if raw_tail is None else _text(raw_tail, f"{what}.prompt_tail")
            ),
        )
    if "shells" in mapping:
        config = replace(config, shells=_str_tuple(mapping["shells"], f"{what}.shells"))
    if "probe_remote" in mapping:
        config = replace(
            config, probe_remote=_text(mapping["probe_remote"], f"{what}.probe_remote")
        )
    if "broker" in mapping:
        config = replace(
            config,
            broker=_choice(
                mapping["broker"], f"{what}.broker", ("direct", "systemd-run")
            ),
        )

    if not config.allow:
        raise ConfigError(
            f"{what}.allow: refusing an EMPTY allowlist — no command could ever run"
        )
    if "cargo" in config.allow and "cargo" not in config.allow_subcommand:
        raise ConfigError(
            f"{what}.allow_subcommand: cargo is allowed but has no positive subcommand list"
        )
    for index, name in enumerate(config.allow):
        _policy_name(name, f"{what}.allow[{index}]")
    for index, name in enumerate(config.prefixes):
        _policy_name(name, f"{what}.prefixes[{index}]")
    for index, name in enumerate(config.shells):
        _policy_name(name, f"{what}.shells[{index}]")
    render_tab_name(config.tab_name, agent="agent", project="project")
    return config


def parse_config(
    document: object, *, source_path: str | None, project_root: str
) -> Config:
    """Validate a decoded configuration document without leaking narrowing exceptions.

    YAML is an untyped boundary.  Shape errors are configuration failures (exit 78), not Python
    programming errors that should escape as a traceback.
    """
    try:
        return _parse_config(
            document, source_path=source_path, project_root=project_root
        )
    except TypeError as exc:
        raise ConfigError(str(exc)) from exc


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
    except (OSError, UnicodeError) as exc:
        raise ConfigError(f"cannot read config file {path}: {exc}") from exc

    try:
        import yaml
    except ModuleNotFoundError as exc:  # pragma: no cover - damaged installation
        raise ConfigError(
            f"{path} exists but the declared PyYAML dependency is not installed. Repair this "
            "herdr-run installation."
        ) from exc

    try:
        document = core_load(text)
    except (yaml.YAMLError, ValueError, OverflowError) as exc:
        raise ConfigError(f"{path}: invalid YAML: {exc}") from exc

    return parse_config(document, source_path=path, project_root=project_root)
