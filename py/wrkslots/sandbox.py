"""Run a command boxed to one slot: resource limits plus a confined file-system view.

``wrkslots run SLOT -- COMMAND`` needs no host configuration. The box is
orthogonal to how the slot is stored: a plain-worktree slot and a disk-image
slot get exactly the same limits and the same view; only the location of the
slot's private state differs (the state image, or ``<control>/slot-state/``).

* **Limits.** Each slot gets its own slice, ``wrkslots-<slot>.slice``, under
  ``wrkslots.slice`` (nested inside whatever slice the caller already occupies).
  Memory, CPU, task, and IO limits apply to the slice, so every command run
  against the same slot shares one budget.
* **Process tree.** The calling process itself is placed in a transient systemd
  *scope* in that slice, then execs, so COMMAND keeps the caller's PID. A
  terminal multiplexer that detects agents by the pane's own shell PID (Herdr)
  still sees the agent.
* **Isolation modes.** ``userns`` builds the view in a new unprivileged user and
  mount namespace. ``root`` builds the identical view through a short-lived
  ``sudo -n`` launcher in a plain mount namespace, then drops to the invoking
  user's uid, gid, and groups and seals it (``no_new_privs``) before exec, so
  setuid helpers inside the box (``sudo``, a harness launcher's seteuid step)
  fail exactly as under ``userns``. ``cgroup`` applies the limits only and
  leaves the file system alone. Every mode is an accident boundary for
  cooperative but sloppy agents, not a security boundary against a hostile
  or racing process running as the same user.
* **The view** (``userns`` and ``root``), built by :func:`build_view`:

  1. ``/tmp`` is a fresh tmpfs for this launch, discarded at exit.
  2. The real ``$HOME`` (and every other mount path that exposes the same
     directory) stays visible **read-only**, submounts included; nothing is
     copied. ``home: hidden`` covers it with an empty tmpfs instead and binds
     back only the ``home_expose`` paths, read-only.
  3. ``home_shared`` paths (agent credentials and transcripts) are bound
     read-write onto themselves; ``home_private`` paths are per-slot,
     persistent, writable directories from the slot's state.
  4. The slot, the Git common directories its checkouts commit into, the
     wrkslots control directory, the project's blessed ``outputs``, and any
     ``read_write`` paths are writable.
  5. ``home_hidden`` entries are masked: directories by an empty read-only
     tmpfs, files by ``/dev/null``.
  6. With ``protect_system`` every remaining mount becomes read-only, except
     ``/proc``, ``/sys``, ``/dev``, and ``/run/user``.
  7. An Eden slot (``SlotView.eden_checkout``) masks the shared host daemon
     configuration directories, other slots' ``.eden-state`` and the host
     backing store (the last with a bare shared-directory skeleton so the
     lane's own ``sl root`` still works), re-roots the namespace on a
     user-owned tmpfs so Sapling's shared-directory ownership walk passes,
     and, with ``SlotView.checkout_alias``, binds its own checkout again at
     a caller-chosen absolute path.

The network is not touched: agent harnesses and site policy own that.
"""

from __future__ import annotations

import contextlib
import ctypes
import dataclasses
import errno as _errno
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import random
import time
from pathlib import Path
from typing import Callable, Mapping, Sequence

ISOLATIONS = ("userns", "cgroup", "root")
COORDINATOR_SCOPES = ("worktrees", "project")
HOME_MODES = ("ro", "hidden")
#: Agent harness state that must stay shared with the host and writable:
#: credentials, settings, and session transcripts. A private copy per slot
#: would log the agent out and scatter its history.
DEFAULT_HOME_SHARED = (
    ".claude",
    ".codex",
    ".muse",
    ".config/muse",
    ".config/opencode",
    ".local/share/opencode",
    ".local/state/herdr",
    ".local/share/muse",
    # Package caches: a build that fetches a crate it has not cached writes here.
    ".cargo/registry",
    ".cargo/git",
)
#: Per-slot, persistent, writable directories: they live in the slot's private
#: $HOME layer instead of being bound from the real $HOME.
DEFAULT_HOME_PRIVATE = (".cache", ".buck")
#: Top-level $HOME files seeded ONCE into the slot's layer as a private,
#: writable copy. A harness that rewrites its state file with a temporary file
#: and a rename (``~/.claude.json``) needs a writable $HOME directory and a
#: real file, which a bind mount of the host's file cannot provide.
DEFAULT_HOME_PRIVATE_FILES = (".claude.json",)
#: Credentials masked inside the box even though the rest of $HOME is readable.
DEFAULT_HOME_HIDDEN = (
    ".ssh",
    ".gnupg",
    ".aws",
    ".azure",
    ".kube",
    ".docker/config.json",
    ".netrc",
    ".git-credentials",
    ".pgpass",
    ".arcrc",
    ".config/gh/hosts.yml",
    ".config/gcloud",
)
#: What ``home: hidden`` binds back, read-only.
DEFAULT_HOME_EXPOSE = ("bin", ".local/bin")
#: Project-root-relative directories the box may write in the primary checkout.
DEFAULT_OUTPUTS = ("ai_docs", "experiments")
DEFAULT_ENV: tuple[tuple[str, str], ...] = ()
DEFAULT_TMP_SIZE = "16G"
DEFAULT_TASKS_MAX = 8192
#: ``run --in-scope --git-config-policy``. inherit (the default): the view shows
#: whatever Git configuration the host has. isolated: the fixed default Git
#: configuration locations below are masked (or confirmed absent) inside the view.
GIT_CONFIG_POLICIES = ("inherit", "isolated")
#: $HOME-relative locations the isolated policy masks below $HOME and every alias
#: of it: the global file and the default XDG directory ($HOME/.config/git, the
#: location used when XDG_CONFIG_HOME is unset).
GIT_CONFIG_HOME_PATHS = (".gitconfig", ".config/git")
#: The fixed system file the isolated policy masks: libgit2's POSIX default
#: system configuration path. Not configurable: the policy takes no path input.
GIT_CONFIG_SYSTEM_PATHS = ("/etc/gitconfig",)
_ENV_DENYLIST = frozenset(
    {
        "INVOCATION_ID",
        "JOURNAL_STREAM",
        "NOTIFY_SOCKET",
        "LISTEN_FDS",
        "LISTEN_PID",
        "LISTEN_FDNAMES",
        "MANAGERPID",
        "SYSTEMD_EXEC_PID",
    }
)
_TEMP_VARIABLES = ("TMPDIR", "TMP", "TEMP")
_LIMIT_RE = re.compile(r"^(infinity|\d+(\.\d+)?[KMGT]?|\d+(\.\d+)?%)$")
_TMP_SIZE_RE = re.compile(r"^(\d+[KMGT]?|\d+%)$")
_ENV_NAME_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")
#: Keys of earlier drafts, refused with the replacement named.
_RENAMED_KEYS = {
    "home_writable": "home_private",
    "home_paths": "home_expose (with home: hidden)",
    "memory_max": "limits.memory_max",
    "memory_high": "limits.memory_high",
    "cpu_quota": "limits.cpu_quota",
    "tasks_max": "limits.tasks_max",
    "io_weight": "limits.io_weight",
    "all_slots_memory_max": "limits.all_slots_memory_max",
    "all_slots_cpu_quota": "limits.all_slots_cpu_quota",
}


class SandboxError(RuntimeError):
    """A sandbox request is invalid or cannot run on this host."""


class MountError(SandboxError):
    """A mount system call failed; ``errno`` says why."""

    def __init__(self, message: str, error: int) -> None:
        super().__init__(message)
        self.errno = error


# ------------------------------------------------------------------ settings


@dataclasses.dataclass(frozen=True)
class SandboxLimits:
    """systemd limits for the slot's slice and the all-slots slice."""

    memory_max: str | None = None
    memory_high: str | None = None
    cpu_quota: str | None = None
    tasks_max: int | None = DEFAULT_TASKS_MAX
    io_weight: int | None = None
    all_slots_memory_max: str | None = None
    all_slots_cpu_quota: str | None = None
    # Ordered (slot-name prefix, CPUQuota) overrides; first match wins over
    # cpu_quota, so a host can cap wide lanes without raising every lane.
    slot_cpu_quotas: tuple[tuple[str, str], ...] = ()


@dataclasses.dataclass(frozen=True)
class SandboxSettings:
    """The ``configuration.sandbox`` section; every field can be overridden per run."""

    isolation: str = "userns"
    home: str = "ro"
    home_shared: tuple[str, ...] = DEFAULT_HOME_SHARED
    home_private: tuple[str, ...] = DEFAULT_HOME_PRIVATE
    home_private_files: tuple[str, ...] = DEFAULT_HOME_PRIVATE_FILES
    home_hidden: tuple[str, ...] = DEFAULT_HOME_HIDDEN
    home_expose: tuple[str, ...] = DEFAULT_HOME_EXPOSE
    outputs: tuple[str, ...] = DEFAULT_OUTPUTS
    read_write: tuple[str, ...] = ()
    env: tuple[tuple[str, str], ...] = DEFAULT_ENV
    tmp_size: str = DEFAULT_TMP_SIZE
    protect_system: bool = True
    coordinator_writable: str = "worktrees"
    limits: SandboxLimits = SandboxLimits()


SETTING_KEYS = tuple(field.name for field in dataclasses.fields(SandboxSettings))
LIMIT_KEYS = tuple(field.name for field in dataclasses.fields(SandboxLimits))

#: The comment written above each ``sandbox`` key in a literate configuration.
#: Kept beside the defaults above so documentation and defaults cannot drift.
SETTING_DOCS: dict[str, str] = {
    "isolation": (
        "How `wrkslots run`, `shell-command`, and agent launchers box a command. userns "
        "(default): limits plus the file-system view below, built in an unprivileged user "
        "namespace. root: the same view, built by a short-lived `sudo -n` launcher, dropped "
        "to your uid, gid, and groups, and sealed with no_new_privs before COMMAND runs, so "
        "setuid steps (sudo, a launcher's seteuid) fail inside exactly as under userns; root "
        "exists for tools that refuse user namespaces or need host-visible processes, which "
        "stay reachable through /proc. Needs passwordless sudo. cgroup: per-slot limits only, "
        "no file-system view. Every mode is an accident boundary for cooperative but sloppy "
        "agents — it keeps wayward writes out of the real home directory and the host /tmp — "
        "not a security boundary against a hostile or racing process running as the same "
        "user: the user's systemd bus stays reachable, so `systemd-run --user` without "
        "--scope starts work outside it."
    ),
    "home": (
        "ro (default): $HOME inside the box is this slot's persistent private layer, in "
        "which every real top-level entry is bound read-only (nothing is copied). New "
        "top-level files land in the layer; the real $HOME is never written. hidden: only "
        "the home_expose paths are bound into the layer."
    ),
    "home_shared": (
        "$HOME-relative paths bound read-write from the real $HOME: agent credentials, "
        "settings, and transcripts that must stay shared with the host, and package caches "
        "(.cargo/registry and .cargo/git, which cargo writes when it fetches a crate it has "
        "not cached; ~/.cargo/bin and ~/.cargo/config.toml stay read-only on purpose). A "
        "project that pins its toolchain in rust-toolchain.toml may also need .rustup here "
        "so rustup can install it. Missing paths are skipped. Only these paths are writable; "
        "the rest of ~/.config and ~/.local stays read-only. Note: these directories hold "
        "settings and hook files that the harness later runs OUTSIDE any box, so a boxed "
        "agent that edits them affects unboxed runs."
    ),
    "home_private": (
        "$HOME-relative directories that are private to the slot, persistent, and writable "
        "(kept in the slot's state: the state image, or <control>/slot-state/)."
    ),
    "home_private_files": (
        "Top-level $HOME files copied into the slot's layer once and then private and "
        "writable, for tools that rewrite a state file with a temporary file and a rename."
    ),
    "home_hidden": (
        "$HOME-relative credentials masked inside the box: a directory appears empty, a "
        "file reads as empty."
    ),
    "home_expose": "With home: hidden, the $HOME-relative paths bound in, read-only.",
    "outputs": (
        "Directories, relative to the project root (the primary checkout holding this "
        "file), that a boxed command may write. Missing ones are skipped."
    ),
    "read_write": (
        "Extra absolute paths a boxed command may write. A leading ~ and $USER or $HOME "
        "are expanded, so a per-user path can be written as /var/.../$USER/... Missing "
        "paths are skipped."
    ),
    "env": (
        "Extra environment variables for the boxed command, as NAME: value. A leading ~ in "
        "a value is your $HOME."
    ),
    "tmp_size": (
        f"Size of the fresh /tmp tmpfs each launch gets (discarded at exit), such as "
        f"{DEFAULT_TMP_SIZE} or 25%."
    ),
    "protect_system": (
        "true (default): every mount outside the writable paths above is read-only. false "
        "leaves the rest of the file system writable; $HOME stays read-only either way."
    ),
    "coordinator_writable": (
        "What a coordinator box (`wrkslots box`, `agentctl start --project-box`) may write "
        "besides the paths above. worktrees (default): the managed worktrees directory "
        "(every slot, the registry, slot images and state, validate slots) and the Git "
        "directories the project's slots commit into. project: the whole project root. A "
        "coordinator box can write the registry and every slot by design. A project that "
        "keeps validation logs or other coordinator output outside the worktrees directory "
        "chooses project, or lists those paths in read_write."
    ),
    "limits": (
        "systemd limits for the slot's slice, shared by everything run against the slot. "
        "null means no limit."
    ),
}
LIMIT_DOCS: dict[str, str] = {
    "memory_max": "Slot memory ceiling, such as 32G; swap is disabled when set.",
    "memory_high": "Slot memory throttling threshold, such as 24G.",
    "cpu_quota": "Slot CPU ceiling, such as 800% for eight CPUs.",
    "tasks_max": f"Slot process and thread limit (default {DEFAULT_TASKS_MAX}).",
    "io_weight": "Slot IO weight, 1 to 10000.",
    "all_slots_memory_max": "Memory ceiling shared by all slots together.",
    "all_slots_cpu_quota": "CPU ceiling shared by all slots together.",
    "slot_cpu_quotas": "Ordered per-slot CPU ceilings: a list of {slot_prefix, cpu_quota}; the first prefix matching the slot name wins over cpu_quota.",
}


def _str_tuple(value: object, label: str) -> tuple[str, ...]:
    if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
        raise SandboxError(f"{label} must be a list of strings")
    return tuple(str(item) for item in value)


def _optional_str(value: object, label: str) -> str | None:
    if value is None:
        return None
    if not isinstance(value, str) or not _LIMIT_RE.match(value):
        raise SandboxError(f"{label} must look like 32G, 800%, or infinity, not {value!r}")
    return value


def _optional_int(value: object, label: str) -> int | None:
    if value is None:
        return None
    if not isinstance(value, int) or isinstance(value, bool) or value < 1:
        raise SandboxError(f"{label} must be a positive integer or null")
    return value


def _relative(value: str, label: str) -> str:
    candidate = Path(value)
    if not value or candidate.is_absolute() or ".." in candidate.parts or candidate.as_posix() == ".":
        raise SandboxError(f"{label} entries must be relative paths without '..': {value!r}")
    return candidate.as_posix()


def _relative_list(raw: Mapping[str, object], key: str, default: tuple[str, ...]) -> tuple[str, ...]:
    label = f"configuration.sandbox.{key}"
    values = _str_tuple(raw.get(key, list(default)), label)
    return tuple(dict.fromkeys(_relative(item, label) for item in values))


def _top_level_list(raw: Mapping[str, object], key: str, default: tuple[str, ...]) -> tuple[str, ...]:
    values = _relative_list(raw, key, default)
    for value in values:
        if "/" in value:
            raise SandboxError(f"configuration.sandbox.{key} entries must be top-level $HOME names: {value!r}")
    return values


def _absolute_list(raw: Mapping[str, object], key: str) -> tuple[str, ...]:
    label = f"configuration.sandbox.{key}"
    result: list[str] = []
    for item in _str_tuple(raw.get(key, []), label):
        if not (item.startswith("/") or item == "~" or item.startswith("~/")):
            raise SandboxError(f"{label} entries must be absolute or ~/-relative paths: {item!r}")
        result.append(item)
    return tuple(dict.fromkeys(result))


def _env_pairs(value: object) -> tuple[tuple[str, str], ...]:
    if not isinstance(value, dict):
        raise SandboxError("configuration.sandbox.env must be an object of NAME: value strings")
    pairs: list[tuple[str, str]] = []
    for key, item in value.items():
        if not isinstance(key, str) or not _ENV_NAME_RE.match(key):
            raise SandboxError(f"configuration.sandbox.env has an invalid variable name: {key!r}")
        if not isinstance(item, str) or "\n" in item or "\0" in item:
            raise SandboxError(f"configuration.sandbox.env.{key} must be a one-line string")
        pairs.append((key, item))
    return tuple(pairs)


def _slot_cpu_quotas(value: object) -> tuple[tuple[str, str], ...]:
    label = "configuration.sandbox.limits.slot_cpu_quotas"
    if value is None:
        return ()
    if not isinstance(value, list):
        raise SandboxError(f"{label} must be a list of {{slot_prefix, cpu_quota}} objects")
    result: list[tuple[str, str]] = []
    for item in value:
        if not isinstance(item, dict) or set(item) != {"slot_prefix", "cpu_quota"}:
            raise SandboxError(f"{label} entries must be {{slot_prefix, cpu_quota}} objects")
        prefix = item["slot_prefix"]
        if not isinstance(prefix, str) or not prefix:
            raise SandboxError(f"{label} slot_prefix must be a non-empty string")
        quota = _optional_str(item["cpu_quota"], label + ".cpu_quota")
        if quota is None:
            raise SandboxError(f"{label} cpu_quota must not be null")
        result.append((prefix, quota))
    return tuple(result)


def effective_cpu_quota(limits: SandboxLimits, slot: str | None) -> str | None:
    """The CPU ceiling for one slot: first matching prefix override, else cpu_quota."""

    if slot:
        for prefix, quota in limits.slot_cpu_quotas:
            if slot.startswith(prefix):
                return quota
    return limits.cpu_quota


def limits_from_obj(raw: object) -> SandboxLimits:
    """Validate a ``configuration.sandbox.limits`` mapping."""

    if not isinstance(raw, dict):
        raise SandboxError("configuration.sandbox.limits must be an object")
    unknown = sorted(set(raw) - set(LIMIT_KEYS))
    if unknown:
        raise SandboxError(f"configuration.sandbox.limits has unknown keys: {', '.join(unknown)}")
    defaults = SandboxLimits()
    label = "configuration.sandbox.limits."
    return SandboxLimits(
        memory_max=_optional_str(raw.get("memory_max"), label + "memory_max"),
        memory_high=_optional_str(raw.get("memory_high"), label + "memory_high"),
        cpu_quota=_optional_str(raw.get("cpu_quota"), label + "cpu_quota"),
        tasks_max=_optional_int(raw.get("tasks_max", defaults.tasks_max), label + "tasks_max"),
        io_weight=_optional_int(raw.get("io_weight"), label + "io_weight"),
        all_slots_memory_max=_optional_str(raw.get("all_slots_memory_max"), label + "all_slots_memory_max"),
        all_slots_cpu_quota=_optional_str(raw.get("all_slots_cpu_quota"), label + "all_slots_cpu_quota"),
        slot_cpu_quotas=_slot_cpu_quotas(raw.get("slot_cpu_quotas")),
    )


def settings_from_obj(raw: Mapping[str, object]) -> SandboxSettings:
    """Validate a ``configuration.sandbox`` mapping; absent keys take their defaults."""

    unknown = sorted(set(raw) - set(SETTING_KEYS))
    if unknown:
        hints = [f"{key} (now {_RENAMED_KEYS[key]})" if key in _RENAMED_KEYS else key for key in unknown]
        raise SandboxError(f"configuration.sandbox has unknown keys: {', '.join(hints)}")
    defaults = SandboxSettings()
    isolation = raw.get("isolation", defaults.isolation)
    if isolation not in ISOLATIONS:
        hint = " (namespace was renamed userns)" if isolation == "namespace" else ""
        raise SandboxError(f"configuration.sandbox.isolation must be one of {', '.join(ISOLATIONS)}{hint}")
    home = raw.get("home", defaults.home)
    if home not in HOME_MODES:
        hint = " (use hidden with home_expose)" if home in ("select", "none") else ""
        raise SandboxError(f"configuration.sandbox.home must be one of {', '.join(HOME_MODES)}{hint}")
    protect = raw.get("protect_system", defaults.protect_system)
    if not isinstance(protect, bool):
        raise SandboxError("configuration.sandbox.protect_system must be true or false")
    tmp_size = raw.get("tmp_size", defaults.tmp_size)
    if not isinstance(tmp_size, str) or not _tmp_size_within_cap(tmp_size):
        raise SandboxError(f"configuration.sandbox.tmp_size must look like 16G or 50%, not {tmp_size!r}")
    scope = raw.get("coordinator_writable", defaults.coordinator_writable)
    if scope not in COORDINATOR_SCOPES:
        raise SandboxError(
            f"configuration.sandbox.coordinator_writable must be one of {', '.join(COORDINATOR_SCOPES)}"
        )
    env = _env_pairs(raw["env"]) if "env" in raw else defaults.env
    limits = limits_from_obj(raw["limits"]) if "limits" in raw else defaults.limits
    return SandboxSettings(
        isolation=str(isolation),
        home=str(home),
        home_shared=_relative_list(raw, "home_shared", defaults.home_shared),
        home_private=_relative_list(raw, "home_private", defaults.home_private),
        home_private_files=_top_level_list(raw, "home_private_files", defaults.home_private_files),
        home_hidden=_relative_list(raw, "home_hidden", defaults.home_hidden),
        home_expose=_relative_list(raw, "home_expose", defaults.home_expose),
        outputs=_relative_list(raw, "outputs", defaults.outputs),
        read_write=_absolute_list(raw, "read_write"),
        env=env,
        tmp_size=tmp_size,
        protect_system=protect,
        coordinator_writable=str(scope),
        limits=limits,
    )


def settings_to_obj(settings: SandboxSettings) -> dict[str, object]:
    """The full JSON form of ``settings``, every key spelled out."""

    return {
        "isolation": settings.isolation,
        "home": settings.home,
        "home_shared": list(settings.home_shared),
        "home_private": list(settings.home_private),
        "home_private_files": list(settings.home_private_files),
        "home_hidden": list(settings.home_hidden),
        "home_expose": list(settings.home_expose),
        "outputs": list(settings.outputs),
        "read_write": list(settings.read_write),
        "env": dict(settings.env),
        "tmp_size": settings.tmp_size,
        "protect_system": settings.protect_system,
        "coordinator_writable": settings.coordinator_writable,
        "limits": {
            key: (
                [
                    {"slot_prefix": prefix, "cpu_quota": quota}
                    for prefix, quota in settings.limits.slot_cpu_quotas
                ]
                if key == "slot_cpu_quotas"
                else getattr(settings.limits, key)
            )
            for key in LIMIT_KEYS
        },
    }


def default_config_obj() -> dict[str, object]:
    """The ``sandbox`` section ``wrkslots init`` writes for a new project."""

    return settings_to_obj(SandboxSettings())


def merge_defaults(existing: Mapping[str, object]) -> tuple[dict[str, object], list[str]]:
    """Add every missing default key to an existing sandbox section.

    Present keys are never changed (a present ``env`` or ``limits`` object gets
    only its missing limit keys: ``env`` is a value, ``limits`` a record).
    Returns the merged section and the dotted names that were added.
    """

    merged = dict(existing)
    added: list[str] = []
    for key, value in default_config_obj().items():
        if key not in merged:
            merged[key] = value
            added.append(key)
        elif key == "limits" and isinstance(merged[key], dict) and isinstance(value, dict):
            current = merged[key]
            assert isinstance(current, dict)
            limits = {str(name): item for name, item in current.items()}
            for limit_key, limit_value in value.items():
                if limit_key not in limits:
                    limits[limit_key] = limit_value
                    added.append(f"limits.{limit_key}")
            merged[key] = limits
    settings_from_obj(merged)
    return merged, added


def validate_limit(value: str | None, label: str) -> str | None:
    """Validate one systemd limit value such as 32G or 800%."""

    return _optional_str(value, label)


@dataclasses.dataclass(frozen=True)
class SlotView:
    """Everything about a slot that the sandbox must make visible or writable."""

    slot: str
    slot_type: str
    slot_path: Path
    state_directory: Path
    git_directories: tuple[Path, ...]
    control_directory: Path
    representation: str
    project_root: Path | None = None
    #: A coordinator box (``wrkslots box``) rather than one slot's box: the
    #: registry and slots are writable, and ``slot`` is the box name.
    coordinator: bool = False
    #: Extra writable roots (a coordinator box's scope and Git directories).
    writable: tuple[Path, ...] = ()
    #: Roots whose mounts made on the host after the box starts (image slots)
    #: appear inside it; everything else is cut off from host mount events.
    propagate: tuple[Path, ...] = ()
    #: An Eden slot's checkout mount, or None for a non-Eden slot. Its
    #: presence turns on the Eden view: the shared daemon config directories,
    #: other slots' Eden state and the host backing store are masked, and
    #: the root-owned ancestors of the checkout and backing are covered so
    #: Sapling's shared-directory ownership check sees a user-owned root.
    eden_checkout: Path | None = None
    #: The Eden checkout's host backing store, masked in the view.
    eden_backing: Path | None = None
    #: Other slots' ``.eden-state`` directories, masked in the view.
    eden_hidden: tuple[Path, ...] = ()
    #: Absolute path at which the Eden checkout is also bound inside the
    #: box (a slot-view setting chosen by the caller, never a configured
    #: default). The mount point must already exist on the host as an empty
    #: directory, except under the box's fresh ``/tmp`` where the view
    #: creates it in its own tmpfs.
    checkout_alias: Path | None = None


# ------------------------------------------------------------------ slices


def _timeout(deadline_ns: int | None, what: str) -> float | None:
    """Seconds left before ``deadline_ns`` (CLOCK_MONOTONIC), or None for no deadline.

    A deadline that has already passed refuses the step instead of starting it.
    """

    if deadline_ns is None:
        return None
    remaining = deadline_ns - time.monotonic_ns()
    if remaining <= 0:
        raise SandboxError(f"the deadline passed before {what}")
    return remaining / 1_000_000_000


def _deadline_refusal(what: str) -> SandboxError:
    return SandboxError(f"the deadline passed during {what}; it was stopped")


def systemd_escape(value: str, *, deadline_ns: int | None = None) -> str:
    """Escape a string for use inside a systemd unit name."""

    what = f"systemd-escape {value!r}"
    try:
        result = subprocess.run(
            ["systemd-escape", "--", value],
            capture_output=True,
            text=True,
            check=False,
            timeout=_timeout(deadline_ns, what),
        )
    except subprocess.TimeoutExpired as exc:
        raise _deadline_refusal(what) from exc
    if result.returncode != 0:
        raise SandboxError(f"systemd-escape failed for {value!r}: {result.stderr.strip()}")
    return result.stdout.strip()


def parent_slice(cgroup_file: Path = Path("/proc/self/cgroup")) -> str | None:
    """The innermost user-manager slice enclosing the caller, if any.

    wrkslots never moves a process OUT of an enclosing slice: a caller that site
    policy placed in a sandbox slice (an agent harness's jail, for example) must
    stay inside it, because monitoring and limits are scoped to that cgroup. The
    slot's slices are therefore created as children of the caller's slice.
    """

    try:
        text = cgroup_file.read_text(encoding="utf-8")
    except OSError:
        return None
    for line in text.splitlines():
        if not line.startswith("0::"):
            continue
        parts = line[3:].split("/")
        if "user@" not in line:
            return None
        below = parts[next(i for i, part in enumerate(parts) if part.startswith("user@")) + 1 :]
        slices = [part for part in below if part.endswith(".slice")]
        return slices[-1] if slices else None
    return None


#: ``parent`` value meaning "the caller is in no slice of the user manager",
#: decided by the caller. ``None`` instead means "look at /proc/self/cgroup now",
#: which inside a scope the caller has just joined would find a different answer.
NO_ENCLOSING_SLICE = ""


def root_slice(parent: str | None = None) -> str:
    """The slice holding every slot's slice (the all-slots guard).

    ``parent`` is the enclosing slice, :data:`NO_ENCLOSING_SLICE` for none, or
    None to rediscover it with :func:`parent_slice`.
    """

    enclosing = parent if parent is not None else parent_slice()
    prefix = "" if not enclosing else enclosing.removesuffix(".slice") + "-"
    return f"{prefix}wrkslots.slice"


def slice_name(
    slot_type: str, slot: str, parent: str | None = None, *, deadline_ns: int | None = None
) -> str:
    """The slice for one slot, nested under the caller's slice unless ``parent`` is given."""

    # A dash in a slice name means nesting, so the slot name is escaped: slot
    # "kvm-foo" becomes one child of the wrkslots slice, not three levels.
    if slot_type == "box":
        # Coordinator boxes nest under <root>-box.slice, apart from the slots.
        escaped = systemd_escape(slot, deadline_ns=deadline_ns)
        return root_slice(parent).removesuffix(".slice") + f"-box-{escaped}.slice"
    label = slot if slot_type == "agent" else f"{slot_type}:{slot}"
    escaped = systemd_escape(label, deadline_ns=deadline_ns)
    return root_slice(parent).removesuffix(".slice") + f"-{escaped}.slice"


def slice_limit_properties(limits: SandboxLimits, slot: str | None = None) -> list[str]:
    """systemd properties for the per-slot slice."""

    properties: list[str] = []
    if limits.memory_max:
        properties.append(f"MemoryMax={limits.memory_max}")
        properties.append("MemorySwapMax=0")
    if limits.memory_high:
        properties.append(f"MemoryHigh={limits.memory_high}")
    cpu_quota = effective_cpu_quota(limits, slot)
    if cpu_quota:
        properties.append(f"CPUQuota={cpu_quota}")
    if limits.tasks_max:
        properties.append(f"TasksMax={limits.tasks_max}")
    if limits.io_weight:
        properties.append(f"IOWeight={limits.io_weight}")
    return properties


#: Transient user-manager failures (#1765 wrkslots-load-sensitive-gate-tests):
#: the manager is contended, not refusing, so setting limits waits on the
#: slice's own state and tries again. Anything else fails at once.
_TRANSIENT_MANAGER_MARKERS = (
    "NoReply",
    "timed out",
    "is destructive",
    "Connection reset by peer",
    "Resource temporarily unavailable",
)

#: Generous backstop for the slice-limit condition wait (#1765); it bounds a
#: wedged manager, never decides the verdict on a working one.
_SLICE_LIMIT_BACKSTOP_SECONDS = 180.0
_SLICE_LIMIT_POLL_SECONDS = 0.25


def _is_transient_manager_error(text: str) -> bool:
    return any(marker in text for marker in _TRANSIENT_MANAGER_MARKERS)


def _slice_loaded(unit: str, timeout: float | None) -> bool:
    """The retry condition: the user manager has the slice loaded."""

    try:
        shown = subprocess.run(
            ["systemctl", "--user", "show", unit, "--property=LoadState"],
            capture_output=True, text=True, check=False, timeout=timeout,
        )
    except (OSError, subprocess.TimeoutExpired):
        return False
    return shown.returncode == 0 and "LoadState=loaded" in shown.stdout


def _set_slice_limits(unit: str, properties: list[str], deadline_ns: int | None) -> None:
    """Start one slice and set its properties, retrying transient refusals.

    Between tries the wait is the slice's LoadState (re-checked with
    ``systemctl show``), never a try count. The backstop raises the usual
    refusal carrying the last manager error and the number of tries.
    """

    what = f"setting limits on {unit}"
    backstop_ns = time.monotonic_ns() + int(_SLICE_LIMIT_BACKSTOP_SECONDS * 1_000_000_000)
    limit_ns = backstop_ns if deadline_ns is None else min(deadline_ns, backstop_ns)
    tries = 0
    last_error = ""
    while True:
        tries += 1
        if time.monotonic_ns() >= limit_ns:
            raise SandboxError(
                f"cannot set limits on {unit} after {tries - 1} tries: {last_error}"
            )
        timeout = (limit_ns - time.monotonic_ns()) / 1_000_000_000
        try:
            # A slice has to be loaded before its properties can be set;
            # starting it is idempotent and leaves no process behind.
            subprocess.run(
                ["systemctl", "--user", "start", unit],
                capture_output=True, check=False, timeout=timeout,
            )
            result = subprocess.run(
                ["systemctl", "--user", "set-property", "--runtime", unit, *properties],
                capture_output=True, text=True, check=False, timeout=timeout,
            )
        except subprocess.TimeoutExpired as exc:
            if deadline_ns is not None and time.monotonic_ns() >= deadline_ns:
                raise _deadline_refusal(what) from exc
            result = None
            last_error = "systemctl timed out"
        if result is not None and result.returncode == 0:
            return
        if result is not None:
            last_error = result.stderr.strip()
            if not _is_transient_manager_error(last_error):
                raise SandboxError(f"cannot set limits on {unit}: {last_error}")
        # Transient: wait on the condition (the slice loaded), then retry.
        while True:
            if time.monotonic_ns() >= limit_ns:
                raise SandboxError(
                    f"cannot set limits on {unit} after {tries} tries: {last_error}"
                )
            remaining = (limit_ns - time.monotonic_ns()) / 1_000_000_000
            if _slice_loaded(unit, max(0.05, min(remaining, 5.0))):
                break
            time.sleep(_SLICE_LIMIT_POLL_SECONDS)
        if time.monotonic_ns() >= limit_ns:
            raise SandboxError(
                f"cannot set limits on {unit} after {tries} tries: {last_error}"
            )
        print(
            f"wrkslots: retrying limits on {unit} (try {tries + 1}): {last_error}",
            file=sys.stderr,
        )
        # Back off (0.25 s doubling to a 5 s cap, plus jitter) so many
        # steps do not hammer a contended manager in lockstep (#1765).
        delay = min(5.0, _SLICE_LIMIT_POLL_SECONDS * float(2 ** min(tries - 1, 5)))
        time.sleep(delay + random.uniform(0.0, delay * 0.25))


def apply_slice_limits(
    view: SlotView,
    limits: SandboxLimits,
    *,
    parent: str | None = None,
    deadline_ns: int | None = None,
) -> list[str]:
    """Set runtime limits on the slot's slice (and the all-slots slice).

    ``parent`` names the enclosing slice exactly as :func:`root_slice` takes it
    (None rediscovers it). ``deadline_ns`` (CLOCK_MONOTONIC) bounds every
    systemctl call; a call still running at the deadline is killed and refused.
    """

    applied: list[str] = []
    root_limits: list[str] = []
    if limits.all_slots_memory_max:
        root_limits += [f"MemoryMax={limits.all_slots_memory_max}", "MemorySwapMax=0"]
    if limits.all_slots_cpu_quota:
        root_limits.append(f"CPUQuota={limits.all_slots_cpu_quota}")
    for unit, properties in (
        (root_slice(parent), root_limits),
        (
            slice_name(view.slot_type, view.slot, parent, deadline_ns=deadline_ns),
            slice_limit_properties(limits, view.slot),
        ),
    ):
        if not properties:
            continue
        _set_slice_limits(unit, properties, deadline_ns)
        applied.append(f"{unit}: {' '.join(properties)}")
    return applied


@dataclasses.dataclass(frozen=True)
class SlicePlacement:
    """The three slice names a caller places its own scope under, captured once."""

    #: The caller's innermost user-manager slice, or None when it is in none.
    enclosing: str | None
    #: The all-slots slice (aggregate limits), a child of ``enclosing``.
    all_slots: str
    #: This slot's slice (per-slot limits), a child of ``all_slots``.
    slot: str

    def to_obj(self) -> dict[str, str | None]:
        """The JSON form ``run --place-only --format json`` prints and ``--expect-slices`` takes."""

        return {"enclosing": self.enclosing, "all_slots": self.all_slots, "slot": self.slot}


def place_only(
    view: SlotView,
    limits: SandboxLimits,
    *,
    deadline_ns: int | None = None,
    cgroup_file: Path = Path("/proc/self/cgroup"),
) -> tuple[SlicePlacement, list[str]]:
    """Apply the slot's slice limits and return the slice names, joining nothing.

    For a caller that creates its own scope (``systemd-run --scope
    --slice=SLOT``) and later enters the box from inside it. The caller's slice
    is read ONCE, here, outside any scope; the all-slots and slot slices are
    derived from that one answer and the limits are applied to exactly those
    names, so nothing is rediscovered from a cgroup the caller has since moved
    into. Returns the placement and the limit lines applied.
    """

    enclosing = parent_slice(cgroup_file)
    parent = NO_ENCLOSING_SLICE if enclosing is None else enclosing
    placement = SlicePlacement(
        enclosing=enclosing,
        all_slots=root_slice(parent),
        slot=slice_name(view.slot_type, view.slot, parent, deadline_ns=deadline_ns),
    )
    applied = apply_slice_limits(view, limits, parent=parent, deadline_ns=deadline_ns)
    return placement, applied


# ------------------------------------------------------------ the view spec


def _mount_rows(mountinfo: Path = Path("/proc/self/mountinfo")) -> list[tuple[str, str, Path]]:
    rows: list[tuple[str, str, Path]] = []
    for line in mountinfo.read_text(encoding="utf-8", errors="surrogateescape").splitlines():
        fields = line.split(" - ", 1)[0].split()
        if len(fields) >= 5:
            rows.append((fields[2], _unescape(fields[3]), Path(_unescape(fields[4]))))
    return rows


def _unescape(value: str) -> str:
    return value.replace("\\040", " ").replace("\\011", "\t").replace("\\012", "\n").replace("\\134", "\\")


def path_aliases(path: Path) -> list[Path]:
    """Return ``path`` plus other mount paths that expose the same directory.

    On hosts where ``$HOME`` is a bind mount of a subdirectory of a larger file
    system, the same files are also reachable through that file system's own
    mount point. Protecting or hiding ``$HOME`` without its alias would protect
    or hide nothing.
    """

    aliases = [path]
    try:
        rows = _mount_rows()
    except OSError:
        return aliases
    home_rows = [row for row in rows if row[2] == path]
    if not home_rows:
        return aliases
    device, root, _mount_point = home_rows[-1]
    if root == "/":
        return aliases
    for other_device, other_root, other_mount in rows:
        if other_device != device or other_mount == path:
            continue
        # Any mount of the same file system whose root is an ancestor of
        # $HOME's root exposes $HOME below it.
        if other_root == "/" or root.startswith(other_root.rstrip("/") + "/"):
            relative = root[len(other_root.rstrip("/")) :].lstrip("/")
            alias = other_mount / relative
            if alias.is_dir() and alias not in aliases:
                aliases.append(alias)
    return aliases


def forwarded_environment(environ: Mapping[str, str], *, private_tmp: bool) -> dict[str, str]:
    """The caller's environment minus service-manager (and, with a private /tmp, temp) variables."""

    drop = _ENV_DENYLIST | (frozenset(_TEMP_VARIABLES) if private_tmp else frozenset())
    return {
        key: value
        for key, value in environ.items()
        if key not in drop and "=" not in key and "\n" not in value and "\0" not in value
    }


_COORDINATOR_ENV_KEYS = ("WRKSLOTS_BOX", "WRKSLOTS_BOX_WRITABLE", "WRKSLOTS_PROJECT_ROOT")
_SLOT_ENV_KEYS = ("WRKSLOTS_SLOT", "WRKSLOTS_SLOT_TYPE", "WRKSLOTS_SLOT_PATH", "WRKSLOTS_SLOT_REPRESENTATION")


def child_environment(view: SlotView, settings: SandboxSettings, environ: Mapping[str, str]) -> dict[str, str]:
    """The complete environment COMMAND starts with."""

    private_tmp = settings.isolation != "cgroup"
    environment = forwarded_environment(environ, private_tmp=private_tmp)
    home = environ.get("HOME", str(Path.home()))
    if private_tmp:
        environment["TMPDIR"] = "/tmp"
    environment["WRKSLOTS_SANDBOX"] = settings.isolation
    # A box launched from inside another box must describe only itself: a slot
    # box started from a coordinator box must not inherit WRKSLOTS_BOX (which
    # would make it look like a coordinator box to image commands), and a
    # coordinator box must not carry a slot identity.
    for key in (*_COORDINATOR_ENV_KEYS, *_SLOT_ENV_KEYS):
        environment.pop(key, None)
    if view.coordinator:
        environment.update(
            {
                "WRKSLOTS_BOX": view.slot,
                "WRKSLOTS_BOX_WRITABLE": view.representation,
                "WRKSLOTS_PROJECT_ROOT": str(view.project_root or ""),
            }
        )
    else:
        environment.update(
            {
                "WRKSLOTS_SLOT": view.slot,
                "WRKSLOTS_SLOT_TYPE": view.slot_type,
                "WRKSLOTS_SLOT_PATH": str(view.slot_path),
                "WRKSLOTS_SLOT_REPRESENTATION": view.representation,
            }
        )
    for key, value in settings.env:
        environment[key] = _expand_home(value, home)
    return environment


def _expand_home(value: str, home: str) -> str:
    if value == "~":
        return home
    if value.startswith("~/"):
        return home.rstrip("/") + value[1:]
    return value


def expand_path(value: str, home: str, user: str) -> str:
    """Expand a leading ``~`` and ``$USER`` / ``${USER}`` / ``$HOME`` / ``${HOME}``.

    Only these, so a project configuration means the same thing whatever else
    is in the caller's environment.
    """

    value = _expand_home(value, home)
    for name, replacement in (("USER", user), ("HOME", home)):
        value = value.replace("${" + name + "}", replacement)
        value = re.sub(r"\$" + name + r"(?![A-Za-z0-9_])", lambda _match: replacement, value)
    return value


def home_layer(view: SlotView) -> Path:
    """The slot's persistent private layer, mounted over ``$HOME`` inside the box."""

    return view.state_directory / "home"


def private_home_directory(view: SlotView, relative: str) -> Path:
    """Where the slot keeps its private ``$HOME/<relative>``: inside its layer."""

    return home_layer(view) / relative


def home_entries(settings: SandboxSettings, home: Path) -> list[tuple[str, str]]:
    """The real ``$HOME`` entries bound read-only over the layer, as (relative path, kind).

    ``home: ro`` binds every top-level entry except private directories and
    private files (kind ``dir`` or ``file``; symbolic links are recreated in the
    layer, kind ``link``). ``home: hidden`` binds only ``home_expose`` paths.
    Sockets and other special files are left out.
    """

    if settings.home == "ro":
        private_tops = {relative for relative in settings.home_private if "/" not in relative}
        skip = private_tops | set(settings.home_private_files)
        try:
            names = sorted(os.listdir(home))
        except OSError as exc:
            raise SandboxError(f"cannot list $HOME {home}: {exc}") from exc
        candidates = [name for name in names if name not in skip]
    else:
        candidates = list(settings.home_expose)
    entries: list[tuple[str, str]] = []
    for relative in candidates:
        path = home / relative
        if path.is_symlink():
            if "/" not in relative:
                entries.append((relative, "link"))
            elif path.exists():
                entries.append((relative, "dir" if path.is_dir() else "file"))
        elif path.is_dir():
            entries.append((relative, "dir"))
        elif path.is_file():
            entries.append((relative, "file"))
    return entries


_PLACEHOLDERS = "home-placeholders.json"


def prepare_state(view: SlotView, settings: SandboxSettings, home: Path) -> None:
    """Build the slot's private $HOME layer: placeholders, seeded files, private directories.

    Runs as the invoking user before any namespace exists, so everything it
    creates is owned by that user in every isolation mode, and nothing is
    created in the real $HOME. Each real entry the view binds gets an empty
    placeholder of the same kind (a mount point) and each real symbolic link is
    recreated; placeholders and links this function created for entries that
    are no longer bound (gone from $HOME, or a different ``home`` mode) are
    removed, placeholders only while still empty.
    """

    layer = home_layer(view)
    layer.mkdir(parents=True, exist_ok=True, mode=0o700)
    record = view.state_directory / _PLACEHOLDERS
    try:
        loaded = json.loads(record.read_text(encoding="utf-8"))
        previous = {str(item) for item in loaded} if isinstance(loaded, list) else set()
    except (OSError, ValueError):
        previous = set()
    created: set[str] = set()

    def placeholder(relative: str, kind: str) -> None:
        target = layer / relative
        if os.path.lexists(target):
            if (kind == "dir") != (target.is_dir() and not target.is_symlink()):
                print(f"wrkslots run: {target} is in the way of a {kind} mount point", file=sys.stderr)
            elif relative in previous:
                created.add(relative)
            return
        target.parent.mkdir(parents=True, exist_ok=True)
        if kind == "dir":
            target.mkdir(mode=0o700)
        else:
            target.touch(mode=0o600)
        created.add(relative)

    for relative, kind in home_entries(settings, home):
        if kind == "link":
            target = layer / relative
            if not os.path.lexists(target):
                os.symlink(os.readlink(home / relative), target)
                created.add(relative)
            elif relative in previous:
                created.add(relative)
        else:
            placeholder(relative, kind)
    for name in settings.home_private_files:
        target, real = layer / name, home / name
        if not os.path.lexists(target) and real.is_file():
            shutil.copy2(real, target)
            os.chmod(target, 0o600)
    for relative in settings.home_private:
        private_home_directory(view, relative).mkdir(parents=True, exist_ok=True, mode=0o700)
    if settings.home == "hidden":
        # Mount points for shared paths; in "ro" mode they exist in the real tree.
        for relative in settings.home_shared:
            real = home / relative
            if real.exists():
                placeholder(relative, "dir" if real.is_dir() else "file")
    for relative in sorted(previous - created, key=len, reverse=True):
        stale = layer / relative
        try:
            if stale.is_symlink():
                stale.unlink()
            elif stale.is_dir():
                stale.rmdir()
            elif stale.is_file() and stale.stat().st_size == 0:
                stale.unlink()
        except OSError:
            created.add(relative)  # not empty: the agent's own content; keep tracking
    temporary = record.with_name(f"{record.name}.tmp.{os.getpid()}")
    temporary.write_text(json.dumps(sorted(created)), encoding="utf-8")
    os.replace(temporary, record)
    # Earlier drafts kept a persistent per-slot /tmp; /tmp is per launch now.
    legacy_tmp = view.state_directory / "tmp"
    if legacy_tmp.is_dir() and not legacy_tmp.is_symlink():
        shutil.rmtree(legacy_tmp, ignore_errors=True)


#: $HOME-relative shared Eden daemon config directories: every host
#: checkout is listed through them, so an Eden slot's box masks them and a
#: lane sees only its own per-slot state directory.
EDEN_SHARED_CONFIG_DIRS = ("local/.eden", ".eden")


def _checkout_alias_bind(view: SlotView, settings: SandboxSettings) -> tuple[Path, Path] | None:
    """Validate ``view.checkout_alias``; return its (checkout, target) bind.

    The alias target is a caller-chosen absolute path. It must already be an
    empty directory on the host -- a one-time host-side creation that only
    the host owner can make at a root-owned location -- except below the
    box's fresh ``/tmp``, where the view creates the mount point in its own
    tmpfs. A missing host mount point is refused naming the command that
    creates it; the box never creates a directory on the host root.
    """

    target = view.checkout_alias
    if target is None:
        return None
    if view.eden_checkout is None:
        raise SandboxError("--checkout-alias binds an Eden checkout; this slot has none")
    if settings.isolation == "cgroup":
        raise SandboxError("--checkout-alias needs a file-system view; isolation cgroup has none")
    if not target.is_absolute() or os.path.normpath(target) != str(target) or str(target) == "/":
        raise SandboxError(f"--checkout-alias must be an absolute path below /, not {target}")
    checkout = view.eden_checkout
    if target == checkout or checkout in target.parents or target in checkout.parents:
        raise SandboxError(f"--checkout-alias {target} must not overlap the Eden checkout {checkout}")
    if target == view.slot_path or view.slot_path in target.parents:
        raise SandboxError(f"--checkout-alias {target} must not overlap the slot {view.slot_path}")
    if target.exists() or target.is_symlink():
        if target.is_symlink() or not target.is_dir():
            raise SandboxError(f"--checkout-alias {target} is not a real directory; remove it and retry")
        if any(target.iterdir()):
            raise SandboxError(f"--checkout-alias {target} is not empty; the alias bind needs an empty mount point")
    elif target.parent != Path("/tmp"):
        raise SandboxError(
            f"--checkout-alias {target} does not exist: create the mount point once on the host "
            f"with `sudo mkdir {target}` (an empty root-owned directory), then retry"
        )
    return checkout, target


def build_spec(
    view: SlotView,
    settings: SandboxSettings,
    home: Path,
    cwd: Path,
    *,
    slot_writable: bool = True,
    git_config_policy: str = "inherit",
) -> dict[str, object]:
    """Describe the view the helper must build, as plain JSON.

    Pure except for existence checks, so it is shared by the ``userns`` and
    ``root`` modes and by ``--print``. ``slot_writable=False`` (a closed set)
    leaves the slot, the Git directories its checkouts commit into, and the
    view's extra writable roots read-only; only the configured lists remain.
    ``git_config_policy="isolated"`` appends the fixed Git configuration
    locations (:func:`git_config_mask_paths`) to the masks, each once, and
    names them in ``git_config_masks`` so the helper masks them strictly
    (:func:`_mask_git_config`); the default ``inherit`` leaves the spec as it was.
    """

    if home == Path("/") or not home.is_absolute():
        raise SandboxError(f"refusing to box a command with HOME={home}")
    user = os.environ.get("USER") or home.name
    aliases = path_aliases(home)
    binds: list[list[str]] = []

    def add(source: Path, target: Path) -> None:
        pair = [str(source), str(target)]
        if pair not in binds:
            binds.append(pair)

    for relative in settings.home_shared:
        path = home / relative
        if path.exists():
            add(path, path)
    for relative in settings.home_private:
        if "/" in relative and (settings.home == "hidden" or (home / relative).is_dir()):
            # Top-level private directories are simply part of the layer.
            add(private_home_directory(view, relative), home / relative)
    if slot_writable:
        add(view.slot_path, view.slot_path)
        if view.eden_checkout is not None and not view.eden_checkout.is_relative_to(
            view.slot_path
        ):
            # An Eden mount outside the slot (#1730) is not under the
            # slot bind; bind it at its own recorded path.
            add(view.eden_checkout, view.eden_checkout)
        for directory in (*view.git_directories, *view.writable):
            add(directory, directory)
    alias = _checkout_alias_bind(view, settings)
    if alias is not None:
        if not slot_writable:
            raise SandboxError("--checkout-alias binds the checkout writable and refuses a closed set")
        add(alias[0], alias[1])
    # A slot's box never writes the wrkslots control directory (registry,
    # journals, other slots, slot images, private HOME layers): registry commands
    # such as heartbeat, finish, and write-handoff run outside it. A coordinator
    # box is the one exception: its scope includes the control directory.
    if view.project_root is not None:
        for relative in settings.outputs:
            path = view.project_root / relative
            if path.is_dir():
                add(path, path)
    for item in settings.read_write:
        path = Path(expand_path(item, str(home), user))
        if path.exists():
            add(path, path)
    # Gate admission (#1205): a gate launcher that exports $GATE_ADMISSION_DIR
    # gets that queue bound writable; unexported, the box's fresh /tmp stays
    # untouched and no host /tmp path leaks into the view.
    gate = gate_admission_dir()
    if gate is not None and gate.exists():
        add(gate, gate)
    entries = home_entries(settings, home)
    read_only = [] if view.coordinator else [str(view.control_directory)]
    runtime = spec_directory()
    if runtime is not None:
        # Create it now: the view makes it read-only only if it exists, and a
        # box started before any root launch must not be able to create (and
        # then plant files in) the root launcher's spec directory itself.
        ensure_spec_directory(runtime)
        read_only.append(str(runtime))
    masks = [str(base / relative) for base in aliases for relative in settings.home_hidden]
    # Other slots' and boxes' private HOME layers (which hold private copies of
    # credential files), and in a slot's box the image directory too. This box's
    # own layer is bound over $HOME beforehand. A coordinator box keeps
    # slot-images visible: it creates and removes image slots.
    private_state = ("slot-state", "box-state") if view.coordinator else ("slot-images", "slot-state", "box-state")
    masks += [str(view.control_directory / name) for name in private_state]
    if view.eden_checkout is not None:
        # A boxed Eden lane sees only its own checkout and state: the shared
        # host daemon config directories name every other host checkout, and
        # the host backing store's Sapling store is readable through the
        # read-only $HOME bind, so all of them are masked. This slot's own
        # per-slot state stays visible below its slot bind.
        for relative in EDEN_SHARED_CONFIG_DIRS:
            masks += [str(base / relative) for base in aliases]
        masks += [str(path) for path in view.eden_hidden]
        if view.eden_backing is not None:
            masks.append(str(view.eden_backing))
    policy: dict[str, object] = {}
    if git_config_policy == "isolated":
        git_masks = git_config_mask_paths(aliases)
        masks += [path for path in git_masks if path not in masks]
        policy = {"git_config_policy": "isolated", "git_config_masks": git_masks}
    elif git_config_policy != "inherit":
        raise SandboxError(
            f"--git-config-policy must be one of {', '.join(GIT_CONFIG_POLICIES)}, not {git_config_policy!r}"
        )
    return {
        "home": str(home),
        "home_aliases": [str(alias) for alias in aliases[1:]],
        "home_mode": settings.home,
        "home_layer": str(home_layer(view)),
        "home_binds": [relative for relative, kind in entries if kind != "link"],
        "read_only": read_only,
        "binds": binds,
        "masks": masks,
        "tmp_size": settings.tmp_size,
        "protect_system": settings.protect_system,
        "propagate": [str(path) for path in view.propagate],
        "cwd": str(cwd),
        "eden_root_cover": view.eden_checkout is not None,
        "eden_checkout": str(view.eden_checkout) if view.eden_checkout is not None else None,
        "eden_backing": str(view.eden_backing) if view.eden_backing is not None else None,
        **policy,
    }


def gate_admission_dir(environ: Mapping[str, str] | None = None) -> Path | None:
    """The gate-admission directory to bind writable, when explicitly exported.

    Only an explicitly set $GATE_ADMISSION_DIR counts (#1205): defaulting to
    the gate's usual /tmp path would bind a host /tmp subdir into every box,
    breaking the fresh-/tmp contract. A gate launcher exports the variable;
    the gate itself resolves the same variable, defaulting on its side.
    """

    env = os.environ if environ is None else environ
    value = env.get("GATE_ADMISSION_DIR")
    return Path(value) if value else None


def deny_log_command(
    command: Sequence[str],
    log_path: Path,
    *,
    enabled: bool,
    strace: str | None,
) -> list[str]:
    """Wrap ``command`` for log-only denied-write tracing (#1205).

    With $WRKSLOTS_DENY_LOG set and strace present, failed file syscalls
    (EROFS among them) are appended to ``log_path``; tracing never blocks or
    alters the command. Without strace the command runs untraced.
    """

    if not enabled or strace is None or not command:
        return list(command)
    return [
        strace,
        "-f",
        "-qq",
        "-e",
        "trace=%file",
        "-e",
        "status=failed",
        "-o",
        str(log_path),
        "--",
        *command,
    ]


def git_config_mask_paths(aliases: Sequence[Path]) -> list[str]:
    """The isolated policy's fixed locations, in order: each $HOME path's, then the system file.

    ``aliases`` is $HOME followed by the other mount paths that expose it
    (:func:`path_aliases`); every one gets both $HOME-relative locations.
    """

    paths = [str(base / relative) for base in aliases for relative in GIT_CONFIG_HOME_PATHS]
    paths += list(GIT_CONFIG_SYSTEM_PATHS)
    return [os.path.normpath(path) for path in dict.fromkeys(paths)]


def mask_report(spec: Mapping[str, object]) -> dict[str, object]:
    """The masks a view described by ``spec`` applies, as ``--print`` reports them.

    ``masks`` is the complete ordered list the helper applies (credentials,
    private state, and any policy paths); ``git_config_policy`` and
    ``git_config_masks`` say which of them the isolated Git configuration
    policy masks strictly. ``--expect-masks`` compares this whole object.
    """

    masks = spec.get("masks", [])
    git_masks = spec.get("git_config_masks", [])
    assert isinstance(masks, list) and isinstance(git_masks, list)
    return {
        "masks": [str(path) for path in masks],
        "git_config_policy": str(spec.get("git_config_policy", "inherit")),
        "git_config_masks": [str(path) for path in git_masks],
    }


def writable_set(spec: Mapping[str, object], *, created_sources: Sequence[Path] = ()) -> dict[str, object]:
    """Every place a command in the view described by ``spec`` can write.

    Derived from the same spec the helper builds, following :func:`build_view`:
    the private layer mounted over ``$HOME``, each writable bind (source on the
    host, target in the view), the fresh ``/tmp`` tmpfs, and, with
    ``protect_system``, the kernel and runtime trees left writable; without it,
    every mount (``/``). Bind sources that do not exist are left out, as the
    helper skips them, except ``created_sources``: directories that
    :func:`prepare_state` creates before the helper runs (see
    :func:`planned_private_sources`), so a report made before preparing the
    state names the binds the real entry will have.
    """

    raw_binds = spec.get("binds", [])
    assert isinstance(raw_binds, list)
    protect = bool(spec.get("protect_system", True))
    created = {str(path) for path in created_sources}
    return {
        "home_layer": {"source": str(spec["home_layer"]), "target": str(spec["home"])},
        "binds": [
            {"source": str(pair[0]), "target": str(pair[1])}
            for pair in raw_binds
            if isinstance(pair, list)
            and len(pair) == 2
            and (os.path.exists(str(pair[0])) or str(pair[0]) in created)
        ],
        "tmp": {"target": "/tmp", "size": str(spec.get("tmp_size", DEFAULT_TMP_SIZE))},
        "protect_system": protect,
        "always_writable": list(_ALWAYS_WRITABLE_PREFIXES) if protect else ["/"],
    }


def planned_private_sources(view: SlotView, settings: SandboxSettings) -> tuple[Path, ...]:
    """The bind sources :func:`prepare_state` creates: every ``home_private`` directory in the layer.

    Only these; an arbitrary missing ``read_write`` or shared path is not created
    by anything and stays skipped.
    """

    return tuple(private_home_directory(view, relative) for relative in settings.home_private)


# ---------------------------------------------------------- building the view

_CLONE_NEWNS = 0x00020000
_CLONE_NEWUSER = 0x10000000
_MS_RDONLY = 1
_MS_NOSUID = 2
_MS_NODEV = 4
_MS_NOEXEC = 8
_MS_REMOUNT = 32
_MS_NOATIME = 1024
_MS_NODIRATIME = 2048
_MS_BIND = 4096
_MS_REC = 16384
_MS_PRIVATE = 1 << 18
_MS_SLAVE = 1 << 19
_MS_RELATIME = 1 << 21
_MS_STRICTATIME = 1 << 24
_O_PATH = 0o10000000
_OPTION_FLAGS = {
    "nosuid": _MS_NOSUID,
    "nodev": _MS_NODEV,
    "noexec": _MS_NOEXEC,
    "noatime": _MS_NOATIME,
    "nodiratime": _MS_NODIRATIME,
    "relatime": _MS_RELATIME,
    "strictatime": _MS_STRICTATIME,
}
#: Mount trees left writable even under protect_system, as systemd's
#: ProtectSystem=strict does: kernel interfaces and per-user runtime sockets.
_ALWAYS_WRITABLE_PREFIXES = ("/proc", "/sys", "/dev", "/run/user")


def _libc() -> ctypes.CDLL:
    return ctypes.CDLL(None, use_errno=True)


def _check(result: int, what: str) -> None:
    if result != 0:
        error = ctypes.get_errno()
        raise MountError(f"{what}: {os.strerror(error)}", error)


def _mount(source: str | None, target: str, fstype: str | None, flags: int, data: str | None = None) -> None:
    libc = _libc()
    libc.mount.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_char_p, ctypes.c_ulong, ctypes.c_char_p]
    _check(
        libc.mount(
            source.encode() if source is not None else None,
            target.encode(),
            fstype.encode() if fstype is not None else None,
            flags,
            data.encode() if data is not None else None,
        ),
        f"mount {source or fstype} on {target}",
    )


def _write(path: str, text: str) -> None:
    with open(path, "w", encoding="ascii") as handle:
        handle.write(text)


def _under(path: str, roots: Sequence[str]) -> bool:
    return any(path == root or path.startswith(root.rstrip("/") + "/") for root in roots)


def _mounts() -> list[tuple[str, int]]:
    """(mount point, per-mount flags to preserve) for every mount, parents first."""

    rows: list[tuple[str, int]] = []
    with open("/proc/self/mountinfo", encoding="utf-8", errors="surrogateescape") as handle:
        for line in handle:
            fields = line.split(" - ", 1)[0].split()
            if len(fields) < 6:
                continue
            flags = 0
            for option in fields[5].split(","):
                flags |= _OPTION_FLAGS.get(option, 0)
            rows.append((_unescape(fields[4]), flags))
    return rows


def _mount_at_fd(
    source: str | None, target: str, fstype: str | None, flags: int, data: str | None = None
) -> None:
    """Mount onto `target` through its verified O_PATH fd, not its name."""

    descriptor, fd_path = _target_fd_path(target)
    try:
        _mount(source, fd_path, fstype, flags, data)
    finally:
        os.close(descriptor)


def _remount_read_only(point: str, flags: int | None = None) -> None:
    """Make the topmost mount at ``point`` read-only, keeping its other flags.

    The flags must be preserved: inside a user namespace nosuid/nodev/noexec
    and the atime mode are locked, and as root dropping them would weaken the
    host's own policy.
    """

    if flags is None:
        flags = next((value for mount_point, value in reversed(_mounts()) if mount_point == point), 0)
    _mount_at_fd(None, point, None, _MS_BIND | _MS_REMOUNT | _MS_RDONLY | flags)


def _remount_tree_read_only(
    base: str,
    keep: Sequence[str] = (),
    keep_exact: Sequence[str] = (),
    *,
    strict_fd: bool = False,
    strict_roots: Sequence[str] = (),
) -> list[str]:
    """Remount ``base`` and every mount below it read-only; return real failures.

    Mounts at or below a ``keep`` path, and mounts exactly at a ``keep_exact``
    path, are left alone.
    """

    failures: list[str] = []
    # A remount by path reaches only the TOPMOST mount at that path, so use
    # the flags of the last mountinfo row for each path (a stacked mount's
    # locked flags differ from the one it covers).
    topmost: dict[str, int] = {}
    for point, flags in _mounts():
        topmost[point] = flags
    for point, flags in topmost.items():
        if not _under(point, [base]) or _under(point, keep) or point in keep_exact:
            continue
        try:
            os.lstat(point)
        except FileNotFoundError:
            continue  # covered by a tmpfs: unreachable, nothing to protect
        except OSError:
            # A failed lstat (EACCES, a dead FUSE transport) proves
            # nothing about writability: fall through and remount it
            # anyway; the remount's own result decides.
            pass
        try:
            if strict_fd:
                _remount_read_only_strict(point, flags, strict_roots)
            else:
                _remount_read_only(point, flags)
        except (MountError, SandboxError) as exc:
            if strict_fd:
                # Strict mode refuses every failure except two proven
                # cases: a shadowed mount (EINVAL, and a fresh lookup
                # that does not land on a mount root) and a dead
                # transport (ENOTCONN: nothing can write through it).
                if isinstance(exc, MountError) and exc.errno == _errno.ENOTCONN:
                    continue
                if (
                    isinstance(exc, MountError)
                    and exc.errno == _errno.EINVAL
                    and _is_proven_shadowed_mount(point, strict_roots)
                ):
                    continue
                if not os.access(point, os.W_OK):
                    continue  # another user's mount this user cannot write anyway
                failures.append(str(exc))
                continue
            # EINVAL: the path no longer names that mount's root because a
            # layer above shadows it; unreachable. Other failures (another
            # user's FUSE mount, for example) are harmless when this user
            # could not write there anyway.
            if isinstance(exc, MountError) and (exc.errno == _errno.EINVAL or not os.access(point, os.W_OK)):
                continue
            failures.append(str(exc))
    return failures


def _target_fd_path(target: str) -> tuple[int, str]:
    """Open `target` as an O_PATH fd; return (fd, its /proc/self/fd path).

    Mounting onto the fd path (not the name) means a same-uid swap of a
    path component after resolution cannot redirect the mount (#1583).
    """

    descriptor = os.open(target, os.O_PATH | os.O_CLOEXEC)
    return descriptor, f"/proc/self/fd/{descriptor}"


def _bind_fd(descriptor: int, target: str) -> None:
    """Bind the pinned ``descriptor`` (recursively) at ``target``, creating a mount point."""

    source_path = f"/proc/self/fd/{descriptor}"
    if not os.path.lexists(target):
        if os.path.isdir(source_path):
            os.makedirs(target, exist_ok=True)
        else:
            os.makedirs(os.path.dirname(target), exist_ok=True)
            with open(target, "a", encoding="utf-8"):
                pass
    target_descriptor, target_path = _target_fd_path(target)
    try:
        _mount(source_path, target_path, None, _MS_BIND | _MS_REC)
    finally:
        os.close(target_descriptor)


def _mask(path: str) -> None:
    if not os.path.lexists(path):
        return
    # mount(2) follows symbolic links; name the mount point it will really use.
    path = os.path.realpath(path)
    if not os.path.exists(path):
        return
    if os.path.isdir(path):
        # Owned by the user, so it reads as an empty directory, not EACCES.
        descriptor, fd_path = _target_fd_path(path)
        try:
            _mount(
                "tmpfs",
                fd_path,
                "tmpfs",
                _MS_NOSUID | _MS_NODEV | _MS_NOEXEC | _MS_RDONLY,
                f"mode=0700,uid={os.getuid()},gid={os.getgid()},size=4k",
            )
        finally:
            os.close(descriptor)
    else:
        _mount_at_fd("/dev/null", path, None, _MS_BIND)
        _remount_read_only(path)


def _mask_eden_backing(path: str) -> None:
    """Mask a host Eden backing store, leaving a bare shared-dir skeleton.

    The store's contents are hidden exactly as :func:`_mask` hides a
    directory, but Sapling still has to find its shared repository: it
    refuses a checkout whose sharedpath names a directory with no
    ``.hg/store`` at all, which would break ``sl root`` in the lane's own
    checkout. An empty ``.hg`` skeleton (a ``store`` directory and an empty
    ``requires`` file) keeps that discovery working while every real store
    file reads as absent. The skeleton lives on a writable tmpfs because
    Sapling also wants write access to its shared directory; anything a
    boxed command writes there lands in this launch's tmpfs, never on the
    host, and is discarded with the box. Store-reading commands already
    fail against the read-only host backing (its lock files cannot be
    opened), so no working behaviour is lost.
    """

    if not os.path.lexists(path):
        return
    path = os.path.realpath(path)
    if not os.path.isdir(path):
        _mask(path)
        return
    _mount(
        "tmpfs",
        path,
        "tmpfs",
        _MS_NOSUID | _MS_NODEV | _MS_NOEXEC,
        f"mode=0755,uid={os.getuid()},gid={os.getgid()},size=16m",
    )
    os.makedirs(os.path.join(path, ".hg", "store"))
    with open(os.path.join(path, ".hg", "requires"), "a", encoding="utf-8"):
        pass


def _mask_git_config(path: str, *, strict_fd: bool = False, strict_roots: Sequence[str] = ()) -> str:
    """Mask one isolated-policy Git configuration path; return its outcome.

    Unlike :func:`_mask`, nothing is resolved: a symbolic link (valid or
    dangling) at the path, or a parent directory reached through one, is
    refused rather than followed to mask something else, and any failure
    refuses. A directory is covered by an empty read-only tmpfs ("directory"),
    a regular file by a read-only bind of /dev/null ("file"); either mount
    keeps the path's host contents out of the view for the command's whole
    lifetime. A path that does not exist is REFUSED: a mount needs an existing
    mount point, and creating one would write a Git configuration location on
    the host (never done), so nothing could stop a host process from creating
    the file while the command runs.
    """

    parent = os.path.dirname(path)
    if not os.path.isabs(path) or os.path.normpath(path) != path or os.path.realpath(parent) != parent:
        raise SandboxError(
            f"git-config-policy isolated: {path} is reached through a symbolic link or is not a "
            "normalized absolute path; refusing to mask what it points at"
        )
    try:
        info = os.lstat(path)
    except FileNotFoundError as exc:
        raise SandboxError(
            f"git-config-policy isolated: {path} does not exist, so no mount can keep it out of the "
            "view for the command's lifetime (a file created there later on the host would be "
            "visible); refusing"
        ) from exc
    except OSError as exc:
        raise SandboxError(f"git-config-policy isolated: cannot inspect {path}: {exc}") from exc
    if stat.S_ISLNK(info.st_mode):
        raise SandboxError(
            f"git-config-policy isolated: {path} is a symbolic link; it is not followed, refusing"
        )
    try:
        if stat.S_ISDIR(info.st_mode):
            _mount_at_dispatch(
                "tmpfs",
                path,
                "tmpfs",
                _MS_NOSUID | _MS_NODEV | _MS_NOEXEC | _MS_RDONLY,
                f"mode=0700,uid={os.getuid()},gid={os.getgid()},size=4k",
                strict_fd,
                strict_roots,
            )
            return "directory"
        if stat.S_ISREG(info.st_mode):
            _mount_at_dispatch("/dev/null", path, None, _MS_BIND, None, strict_fd, strict_roots)
            _remount_read_only_dispatch(path, strict_fd, strict_roots)
            return "file"
    except MountError as exc:
        raise SandboxError(f"git-config-policy isolated: cannot mask {path}: {exc}") from exc
    raise SandboxError(f"git-config-policy isolated: {path} is neither a regular file nor a directory, refusing")


def _read_only(path: str) -> bool:
    return bool(os.statvfs(path).f_flag & os.ST_RDONLY)


def _confirm_git_config_masks(
    outcomes: Sequence[tuple[str, str]],
    *,
    lstat: Callable[[str], os.stat_result] = os.lstat,
    listdir: Callable[[str], list[str]] = os.listdir,
    read_only: Callable[[str], bool] = _read_only,
) -> None:
    """Refuse unless every isolated-policy path shows its outcome inside the finished view.

    Called after the whole view is built and before the command starts: a
    "file" must be the read-only /dev/null character device and a "directory"
    an empty read-only directory; any other outcome, or a path that is missing
    now, refuses.
    """

    null = os.stat("/dev/null")
    for path, outcome in outcomes:
        try:
            info: os.stat_result | None = lstat(path)
        except FileNotFoundError:
            info = None
        except OSError as exc:
            raise SandboxError(f"git-config-policy isolated: cannot confirm {path}: {exc}") from exc
        if outcome == "file":
            confirmed = (
                info is not None
                and stat.S_ISCHR(info.st_mode)
                and info.st_rdev == null.st_rdev
                and read_only(path)
            )
        elif outcome == "directory":
            confirmed = info is not None and stat.S_ISDIR(info.st_mode) and not listdir(path) and read_only(path)
        else:
            confirmed = False
        if not confirmed:
            raise SandboxError(
                f"git-config-policy isolated: {path} is not {outcome} inside the view; refusing to "
                "run the command"
            )


def _same_kind(real: str, placeholder: str) -> bool:
    """A real entry can be bound on its placeholder: both exist, both dirs or both not."""

    if not os.path.exists(real) or os.path.islink(placeholder) or not os.path.exists(placeholder):
        return False
    return os.path.isdir(real) == os.path.isdir(placeholder)


_SYS_OPEN_TREE = 428
_SYS_MOVE_MOUNT = 429
_SYS_PIVOT_ROOT = 155
_OPEN_TREE_CLONE = 1
_OPEN_TREE_CLOEXEC = 0o2000000
_AT_RECURSIVE = 0x8000
_AT_FDCWD = -100
_MOVE_MOUNT_F_EMPTY_PATH = 0x00000004
_MNT_DETACH = 2


def _syscall(number: int, *args: object) -> int:
    libc = _libc()
    libc.syscall.restype = ctypes.c_long
    result = libc.syscall(number, *args)
    if result < 0:
        error = ctypes.get_errno()
        raise MountError(f"syscall {number}: {os.strerror(error)}", error)
    return int(result)


def _open_tree_clone(root_fd: int, name: str, *, recursive: bool) -> int:
    flags = _OPEN_TREE_CLONE | _OPEN_TREE_CLOEXEC | (_AT_RECURSIVE if recursive else 0)
    return _syscall(_SYS_OPEN_TREE, root_fd, name.encode(), flags)


def _move_mount_clone(descriptor: int, target: str) -> None:
    _syscall(_SYS_MOVE_MOUNT, descriptor, b"", _AT_FDCWD, target.encode(), _MOVE_MOUNT_F_EMPTY_PATH)


# --- fd-based mounts for the root entry (#1792): resolve once, keep the
# fd, mount onto it. No path-string mount is ever a fallback in this mode.
_SYS_OPENAT2 = 437
_SYS_FSOPEN = 430
_SYS_FSCONFIG = 431
_SYS_FSMOUNT = 432
_SYS_MOUNT_SETATTR = 442
_SYS_STATX = 332
_AT_EMPTY_PATH = 0x1000
_MOVE_MOUNT_T_EMPTY_PATH = 0x40
_RESOLVE_BENEATH = 0x08
_RESOLVE_NO_MAGICLINKS = 0x02
_RESOLVE_NO_SYMLINKS = 0x04
_ATTR_RDONLY = 1
_ATTR_NOSUID = 2
_ATTR_NODEV = 4
_ATTR_NOEXEC = 8



def _fd_syscall(number: int, *args: object) -> int:
    """A mount-API syscall for the strict root build; failures refuse."""

    return _syscall(number, *args)


def _syscall_errno(number: int, *args: object) -> int:
    """Run a syscall for a presence probe; return 0 or the errno."""

    libc = _libc()
    libc.syscall.restype = ctypes.c_long
    if libc.syscall(number, *args) < 0:
        return ctypes.get_errno()
    return 0


def _require_fd_mount_syscalls() -> None:
    """Fail closed unless the fd mount APIs exist on this kernel (#1792).

    Presence is probed with calls that cannot mount anything (an
    invalid-argument answer proves the syscall is wired up; ENOSYS
    refuses). During the build itself every strict call refuses on
    any failure, so a mid-build ENOSYS/EINVAL is never a fallback.
    """

    root_fd = os.open("/", os.O_PATH | os.O_CLOEXEC)
    try:
        try:
            tree = _open_tree_fd(root_fd)
        except MountError as exc:
            raise SandboxError(
                "root view needs open_tree for fd-based mounts (unavailable); refusing"
            ) from exc
        os.close(tree)
        fs_fd: int | None = None
        try:
            fs_fd = _fd_syscall(_SYS_FSOPEN, b"tmpfs", 1)
        except MountError as exc:
            raise SandboxError(
                "root view needs fsopen for fd-based mounts (unavailable); refusing"
            ) from exc
        finally:
            if fs_fd is not None:
                os.close(fs_fd)
        for number, name in ((_SYS_MOVE_MOUNT, "move_mount"), (_SYS_MOUNT_SETATTR, "mount_setattr")):
            if _syscall_errno(number, -1, b"", -1, b"", 0x80000000) == 38:  # ENOSYS
                raise SandboxError(f"root view needs {name} for fd-based mounts (unavailable); refusing")
    finally:
        os.close(root_fd)


def _openat2_fd(dfd: int, relative: str) -> int:
    """openat2 `relative` beneath `dfd`, symlink-free; fail closed."""

    import struct

    libc = _libc()
    if not hasattr(libc, "syscall"):
        raise SandboxError("root view needs openat2 (unavailable); refusing")
    how = ctypes.create_string_buffer(
        struct.pack(
            "QQQ",
            os.O_PATH | os.O_CLOEXEC,
            0,
            _RESOLVE_BENEATH | _RESOLVE_NO_SYMLINKS | _RESOLVE_NO_MAGICLINKS,
        ),
        24,
    )
    result = libc.syscall(_SYS_OPENAT2, dfd, relative.encode(), ctypes.byref(how), 24)
    if result < 0:
        error = ctypes.get_errno()
        if error == 38:  # ENOSYS
            raise SandboxError("root view needs openat2 (unavailable); refusing")
        raise MountError(
            f"root view path component {relative!r} does not resolve without symlinks "
            f"({os.strerror(error)}); refusing",
            error,
        )
    return int(result)


#: Trusted-root fds for the strict build (#1792). Every root is opened
#: once, after the root-path ownership checks, and its owner is verified
#: with fstat before it is cached. The cache lets later resolutions
#: start from the verified root instead of reopening it by name. Every
#: fd is O_CLOEXEC, so leftovers close at exec or process exit.
_STRICT_ROOT_FDS: dict[str, int] = {}

#: After the strict build mounts its fresh tmpfs, target resolutions
#: under /tmp start from this fd (the box's tmpfs), not the host /tmp
#: fd used to pin sources before the cover.
_STRICT_BOX_TMP_FD: int | None = None

#: Bind and mask identities recorded during the strict build. They let
#: the final verification compare a target with the fd that was pinned
#: before the mount, rather than trusting a path name after the build.
_STRICT_BIND_RECORDS: list[dict[str, object]] = []
_STRICT_MASK_RECORDS: list[tuple[str, str, int]] = []


def _cache_strict_roots(roots: Sequence[str]) -> None:
    for root in roots:
        real = os.path.realpath(root)
        if real in _STRICT_ROOT_FDS or not os.path.isdir(real):
            continue
        descriptor = _openat2_strict(real)
        info = os.fstat(descriptor)
        owners = (os.getuid(), 0, 65534) if _in_test_user_namespace() else (os.getuid(), 0)
        if info.st_uid not in owners:
            os.close(descriptor)
            raise SandboxError(f"trusted root {real!r} is owned by uid {info.st_uid}; refusing")
        _STRICT_ROOT_FDS[real] = descriptor


def _in_test_user_namespace() -> bool:
    """True when a uid_map other than the identity map is in effect.

    The root helper runs in the initial namespace; tests enter a
    one-id user namespace, where host-root ownership appears as the
    overflow uid. Only there is 65534 an acceptable root owner.
    """

    try:
        lines = Path("/proc/self/uid_map").read_text(encoding="utf-8").splitlines()
    except OSError:
        return False
    for line in lines:
        parts = line.split()
        if len(parts) == 3 and not (parts[0] == "0" and parts[1] == "0" and int(parts[2]) > 1_000_000):
            return True
    return False


def _drop_cached_roots_at_or_below(target: str) -> None:
    """Forget every cached root a mount at `target` may have covered.

    A cached root fd names the tree as it was when opened; once any
    ancestor is covered, lookups through it reach mounts the finished
    view cannot see (and the final checks then confirm the wrong
    mount). Dropping at-or-below roots on every mount and letting
    them re-resolve lazily keeps the cache a trust anchor only for
    trees nothing has covered yet.
    """

    global _STRICT_BOX_TMP_FD
    stale = [root for root in _STRICT_ROOT_FDS if root == target or root.startswith(target.rstrip("/") + "/")]
    for root in stale:
        os.close(_STRICT_ROOT_FDS.pop(root))
    if _STRICT_BOX_TMP_FD is not None and (target == "/tmp" or target.startswith("/tmp/")):
        os.close(_STRICT_BOX_TMP_FD)
        _STRICT_BOX_TMP_FD = None


def _fd_mount_id(descriptor: int) -> int:
    import struct

    buffer = ctypes.create_string_buffer(256)
    _fd_syscall(_SYS_STATX, descriptor, b"", _AT_EMPTY_PATH, 0x1000, ctypes.byref(buffer))
    mask = int(struct.unpack_from("I", buffer, 0)[0])
    if not mask & 0x1000:  # STATX_MNT_ID
        raise SandboxError("statx returned no mount id; refusing")
    return int(struct.unpack_from("Q", buffer, 144)[0])


_STATX_ATTR_MOUNT_ROOT = 0x2000


def _fd_is_mount_root(descriptor: int) -> bool:
    import struct

    buffer = ctypes.create_string_buffer(256)
    _fd_syscall(_SYS_STATX, descriptor, b"", _AT_EMPTY_PATH, 0x1000, ctypes.byref(buffer))
    attributes = int(struct.unpack_from("Q", buffer, 8)[0])
    return bool(attributes & _STATX_ATTR_MOUNT_ROOT)


def _create_relative(root_fd: int, relative: str, create: str) -> int:
    current = root_fd
    owned = False
    try:
        parts = relative.split("/")
        for index, part in enumerate(parts):
            last = index == len(parts) - 1
            try:
                child = _openat2_fd(current, part)
            except MountError as exc:
                if exc.errno != _errno.ENOENT:
                    raise
                if not last or create == "dir":
                    os.mkdir(part, 0o755, dir_fd=current)
                elif create == "file":
                    created = os.open(
                        part, os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o644, dir_fd=current
                    )
                    os.close(created)
                else:
                    raise SandboxError(f"unknown mount-point creation {create!r}; refusing") from exc
                child = _openat2_fd(current, part)
            if owned:
                os.close(current)
            current = child
            owned = True
        return current
    except BaseException:
        if owned:
            os.close(current)
        raise


def _resolve_fd(
    path: str,
    roots: Sequence[str] = (),
    *,
    create: str | None = None,
    allow_unrooted: bool = False,
    target: bool = False,
    fresh: bool = False,
) -> int:
    """Resolve `path` once to an O_PATH fd beneath its trusted root.

    No trusted root, no resolution (the only unrooted spellings are
    `/` itself and calls that pass `allow_unrooted` for `/dev/null`,
    masks, or mount-table points). With `target` set, a path under
    /tmp resolves through the box's fresh tmpfs once it exists. A
    missing path is created through its resolved parents when
    `create` is "dir" or "file" (#1792).
    """

    if not os.path.isabs(path):
        raise SandboxError(f"root view path {path!r} is not absolute; refusing")
    if target and not fresh and _STRICT_BOX_TMP_FD is not None and (path == "/tmp" or path.startswith("/tmp/")):
        base = "/tmp"
        root_fd = os.dup(_STRICT_BOX_TMP_FD)
    else:
        base = ""
        for root in sorted((os.path.realpath(item) for item in roots), key=len, reverse=True):
            if path == root or path.startswith(root + "/"):
                base = root
                break
        if not base:
            if path != "/" and not allow_unrooted:
                raise SandboxError(f"root view path {path!r} is outside every trusted root; refusing")
            base = "/"
        cached = None if fresh else _STRICT_ROOT_FDS.get(base)
        root_fd = os.dup(cached) if cached is not None else _openat2_strict(base)
    try:
        relative = os.path.relpath(path, base)
        if relative == ".":
            owned, root_fd = root_fd, -1
            return owned
        try:
            return _openat2_fd(root_fd, relative)
        except MountError as exc:
            if create is None or exc.errno != _errno.ENOENT:
                raise
        return _create_relative(root_fd, relative, create)
    finally:
        if root_fd >= 0:
            os.close(root_fd)


def _open_tree_fd(descriptor: int, *, recursive: bool = True) -> int:
    flags = _OPEN_TREE_CLONE | _OPEN_TREE_CLOEXEC | _AT_EMPTY_PATH | (_AT_RECURSIVE if recursive else 0)
    return _fd_syscall(_SYS_OPEN_TREE, descriptor, b"", flags)


def _move_mount_fd(tree_fd: int, target_fd: int) -> None:
    _fd_syscall(_SYS_MOVE_MOUNT, tree_fd, b"", target_fd, b"", _MOVE_MOUNT_F_EMPTY_PATH | _MOVE_MOUNT_T_EMPTY_PATH)


def _mount_setattr_fd(
    descriptor: int,
    *,
    attr_set: int = 0,
    attr_clr: int = 0,
    propagation: int = 0,
    recursive: bool = False,
) -> None:
    import struct

    buffer = ctypes.create_string_buffer(struct.pack("QQQQ", attr_set, attr_clr, propagation, 0), 32)
    flags = _AT_EMPTY_PATH | (_AT_RECURSIVE if recursive else 0)
    _fd_syscall(_SYS_MOUNT_SETATTR, descriptor, b"", flags, ctypes.byref(buffer), 32)


def _bind_resolved_fd(source_fd: int, target_fd: int, *, recursive: bool = True) -> int:
    """Bind the pinned source fd onto the resolved target fd (#1792).

    Returns the cloned mount's id, captured from the open_tree fd
    before the move so the final verification can compare it with
    the mount that is actually at the target after the build.
    """

    tree = _open_tree_fd(source_fd, recursive=recursive)
    try:
        mount_id = _fd_mount_id(tree)
        _move_mount_fd(tree, target_fd)
        return mount_id
    finally:
        os.close(tree)


def _mount_ids_by_point() -> dict[str, list[int]]:
    """Mount-table ids per point; a fresh id proves a mount landed."""

    out: dict[str, list[int]] = {}
    try:
        lines = Path("/proc/self/mountinfo").read_text(encoding="utf-8", errors="surrogateescape").splitlines()
    except OSError:
        return out
    for line in lines:
        fields = line.split(" - ", 1)[0].split()
        if len(fields) >= 5:
            try:
                out.setdefault(_unescape(fields[4]), []).append(int(fields[0]))
            except ValueError:
                continue
    return out


def _bind_fd_strict(source_fd: int, target: str, roots: Sequence[str] = ()) -> None:
    source_info = os.fstat(source_fd)
    is_directory = stat.S_ISDIR(source_info.st_mode)
    before = _mount_ids_by_point().get(target, [])
    target_fd = _resolve_fd(target, roots, create="dir" if is_directory else "file", target=True)
    try:
        mount_id = _bind_resolved_fd(source_fd, target_fd)
    finally:
        os.close(target_fd)
    # The tree we moved was cloned from the pinned source fd, so a
    # fresh mount id at the point proves the bind landed there; the
    # name may already be covered (a scratch root under /tmp), which
    # is why this reads the mount table, not the path.
    if not any(identifier not in before for identifier in _mount_ids_by_point().get(target, [])):
        raise SandboxError(f"built view mounts do not match: bind {target} did not land; refusing")
    _STRICT_BIND_RECORDS.append(
        {"target": target, "dev": source_info.st_dev, "ino": source_info.st_ino, "mount_id": mount_id}
    )
    _drop_cached_roots_at_or_below(target)


def _mount_tmpfs_fd(target_fd: int, data: str | None, flags: int) -> int:
    """Mount a fresh tmpfs onto `target_fd`; return its mount id.

    The id is read from the fsmount fd before the move, exactly as
    binds read theirs from the open_tree fd, so the final mask
    confirmation never has to re-derive it from a path.
    """

    fs_fd = _fd_syscall(_SYS_FSOPEN, b"tmpfs", 1)
    try:
        for pair in (data or "").split(","):
            key, separator, value = pair.partition("=")
            if separator:
                _fd_syscall(_SYS_FSCONFIG, fs_fd, 1, key.strip().encode(), value.strip().encode(), 0)
        _fd_syscall(_SYS_FSCONFIG, fs_fd, 6, 0, 0, 0)  # FSCONFIG_CMD_CREATE
        attributes = flags & (_ATTR_RDONLY | _ATTR_NOSUID | _ATTR_NODEV | _ATTR_NOEXEC)
        mount_fd = _fd_syscall(_SYS_FSMOUNT, fs_fd, 1, attributes)
        try:
            mount_id = _fd_mount_id(mount_fd)
            _move_mount_fd(mount_fd, target_fd)
            return mount_id
        finally:
            os.close(mount_fd)
    finally:
        os.close(fs_fd)


def _mount_tmpfs_strict(target: str, data: str | None, flags: int, roots: Sequence[str] = ()) -> None:
    """A fresh tmpfs on `target`, created and mounted only through fds."""

    global _STRICT_BOX_TMP_FD
    # /tmp is the helper's own mount (step 1 of every view), not a
    # spec-chosen target, so it may resolve from "/" itself.
    target_fd = _resolve_fd(target, roots, allow_unrooted=target == "/tmp", target=True)
    try:
        _mount_tmpfs_fd(target_fd, data, flags)
    finally:
        os.close(target_fd)
    _drop_cached_roots_at_or_below(target)
    if target == "/tmp":
        # By name, not through the cached host fd: this must name the
        # tmpfs just mounted, not the covered host /tmp beneath it.
        _STRICT_BOX_TMP_FD = _openat2_strict("/tmp")


def _is_proven_shadowed_mount(point: str, roots: Sequence[str]) -> bool:
    """A fresh lookup lands below a mount root, so the row is covered.

    Proof, not a guess from the error or the last table row: statx on
    the freshly resolved fd lacks STATX_ATTR_MOUNT_ROOT, meaning the
    point names a directory inside some other mount, not the mount
    the sweep meant to remount.
    """

    try:
        descriptor = _resolve_fd(point, roots, allow_unrooted=True, target=True, fresh=True)
    except (SandboxError, OSError):
        return False
    try:
        return not _fd_is_mount_root(descriptor)
    except (SandboxError, OSError):
        return False
    finally:
        os.close(descriptor)


def _remount_read_only_strict(point: str, flags: int | None, roots: Sequence[str] = ()) -> None:
    # attr_set carries only RDONLY on purpose: naming an atime mode
    # here makes mount_setattr refuse (EINVAL) on noatime mounts, and
    # the sweep used to skip that silently, leaving them writable.
    descriptor = _resolve_fd(point, roots, allow_unrooted=True, target=True)
    try:
        _mount_setattr_fd(descriptor, attr_set=_ATTR_RDONLY)
    finally:
        os.close(descriptor)


def _mount_at_fd_strict(
    source: str | None, target: str, fstype: str | None, flags: int, data: str | None, roots: Sequence[str] = ()
) -> None:
    if fstype == "tmpfs":
        _mount_tmpfs_strict(target, data, flags, roots)
        return
    if fstype is not None or source is None:
        raise SandboxError(f"root view cannot fd-mount {fstype or source} on {target}; refusing")
    source_fd = _resolve_fd(source, roots, allow_unrooted=source == "/dev/null")
    target_fd = _resolve_fd(target, roots, target=True)
    try:
        _bind_resolved_fd(source_fd, target_fd, recursive=False)
    finally:
        os.close(source_fd)
        os.close(target_fd)
    _drop_cached_roots_at_or_below(target)


def _mask_strict(path: str, roots: Sequence[str] = ()) -> tuple[str, str, int] | None:
    """Mask by resolved fd in root mode: no realpath, no name mount.

    Resolution comes first: only an ENOENT from openat2 skips (the
    path is genuinely absent); a symlink or any other failure
    refuses. Returns the mask record the final confirmation checks,
    or None when skipped.
    """

    try:
        descriptor = _resolve_fd(path, roots, allow_unrooted=True, target=True)
    except MountError as exc:
        if exc.errno == _errno.ENOENT:
            return None
        raise
    is_directory = stat.S_ISDIR(os.fstat(descriptor).st_mode)
    try:
        if is_directory:
            mount_id = _mount_tmpfs_fd(
                descriptor,
                f"mode=0700,uid={os.getuid()},gid={os.getgid()},size=4k",
                _MS_NOSUID | _MS_NODEV | _MS_NOEXEC | _MS_RDONLY,
            )
        else:
            source_fd = _resolve_fd("/dev/null", roots, allow_unrooted=True)
            try:
                mount_id = _bind_resolved_fd(source_fd, descriptor, recursive=False)
            finally:
                os.close(source_fd)
    finally:
        os.close(descriptor)
    _drop_cached_roots_at_or_below(path)
    if not is_directory:
        _remount_read_only_strict(path, None, roots)
    return (path, "directory" if is_directory else "file", mount_id)


_TMPFS_MAGIC = 0x01021994


def _fstatfs_magic(descriptor: int) -> int:
    """f_type of the filesystem behind `descriptor` (libc fstatfs)."""

    import struct

    buffer = ctypes.create_string_buffer(120)
    if _libc().fstatfs(descriptor, ctypes.byref(buffer)) != 0:
        error = ctypes.get_errno()
        raise MountError(f"fstatfs: {os.strerror(error)}", error)
    return int(struct.unpack_from("q", buffer, 0)[0])


def _confirm_strict_masks(records: Sequence[tuple[str, str, int]], roots: Sequence[str] = ()) -> None:
    """Every recorded mask is still the mount that was made for it.

    Confirmation resolves fresh and uncached in the finished view:
    the cached fds may name a covered tree the view cannot see.
    """

    null_info = os.stat("/dev/null")
    for path, kind, mount_id in records:
        descriptor = _resolve_fd(path, roots, allow_unrooted=True, target=True, fresh=True)
        try:
            if _fd_mount_id(descriptor) != mount_id:
                raise SandboxError(f"mask {path} is not the mount made for it; refusing")
            info = os.fstat(descriptor)
            if kind == "file":
                if not stat.S_ISCHR(info.st_mode) or info.st_rdev != null_info.st_rdev:
                    raise SandboxError(f"mask {path} is not the /dev/null cover made for it; refusing")
            elif _fstatfs_magic(descriptor) != _TMPFS_MAGIC or not os.statvfs(descriptor).f_flag & os.ST_RDONLY:
                raise SandboxError(f"mask {path} is not the empty read-only tmpfs made for it; refusing")
            elif kind == "directory":
                listing_fd = os.open(".", os.O_RDONLY | os.O_CLOEXEC, dir_fd=descriptor)
                try:
                    if os.listdir(listing_fd):
                        raise SandboxError(f"mask {path} is not the empty tmpfs made for it; refusing")
                finally:
                    os.close(listing_fd)
        finally:
            os.close(descriptor)


def _audit_strict_view_read_only(
    keep: Sequence[str], keep_exact: Sequence[str], roots: Sequence[str]
) -> None:
    """Every non-kept mount in the finished view is read-only.

    The sweep's word is not the proof: resolve each point fresh and
    uncached in the finished view and read ST_RDONLY off its own
    statvfs. A point that no longer resolves is unreachable in the
    view, so only ENOENT is excused.
    """

    points = list(dict.fromkeys(point for point, _flags in _mounts()))
    for point in points:
        if _under(point, keep) or point in keep_exact:
            continue
        try:
            descriptor = _resolve_fd(point, roots, allow_unrooted=True, target=True, fresh=True)
        except MountError as exc:
            if exc.errno in (_errno.ENOENT, _errno.ENOTCONN):
                continue  # gone, or a dead transport nothing can write through
            raise SandboxError(f"read-only audit cannot resolve {point}: {exc}; refusing") from exc
        try:
            try:
                writable = not os.statvfs(descriptor).f_flag & os.ST_RDONLY
            except OSError as exc:
                if exc.errno == _errno.ENOTCONN:
                    continue  # dead transport: nothing can write through it
                if not os.access(point, os.W_OK):
                    continue  # this user cannot write there in this view anyway
                raise SandboxError(f"read-only audit cannot inspect {point}: {exc}; refusing") from exc
            if writable:
                raise SandboxError(f"read-only audit: {point} is writable in the finished view; refusing")
        finally:
            os.close(descriptor)


def _bind_dispatch(descriptor: int, target: str, strict_fd: bool, roots: Sequence[str]) -> None:
    if strict_fd:
        _bind_fd_strict(descriptor, target, roots)
    else:
        _bind_fd(descriptor, target)


def _mount_at_dispatch(
    source: str | None,
    target: str,
    fstype: str | None,
    flags: int,
    data: str | None,
    strict_fd: bool,
    roots: Sequence[str],
) -> None:
    if strict_fd:
        _mount_at_fd_strict(source, target, fstype, flags, data, roots)
    else:
        _mount_at_fd(source, target, fstype, flags, data)


def _mask_dispatch(
    path: str, strict_fd: bool, roots: Sequence[str]
) -> tuple[str, str, int] | None:
    if strict_fd:
        return _mask_strict(path, roots)
    _mask(path)
    return None


def _remount_read_only_dispatch(point: str, strict_fd: bool, roots: Sequence[str]) -> None:
    if strict_fd:
        _remount_read_only_strict(point, None, roots)
    else:
        _remount_read_only(point)


def _pivot_user_root() -> None:
    """Re-root this namespace on a user-owned tmpfs under the fresh ``/tmp``.

    Sapling's shared-repository check walks the ancestors of a checkout's
    shared store to ``/`` and refuses any that are owned by neither the user
    nor root. Inside a user namespace a host root-owned directory shows as
    the overflow uid, so the check always fails at ``/`` (``eden info``
    exit 77). Mounting over ``/`` does not help: the process root stays
    pinned to the covered mount. Pivot instead: clone every top-level entry
    of the old root (fd-based, via open_tree), pivot into a bind of a
    staging directory on the fresh user-owned ``/tmp`` tmpfs, re-attach the
    clones, and detach the old root. Afterwards ``/`` is a directory the
    box user owns and every top-level path resolves as before.
    """

    stage_mount = Path("/tmp/.wrkslots-stage")
    staging = stage_mount / "root"
    if os.path.lexists(stage_mount):
        raise SandboxError(f"cannot build the Eden view: {stage_mount} already exists in the fresh /tmp")
    root_fd = os.open("/", _O_PATH)
    try:
        names = os.listdir(f"/proc/self/fd/{root_fd}")
        entries: dict[str, int] = {}
        links: dict[str, str] = {}
        proc_clone: int | None = None
        for name in names:
            info = os.stat(name, dir_fd=root_fd, follow_symlinks=False)
            if stat.S_ISLNK(info.st_mode):
                links[name] = os.readlink(name, dir_fd=root_fd)
            elif name == "proc":
                # /proc must come back without a /proc to look through, so
                # it is the one entry cloned with open_tree; every other
                # entry is bound back through its pinned descriptor below,
                # the same fd-based bind the slot tree uses.
                proc_clone = _open_tree_clone(root_fd, name, recursive=True)
            else:
                entries[name] = os.open(name, _O_PATH, dir_fd=root_fd)
        try:
            stage_mount.mkdir(mode=0o755)
            _mount("tmpfs", str(stage_mount), "tmpfs", _MS_NOSUID | _MS_NODEV, "mode=0755,size=256m")
            staging.mkdir(mode=0o755)
            (staging / ".old-root").mkdir(mode=0o755)
            _mount(str(staging), str(staging), None, _MS_BIND)
            _syscall(_SYS_PIVOT_ROOT, str(staging).encode(), str(staging / ".old-root").encode())
            os.chdir("/")
            _mount(None, "/", None, _MS_REC | _MS_PRIVATE)
            if proc_clone is not None:
                os.mkdir("/proc", mode=0o755)
                _move_mount_clone(proc_clone, "/proc")
                proc_clone = None
            for name in sorted(names):
                if name == "proc" or name not in entries:
                    continue
                _bind_fd(entries[name], "/" + name)
            for name, target_text in links.items():
                os.symlink(target_text, "/" + name)
        finally:
            if proc_clone is not None:
                os.close(proc_clone)
            for descriptor in entries.values():
                os.close(descriptor)
    finally:
        os.close(root_fd)


def _detach_pivoted_root() -> None:
    """Detach the old root left at ``/.old-root`` by :func:`_pivot_user_root`.

    Called only once every pinned bind has been made: a directory on a
    detached mount cannot be bind-mounted through its descriptor (EINVAL),
    so the old tree must stay attached while the view is built from those
    descriptors. Its staging mount under ``/tmp`` goes with it.
    """

    libc = _libc()
    libc.umount2.argtypes = [ctypes.c_char_p, ctypes.c_int]
    if libc.umount2(b"/.old-root", _MNT_DETACH) != 0:
        error = ctypes.get_errno()
        raise MountError(f"cannot detach the old root: {os.strerror(error)}", error)
    os.rmdir("/.old-root")
    with contextlib.suppress(OSError):
        libc.umount2(b"/tmp/.wrkslots-stage", _MNT_DETACH)
        os.rmdir("/tmp/.wrkslots-stage")


def _cover_unowned_ancestors(targets: Sequence[str]) -> None:
    """Cover root-owned ancestors of ``targets`` with a user tmpfs, binding the child back.

    The pivot makes ``/`` itself user-owned; an ancestor such as ``/home``
    can still be a host root-owned directory (the overflow uid inside a
    user namespace) on the walk Sapling makes from a shared store. Cover
    each such ancestor with an empty tmpfs owned by the box user and bind
    back the one child that leads to the target, recursively, so nothing
    below it changes. Ancestors owned by the user, or by real root in root
    mode, are left alone.
    """

    for target in targets:
        for ancestor in [str(parent) for parent in Path(target).parents][::-1]:
            if ancestor == "/":
                continue
            try:
                info = os.stat(ancestor)
            except OSError:
                continue
            if info.st_uid in (os.geteuid(), 0):
                continue
            relative = Path(target).relative_to(ancestor)
            child = str(Path(ancestor) / relative.parts[0])
            descriptor = os.open(child, _O_PATH)
            try:
                _mount(
                    "tmpfs",
                    ancestor,
                    "tmpfs",
                    _MS_NOSUID | _MS_NODEV,
                    "mode=0755,size=16m",
                )
                _bind_fd(descriptor, child)
            finally:
                os.close(descriptor)


def _cover_shared_ancestors(spec: Mapping[str, object]) -> None:
    """Make the ancestors Sapling walks from an Eden checkout user-owned."""

    targets = [
        str(spec[key])
        for key in ("eden_checkout", "eden_backing")
        if isinstance(spec.get(key), str) and spec.get(key)
    ]
    _pivot_user_root()
    _cover_unowned_ancestors(targets)


def build_view(spec: Mapping[str, object], *, strict_fd: bool = False, strict_roots: Sequence[str] = ()) -> None:
    """Build the view described by ``spec`` in the CURRENT (new) mount namespace.

    Shared by both isolation modes. The caller has entered a mount namespace in
    which it holds CAP_SYS_ADMIN, and runs as the invoking user (in ``root``
    mode: uid, gid, and groups already dropped, CAP_SYS_ADMIN kept), so every
    path lookup and access check is the user's own and every file created is
    the user's.

    With ``strict_fd`` (the root entry only, #1792) every content mount
    is made through an fd resolved once beneath a trusted root: no
    path-string mount and no fallback when the fd mount APIs are
    missing. The one exception is propagation on `/` itself in the
    non-strict spelling below; strict mode changes it with
    mount_setattr on the resolved root fd instead.
    """

    def strings(key: str) -> list[str]:
        value = spec.get(key, [])
        assert isinstance(value, list)
        return [str(item) for item in value]

    propagate = strings("propagate")
    # Slave propagation keeps host mount events flowing in (a coordinator box
    # sees image slots mounted after it started) while nothing mounted in the
    # box ever reaches the host. Without roots to propagate, everything is private.
    if strict_fd:
        _STRICT_BIND_RECORDS.clear()
        _STRICT_MASK_RECORDS.clear()
        _require_fd_mount_syscalls()
        _cache_strict_roots(strict_roots)
        slash_fd = _resolve_fd("/", strict_roots)
        try:
            _mount_setattr_fd(
                slash_fd,
                propagation=_MS_SLAVE if propagate else _MS_PRIVATE,
                recursive=True,
            )
        finally:
            os.close(slash_fd)
    else:
        _mount(None, "/", None, _MS_REC | (_MS_SLAVE if propagate else _MS_PRIVATE))
    home = str(spec["home"])
    layer = str(spec["home_layer"])
    mode = str(spec.get("home_mode", "ro"))
    aliases = [alias for alias in strings("home_aliases") if os.path.isdir(alias)]
    raw_binds = spec.get("binds", [])
    assert isinstance(raw_binds, list)
    binds = [
        (str(pair[0]), str(pair[1]))
        for pair in raw_binds
        if isinstance(pair, list) and len(pair) == 2 and os.path.exists(str(pair[0]))
    ]
    home_binds = [
        relative
        for relative in strings("home_binds")
        if _same_kind(os.path.join(home, relative), os.path.join(layer, relative))
    ]
    # Pin every source before anything is covered: once /tmp or $HOME is
    # covered, the real paths below it are reachable only through these
    # descriptors, and nothing has to be staged on the host.
    read_only = [path for path in strings("read_only") if os.path.isdir(path)]
    pinned: dict[str, int] = {}
    for path in [
        layer,
        *aliases,
        *(os.path.join(home, rel) for rel in home_binds),
        *read_only,
        *(src for src, _ in binds),
    ]:
        if path not in pinned:
            pinned[path] = _resolve_fd(path, strict_roots) if strict_fd else os.open(path, _O_PATH)

    # 1. A fresh /tmp for this launch.
    if strict_fd:
        _mount_tmpfs_strict(
            "/tmp", f"mode=1777,size={spec.get('tmp_size', DEFAULT_TMP_SIZE)}", _MS_NOSUID | _MS_NODEV, strict_roots
        )
    else:
        _mount("tmpfs", "/tmp", "tmpfs", _MS_NOSUID | _MS_NODEV, f"mode=1777,size={spec.get('tmp_size', DEFAULT_TMP_SIZE)}")
    if spec.get("eden_root_cover"):
        # 1a. An Eden view is re-rooted on a user-owned tmpfs so Sapling's
        # shared-directory ownership walk reaches a root it accepts.
        _cover_shared_ancestors(spec)

    # 2. $HOME: the slot's private layer, with the real entries bound read-only
    #    on their placeholders. The layer itself stays writable, so a harness
    #    can create and rename top-level files; they land in the layer.
    _bind_dispatch(pinned[layer], home, strict_fd, strict_roots)
    for relative in home_binds:
        target = os.path.join(home, relative)
        _bind_dispatch(pinned[target], target, strict_fd, strict_roots)
        failures = _remount_tree_read_only(target, strict_fd=strict_fd, strict_roots=strict_roots)
        if failures:
            raise SandboxError(f"could not make {target} read-only: " + "; ".join(failures[:3]))
    # Other paths to the same files: read-only ("ro") or empty ("hidden").
    for alias in aliases:
        if mode == "ro":
            _bind_dispatch(pinned[alias], alias, strict_fd, strict_roots)
            failures = _remount_tree_read_only(alias, strict_fd=strict_fd, strict_roots=strict_roots)
            if failures:
                raise SandboxError(f"could not make {alias} read-only: " + "; ".join(failures[:3]))
        else:
            _mount_at_dispatch(
                "tmpfs", alias, "tmpfs", _MS_NOSUID | _MS_NODEV, "mode=0755,size=16m", strict_fd, strict_roots
            )

    # 3. Trees that stay read-only whatever protect_system says: the wrkslots
    # control directory (registry, other slots and their mounts) and the root
    # launcher's spec directory. A slot below the control directory is bound
    # writable again next.
    for path in read_only:
        _bind_dispatch(pinned[path], path, strict_fd, strict_roots)
        failures = _remount_tree_read_only(path, strict_fd=strict_fd, strict_roots=strict_roots)
        if failures:
            raise SandboxError(f"could not make {path} read-only: " + "; ".join(failures[:3]))

    # 4. Writable binds, in order: shared $HOME state, nested private
    # directories, the slot, Git directories, outputs, extra paths.
    for source, target in binds:
        _bind_dispatch(pinned[source], target, strict_fd, strict_roots)

    # 5. Masks last, so they win over anything bound above. The isolated Git
    # configuration policy's paths are masked strictly (no link followed,
    # failures refuse) and their outcomes are confirmed once the view is done.
    git_config_masks = strings("git_config_masks")
    git_config_outcomes: list[tuple[str, str]] = []
    for path in strings("masks"):
        if path in git_config_masks:
            if strict_fd:
                outcome = _mask_git_config(path, strict_fd=True, strict_roots=strict_roots)
            else:
                outcome = _mask_git_config(path)
            git_config_outcomes.append((path, outcome))
        elif path == spec.get("eden_backing"):
            _mask_eden_backing(path)
        else:
            mask_record = _mask_dispatch(path, strict_fd, strict_roots)
            if mask_record is not None:
                _STRICT_MASK_RECORDS.append(mask_record)
    if sorted(path for path, _outcome in git_config_outcomes) != sorted(git_config_masks):
        raise SandboxError("git-config-policy isolated: a policy path is missing from the view's masks")
    if mode == "hidden":
        for alias in aliases:
            _remount_read_only_dispatch(alias, strict_fd, strict_roots)

    if spec.get("eden_root_cover"):
        # The old root stayed attached only so the pinned binds above could
        # be made from it; detach it before the read-only sweep so none of
        # its mounts are swept (or visible) in the finished view.
        _detach_pivoted_root()
    # 6. Everything else read-only. The layer mount is skipped by its EXACT
    # path only: the read-only binds below it must not be skipped with it.
    if spec.get("protect_system", True):
        keep = [*(target for _source, target in binds), "/tmp", *_ALWAYS_WRITABLE_PREFIXES]
        if isinstance(spec.get("eden_backing"), str):
            # The backing skeleton stays writable for Sapling's shared-dir
            # write check; it is a per-launch tmpfs, never the host store.
            keep.append(str(spec["eden_backing"]))
        failures = _remount_tree_read_only("/", keep, keep_exact=[home], strict_fd=strict_fd, strict_roots=strict_roots)
        if failures:
            raise SandboxError(
                "could not make every mount read-only (refusing to run unprotected): "
                + "; ".join(failures[:3])
            )
    if propagate:
        _limit_propagation(propagate, strict_fd=strict_fd, strict_roots=strict_roots)
    if git_config_masks:
        _confirm_git_config_masks(git_config_outcomes)
    if strict_fd:
        _confirm_strict_masks(_STRICT_MASK_RECORDS, strict_roots)
        if spec.get("protect_system", True):
            audit_keep = [*(target for _source, target in binds), "/tmp", *_ALWAYS_WRITABLE_PREFIXES]
            if isinstance(spec.get("eden_backing"), str):
                audit_keep.append(str(spec["eden_backing"]))
            _audit_strict_view_read_only(audit_keep, [home], strict_roots)
    for descriptor in pinned.values():
        os.close(descriptor)
    os.chdir(str(spec.get("cwd", "/")))


def _limit_propagation(roots: Sequence[str], *, strict_fd: bool = False, strict_roots: Sequence[str] = ()) -> None:
    """Keep host mount events only where ``roots`` are visible; make every other mount private.

    The mount each root resolves through in the view (the topmost mount whose
    mount point is the longest prefix of the root) and every mount below a root
    stay slaves, so an image slot mounted on the host appears at its slot path.
    Any other mount, such as ``/`` itself or a read-only bind of $HOME, stops
    receiving host mounts, which would otherwise arrive writable.
    """

    points = list(dict.fromkeys(point for point, _flags in _mounts()))
    keep: set[str] = set()
    for root in roots:
        covering = [point for point in points if _under(root, [point])]
        if covering:
            keep.add(max(covering, key=len))
    for point in points:
        if point in keep or _under(point, roots) or not os.path.lexists(point):
            continue
        try:
            if strict_fd:
                descriptor = _resolve_fd(point, strict_roots, allow_unrooted=True, target=True)
                try:
                    _mount_setattr_fd(descriptor, propagation=_MS_PRIVATE)
                finally:
                    os.close(descriptor)
            else:
                _mount(None, point, None, _MS_PRIVATE)
        except (MountError, SandboxError):
            if strict_fd:
                raise
            continue  # shadowed by a mount above it: unreachable


# ------------------------------------------------------------- userns helper


def _traced(spec: Mapping[str, object], command: Sequence[str], environ: Mapping[str, str]) -> list[str]:
    """Apply log-only denied-write tracing when $WRKSLOTS_DENY_LOG is set."""

    enabled = environ.get("WRKSLOTS_DENY_LOG", "").lower() in ("1", "true", "yes")
    log_path = Path(str(spec.get("home_layer", "/tmp"))) / "wrkslots-deny.log"
    return deny_log_command(command, log_path, enabled=enabled, strace=shutil.which("strace"))


def enter_userns(spec: Mapping[str, object], command: Sequence[str]) -> None:
    """Enter a new user + mount namespace, build the view, exec ``command``."""

    uid, gid = os.getuid(), os.getgid()
    _maybe_pdeathsig()
    _check(_libc().unshare(_CLONE_NEWUSER | _CLONE_NEWNS), "unshare user+mount namespace")
    _write("/proc/self/setgroups", "deny")
    _write("/proc/self/uid_map", f"{uid} {uid} 1\n")
    _write("/proc/self/gid_map", f"{gid} {gid} 1\n")
    build_view(spec)
    command = _traced(spec, command, os.environ)
    os.execvp(command[0], list(command))


# --------------------------------------------------------------- root helper

_PR_SET_KEEPCAPS = 8
_PR_SET_NO_NEW_PRIVS = 38

#: The root-owned copy of this file the sudoers entry names (#1583). A
#: checkout copy is user-writable, so sudo may never run it as root.
_INSTALLED_HELPER = Path("/usr/local/lib/wrkslots/sandbox.py")
_PR_SET_DUMPABLE = 4
_PR_SET_PDEATHSIG = 1


def _maybe_pdeathsig() -> None:
    """Die with the launcher ONLY when explicitly opted in (userns entry).

    A boxed lane must normally survive its launcher shell exiting, so
    this is never unconditional: WRKSLOTS_PDEATHSIG=1 may be set by
    test harnesses that kill their launcher and need no boxed child
    left. The root entry does not call this (#1792): the kernel
    clears the death signal when the helper drops uid, so it could
    never deliver that promise there; root-mode test cleanup is the
    harness's job (the live root tests stop their systemd slice in a
    finally block).
    """

    if os.environ.get("WRKSLOTS_PDEATHSIG") == "1":
        _prctl(_PR_SET_PDEATHSIG, 9)  # SIGKILL
_CAP_SYS_ADMIN = 21
_LINUX_CAPABILITY_VERSION_3 = 0x20080522


class _CapHeader(ctypes.Structure):
    _fields_ = [("version", ctypes.c_uint32), ("pid", ctypes.c_int)]


class _CapData(ctypes.Structure):
    _fields_ = [("effective", ctypes.c_uint32), ("permitted", ctypes.c_uint32), ("inheritable", ctypes.c_uint32)]


def _set_capabilities(capabilities: Sequence[int]) -> None:
    """Set the effective and permitted sets to exactly ``capabilities``."""

    header = _CapHeader(_LINUX_CAPABILITY_VERSION_3, 0)
    data = (_CapData * 2)()
    for capability in capabilities:
        index, bit = divmod(capability, 32)
        data[index].effective |= 1 << bit
        data[index].permitted |= 1 << bit
    _check(_libc().capset(ctypes.byref(header), data), "capset")


def _set_capabilities_permitted(capabilities: Sequence[int]) -> None:
    """Set only the permitted set; nothing is effective until raised."""

    header = _CapHeader(_LINUX_CAPABILITY_VERSION_3, 0)
    data = (_CapData * 2)()
    for capability in capabilities:
        index, bit = divmod(capability, 32)
        data[index].permitted |= 1 << bit
    _check(_libc().capset(ctypes.byref(header), data), "capset")


def _set_capabilities_split(
    effective: Sequence[int], permitted: Sequence[int]
) -> None:
    """Set effective and permitted sets independently."""

    header = _CapHeader(_LINUX_CAPABILITY_VERSION_3, 0)
    data = (_CapData * 2)()
    for capability in effective:
        index, bit = divmod(capability, 32)
        data[index].effective |= 1 << bit
    for capability in permitted:
        index, bit = divmod(capability, 32)
        data[index].permitted |= 1 << bit
    _check(_libc().capset(ctypes.byref(header), data), "capset")


def _verify_mount_points(
    view: Mapping[str, object],
    *,
    records: Sequence[Mapping[str, object]] | None = None,
    roots: Sequence[str] = (),
) -> None:
    """The built view's mounts are where they should be (#1792).

    When bind records are present (the strict root build), each
    target is re-resolved with openat2 and compared with the
    identity recorded before its mount: the pinned source fd's
    device+inode and the open_tree fd's mount id. Comparing a source
    and target by name after the build proves nothing (most binds
    are path onto itself), so names are never compared here. A
    masked target is skipped (masks win by design and are confirmed
    separately). Without records, this falls back to checking that
    the expected points are mounts at all.
    """

    bind_records = _STRICT_BIND_RECORDS if records is None else records
    if bind_records:
        masks = view.get("masks", [])
        masked = {str(item) for item in masks} if isinstance(masks, list) else set()
        latest: dict[str, Mapping[str, object]] = {}
        for record in bind_records:
            latest[str(record["target"])] = record
        for target, record in latest.items():
            if target in masked:
                continue
            descriptor = _resolve_fd(target, roots, target=True, fresh=True)
            try:
                landed = os.fstat(descriptor)
                if (landed.st_dev, landed.st_ino) != (record["dev"], record["ino"]):
                    raise SandboxError(
                        f"built view mounts do not match: bind {target} does not show its pinned source; refusing"
                    )
                if _fd_mount_id(descriptor) != record["mount_id"]:
                    raise SandboxError(
                        f"built view mounts do not match: bind {target} is not its recorded mount; refusing"
                    )
            finally:
                os.close(descriptor)
        return
    topmost: dict[str, str] = {}
    for device, _root, point in _mount_rows():
        topmost[str(point)] = device
    expected = {"/tmp", str(view["home"])}
    raw_binds = view.get("binds", [])
    if isinstance(raw_binds, list):
        for pair in raw_binds:
            if isinstance(pair, list) and len(pair) == 2:
                expected.add(str(pair[1]))
    missing = sorted(point for point in expected if point not in topmost)
    if missing:
        raise SandboxError(f"built view mounts do not match: missing {missing}; refusing")


def _prctl(option: int, value: int) -> None:
    _check(_libc().prctl(option, ctypes.c_ulong(value), 0, 0, 0), f"prctl({option})")


_SPEC_BYTES_LIMIT = 16 * 1024 * 1024
def _sudo_identity(parent_status: str | None = None) -> tuple[int, int, list[int], str]:
    """The invoking user, from sudo's record cross-checked against its parent.

    `SUDO_UID` alone is claimable with `setenv`; the parent process (the
    sudo monitor) carries the caller's real uid in /proc/<ppid>/status,
    which a setenv forgery cannot mint. Fail closed when the parent is
    not sudo-shaped or cannot be read.
    """

    import pwd

    try:
        uid = int(os.environ["SUDO_UID"])
        gid = int(os.environ["SUDO_GID"])
    except (KeyError, ValueError) as exc:
        raise SandboxError("enter-root must run through sudo (SUDO_UID/SUDO_GID are missing)") from exc
    if uid == 0:
        raise SandboxError("isolation root boxes a non-root user; the invoking user is root")
    # Cross-check against the parent (sudo) process's real uid AND gid:
    # with `setenv` a user can claim another uid/gid in the environment,
    # but cannot mint a sudo parent carrying them. (The process name is
    # not checked: comm is settable and proves nothing.)
    try:
        if parent_status is None:
            parent_status = Path(f"/proc/{os.getppid()}/status").read_text(encoding="utf-8")
        fields = {
            line.split(":", 1)[0]: line.split(":", 1)[1].strip()
            for line in parent_status.splitlines()
            if ":" in line
        }
        parent_uid = int(fields["Uid"].split()[0])
        parent_gid = int(fields["Gid"].split()[0])
    except (OSError, ValueError, KeyError) as exc:
        raise SandboxError("enter-root parent is not readable as a sudo process") from exc
    if parent_uid != uid:
        raise SandboxError("enter-root parent is not the sudo monitor for the invoking user")
    caller_groups = _caller_groups(uid) or []
    if gid != parent_gid and gid not in caller_groups:
        raise SandboxError("the sudo gid is not the invoking user's own group")
    try:
        name = pwd.getpwuid(uid).pw_name
    except KeyError as exc:
        raise SandboxError(f"invoking uid {uid} has no passwd entry") from exc
    groups = sorted(set(os.getgrouplist(name, gid)))
    return uid, gid, groups, name


def _registered_claim(view: SlotView) -> dict[str, object]:
    """What `write_root_spec` records for the root helper to verify (#1583)."""

    if view.coordinator:
        return {"kind": "box", "control": str(view.control_directory)}
    return {
        "kind": "slot",
        "slot": view.slot,
        "slot_path": str(view.slot_path),
        "control": str(view.control_directory),
        "project_root": str(view.project_root) if view.project_root is not None else None,
    }


#: Mount targets a root-built view may never cover: a user-authored spec
#: binding over one of these (then running a setuid program inside) is the
#: classic namespace escalation the root helper exists to refuse (#1583).
_SYSTEM_TARGET_PREFIXES = (
    "/etc", "/usr", "/lib", "/lib64", "/bin", "/sbin", "/proc", "/sys", "/dev", "/boot", "/root",
)

#: Registry-claim hard bounds additionally refuse these: a slot or
#: project root never legitimately sits directly under them. (The path
#: denylist above cannot include /run: the runtime spec directory
#: legitimately lives under /run/user/<uid>.)
_CLAIM_PREFIXES = _SYSTEM_TARGET_PREFIXES + ("/var", "/opt", "/run", "/srv")


#: Closed schemas for the root spec: any other key refuses (#1583).
_ROOT_DOCUMENT_KEYS = frozenset(
    {"view", "command", "environment", "uid", "gid", "groups", "umask", "registered"}
)
_ROOT_VIEW_KEYS = frozenset(
    {
        "home", "home_aliases", "home_mode", "home_layer", "home_binds", "read_only",
        "binds", "masks", "tmp_size", "protect_system", "propagate", "cwd",
        "git_config_policy", "git_config_masks",
        "eden_root_cover", "eden_checkout", "eden_backing",
    }
)

_TMP_SIZE = re.compile(r"([0-9]+)([KMGT%]?)\Z")


def _host_memory_bytes() -> int:
    """Total host memory for the percent tmp_size cap; 0 when unknown."""

    try:
        for line in Path("/proc/meminfo").read_text(encoding="utf-8").splitlines():
            if line.startswith("MemTotal:"):
                return int(line.split()[1]) * 1024
    except (OSError, ValueError, IndexError):
        pass
    try:
        return os.sysconf("SC_PHYS_PAGES") * os.sysconf("SC_PAGE_SIZE")
    except (OSError, ValueError):
        return 0


def _tmp_size_within_cap(value: object, host_memory_bytes: int | None = None) -> bool:
    """The tmp_size grammar plus the shared 64 GiB cap (#1792).

    One owner for the settings model, the root schema, and (mirrored)
    the Rust config validation: an absolute size with an optional
    K/M/G/T suffix, or "N%" (1-100) of host memory; both forms must
    land at or below 64 GiB.
    """

    match = _TMP_SIZE.fullmatch(str(value)) if isinstance(value, (str, int)) and not isinstance(value, bool) else None
    if match is None:
        return False
    amount, suffix = int(match.group(1)), match.group(2)
    if suffix == "%":
        memory = _host_memory_bytes() if host_memory_bytes is None else host_memory_bytes
        return 1 <= amount <= 100 and 0 < amount * memory // 100 <= 64 * (1 << 30)
    multiplier = {"": 1, "K": 1 << 10, "M": 1 << 20, "G": 1 << 30, "T": 1 << 40}
    return 0 < amount * multiplier[suffix] <= 64 * (1 << 30)


def _check_root_schema(
    document: Mapping[str, object], *, host_memory_bytes: int | None = None
) -> dict[str, object]:
    """Validate the stdin spec against the closed schema; return its view.

    Every view value is type-checked here (#1792), so a wrongly typed
    value refuses at the boundary instead of tripping a late assert
    inside `build_view` while capabilities are held.
    """

    unknown = set(document) - _ROOT_DOCUMENT_KEYS
    if unknown:
        raise SandboxError(f"root launcher spec has unknown keys {sorted(unknown)}; refusing")
    view = document.get("view")
    if not isinstance(view, dict):
        raise SandboxError("root launcher spec has no view object; refusing")
    unknown_view = set(view) - _ROOT_VIEW_KEYS
    if unknown_view:
        raise SandboxError(f"root view has unknown keys {sorted(unknown_view)}; refusing")
    for key in ("home", "home_layer", "home_mode", "cwd"):
        if key in view and not isinstance(view[key], str):
            raise SandboxError(f"root view {key} must be a string; refusing")
    for key in ("home_aliases", "home_binds", "read_only", "masks", "propagate", "git_config_masks"):
        if key in view and (
            not isinstance(view[key], list) or not all(isinstance(item, str) for item in view[key])
        ):
            raise SandboxError(f"root view {key} must be a list of strings; refusing")
    if "binds" in view and (
        not isinstance(view["binds"], list)
        or not all(
            isinstance(pair, list) and len(pair) == 2 and all(isinstance(item, str) for item in pair)
            for pair in view["binds"]
        )
    ):
        raise SandboxError("root view binds must be a list of [source, target] string pairs; refusing")
    for key in ("protect_system", "eden_root_cover"):
        if key in view and not isinstance(view[key], bool):
            raise SandboxError(f"root view {key} must be a boolean; refusing")
    for key in ("eden_checkout", "eden_backing", "git_config_policy"):
        if key in view and view[key] is not None and not isinstance(view[key], str):
            raise SandboxError(f"root view {key} must be a string or null; refusing")
    tmp_size = view.get("tmp_size", DEFAULT_TMP_SIZE)
    if not _tmp_size_within_cap(tmp_size, host_memory_bytes):
        raise SandboxError(f"root view tmp_size {tmp_size!r} is invalid; refusing")
    return view


def _verify_registered(
    document: Mapping[str, object], view: Mapping[str, object], uid: int
) -> dict[str, str]:
    """Derive the trusted roots for this launch; refuse anything else.

    Runs AFTER the privilege drop, as the invoking user. Home comes from
    the password database, slot and project paths from the registry row
    compared exactly, control from the claim after an ownership check.
    Registry reads are nofollow, nonblocking, regular-file only.
    Returns {"home", "control", "slot_path", "project_root"} (slot keys
    empty for a coordinator box).
    """

    import pwd

    registered = document.get("registered")
    if not isinstance(registered, dict):
        raise SandboxError("the root launcher spec names no registered slot; refusing")
    control = registered.get("control")
    if not isinstance(control, str) or not os.path.isabs(control):
        raise SandboxError("the root launcher spec names no control directory; refusing")
    try:
        info = os.stat(control)
    except OSError as exc:
        raise SandboxError(f"the root launcher control directory cannot be inspected: {exc}") from exc
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != uid:
        raise SandboxError("the root launcher control directory is not the invoking user's; refusing")
    try:
        home = pwd.getpwuid(uid).pw_dir
    except KeyError as exc:
        raise SandboxError(f"invoking uid {uid} has no passwd entry") from exc
    claimed_home = os.path.realpath(str(view.get("home", "")))
    if claimed_home != os.path.realpath(home):
        # Root mode takes the passwd home only: nothing the invoking uid
        # can write (including a caller-controlled HOME) is a trust anchor.
        raise SandboxError("the root view home is not the invoking user's passwd home; refusing")
    derived_project = str(Path(control).parent)
    if registered.get("kind") == "box":
        return {"home": home, "control": control, "slot_path": "", "project_root": derived_project}
    if registered.get("kind") != "slot":
        raise SandboxError("the root launcher spec names an unknown registration kind; refusing")
    slot = registered.get("slot")
    slot_path = registered.get("slot_path")
    if not isinstance(slot, str) or not slot or not isinstance(slot_path, str):
        raise SandboxError("the root launcher spec names no slot; refusing")
    claimed_project = registered.get("project_root")
    # The registry's project root is the control directory's parent; a
    # claimed root is accepted only when the row resolves exactly under
    # it too (fixtures and flat layouts name it differently).
    bases = [derived_project]
    if isinstance(claimed_project, str) and claimed_project not in bases:
        bases.append(claimed_project)
    project_root = derived_project
    for active in Path(control).glob("ACTIVE.*.json"):
        try:
            descriptor = os.open(
                active, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC | os.O_NONBLOCK
            )
        except OSError:
            continue
        try:
            if not stat.S_ISREG(os.fstat(descriptor).st_mode):
                continue
            document_active = json.loads(os.read(descriptor, 16 << 20).decode("utf-8"))
        except (OSError, ValueError):
            continue
        finally:
            os.close(descriptor)
        rows = document_active.get("slots") if isinstance(document_active, dict) else None
        if not isinstance(rows, list):
            continue
        for row in rows:
            if not isinstance(row, dict) or row.get("slot") != slot:
                continue
            checkouts = row.get("checkouts")
            if not isinstance(checkouts, list):
                continue
            for checkout in checkouts:
                if not isinstance(checkout, dict):
                    continue
                relative = checkout.get("path")
                if not isinstance(relative, str):
                    continue
                claimed = os.path.realpath(slot_path)
                # Hard bounds whatever the registry says: slot roots live
                # beneath the control area, never at or over a system path.
                if any(
                    claimed == prefix or claimed.startswith(prefix + "/")
                    for prefix in _CLAIM_PREFIXES
                ):
                    continue
                if not any(
                    claimed == os.path.realpath(base)
                    or claimed.startswith(os.path.realpath(base) + "/")
                    for base in {control, *bases}
                ):
                    continue
                for base in bases:
                    resolved = os.path.realpath(os.path.join(base, relative))
                    if resolved == claimed or resolved.startswith(claimed + "/"):
                        return {
                            "home": home,
                            "control": control,
                            "slot_path": claimed,
                            "project_root": base,
                        }
    raise SandboxError(f"slot {slot!r} is not registered in {control}; refusing the root launch")


def _allowed_roots(
    trusted: Mapping[str, str], view: Mapping[str, object], uid: int
) -> list[str]:
    """Roots from TRUSTED data only: passwd home, registry slot/project,
    control, the per-slot state directory, the box `/tmp`, and the
    invoking user's runtime-spec and credential directories (per-user,
    justified in the issue file). Spec-claimed aliases and cwd are NOT
    roots; they must resolve beneath one of these."""

    import pwd

    roots = [trusted["home"], trusted["control"], trusted["project_root"], "/tmp"]
    if trusted.get("slot_path"):
        roots.append(trusted["slot_path"])
    roots.append(str(Path(str(view["home_layer"])).parent))
    roots.append(f"/run/user/{uid}/wrkslots")
    try:
        roots.append(f"/var/facebook/credentials/{pwd.getpwuid(uid).pw_name}")
    except KeyError:
        pass
    return [os.path.realpath(root) for root in roots if root]


def _check_root_ownership(roots: Sequence[str], uid: int) -> None:
    """No ancestor of an allowed root is writable by the invoking user
    through group/other permissions (sticky world-writable, e.g. /tmp,
    cannot have entries replaced and is accepted)."""

    try:
        import grp

        user_groups = {grp.getgrgid(gid).gr_name for gid in os.getgroups()}
    except (KeyError, ImportError):
        user_groups = set()
    for root in roots:
        current = root
        first = True
        while True:
            try:
                info = os.stat(current)
            except FileNotFoundError:
                break  # a not-yet-created state root: nothing crosses it yet
            except OSError as exc:
                raise SandboxError(f"allowed root {root!r} cannot be inspected: {exc}") from exc
            if info.st_uid == 0 and not first:
                # Root-owned system territory: the host's own layout,
                # trusted as far as this walk is concerned.
                parent = str(Path(current).parent)
                if parent == current:
                    break
                current = parent
                continue
            mode = stat.S_IMODE(info.st_mode)
            world = bool(mode & 0o002 and not mode & 0o1000)
            try:
                import grp as _grp

                group_name = _grp.getgrgid(info.st_gid).gr_name
            except (KeyError, ImportError):
                group_name = ""
            group = bool(
                mode & 0o020
                and info.st_uid not in (uid, 0)
                and group_name in user_groups
            )
            if world or group:
                raise SandboxError(
                    f"allowed root {root!r} crosses writable {current!r}; refusing"
                )
            first = False
            parent = str(Path(current).parent)
            if parent == current:
                break
            current = parent


def _beneath(path: str, roots: Sequence[str]) -> bool:
    return any(path == root or path.startswith(root + "/") for root in roots)


def _openat2_strict(path: str) -> int:
    """Open `path` with symlink-free resolution; fail closed without openat2."""

    import struct

    libc = _libc()
    if not hasattr(libc, "syscall"):
        raise SandboxError("root launch needs openat2 (unavailable); refusing")
    # struct open_how { u64 flags; u64 mode; u64 resolve; }
    how = ctypes.create_string_buffer(struct.pack("QQQ", os.O_PATH | os.O_CLOEXEC, 0, 0x04 | 0x02), 24)
    result = libc.syscall(437, -100, path.encode(), ctypes.byref(how), 24)
    if result < 0:
        error = ctypes.get_errno()
        if error == 38:  # ENOSYS
            raise SandboxError("root launch needs openat2 (unavailable); refusing")
        raise MountError(
            f"root view path {path!r} does not resolve without symlinks ({os.strerror(error)}); refusing",
            error,
        )
    return int(result)


def _force_root_policy(
    view: dict[str, object], trusted: Mapping[str, str], uid: int
) -> None:
    """Force root-mode mount policy from trusted values, never the spec.

    Eden keys refuse (a writable user-owned `/` is never a root view);
    `protect_system` is forced on (build_view reads any falsy value as
    off, so refusal alone would not be enough); propagation is forced
    to the trusted control/project roots for a box and to none for a
    slot, whatever the spec claimed (#1583, findings N3/N4).
    """

    if view.get("eden_root_cover") or view.get("eden_checkout") or view.get("eden_backing"):
        raise SandboxError("root mode refuses Eden view keys; refusing")
    view["protect_system"] = True
    if trusted.get("slot_path"):
        view["propagate"] = []
    else:
        # A coordinator box propagates only its trusted control scope.
        view["propagate"] = [trusted["control"]]


def _check_root_paths(
    view: Mapping[str, object],
    trusted: Mapping[str, str],
    uid: int,
    *,
    built: bool = False,
    roots_override: list[str] | None = None,
) -> list[str]:
    """Every source and target beneath a trusted root, resolved strictly.

    Runs as the invoking user (after the drop) and again against the
    built view (`built`: roots re-resolve in the view, where the home
    layer bind makes the passwd home spell itself). The system-prefix
    denylist stays as belt and braces.
    """

    # The state directory (home layer's parent) is computed from the
    # trusted control root, never taken from the spec unchecked.
    layer_parent = os.path.realpath(str(Path(str(view["home_layer"])).parent))
    if not _beneath(layer_parent, [os.path.realpath(trusted["control"])]):
        raise SandboxError("the root view state directory is outside the control directory; refusing")
    roots = roots_override if roots_override is not None else _allowed_roots(trusted, view, uid)
    # A home alias (another mount path exposing the same directory) is a
    # root only when it is that same directory (device+inode) or an
    # ancestor of it.
    home_resolved = os.path.realpath(str(view["home"]))
    try:
        home_stat = os.stat(home_resolved)
        home_id = (home_stat.st_dev, home_stat.st_ino)
    except OSError:
        home_id = None
    aliases = view.get("home_aliases", [])
    if isinstance(aliases, list):
        for alias in aliases:
            resolved_alias = os.path.realpath(str(alias))
            try:
                alias_stat = os.stat(resolved_alias)
                same = (alias_stat.st_dev, alias_stat.st_ino) == home_id
            except OSError:
                same = False
            if same or home_resolved.startswith(resolved_alias + "/"):
                roots.append(resolved_alias)
    paths: list[str] = [str(view["home"]), str(view["home_layer"]), str(view.get("cwd", "/"))]
    raw_binds = view.get("binds", [])
    if isinstance(raw_binds, list):
        for pair in raw_binds:
            if isinstance(pair, list) and len(pair) == 2:
                paths.extend([str(pair[0]), str(pair[1])])
    for key in ("read_only", "home_aliases"):
        value = view.get(key, [])
        if isinstance(value, list):
            paths.extend(str(item) for item in value)
    home_binds = view.get("home_binds", [])
    if isinstance(home_binds, list):
        paths.extend(os.path.join(str(view["home"]), str(item)) for item in home_binds)
    for path in paths:
        if not os.path.isabs(path):
            raise SandboxError(f"root view path {path!r} is not absolute; refusing")
        descriptor = _openat2_strict(path) if os.path.exists(path) else None
        if descriptor is not None:
            os.close(descriptor)
        resolved = os.path.realpath(path)
        if not _beneath(resolved, roots):
            raise SandboxError(
                f"root view path {path!r} resolves outside the allowed roots {roots}; refusing"
            )
        for prefix in _SYSTEM_TARGET_PREFIXES:
            if resolved == prefix or resolved.startswith(prefix + "/"):
                raise SandboxError(
                    f"root view path {path!r} resolves over system path {prefix}; refusing"
                )
    # Masks bring no content (they cover with tmpfs or /dev/null), so they
    # may name outside paths (an Eden backing to hide); only a cover over
    # a system path itself is refused.
    masks = view.get("masks", [])
    if isinstance(masks, list):
        for item in masks:
            path = str(item)
            if not os.path.isabs(path):
                raise SandboxError(f"root view mask {path!r} is not absolute; refusing")
            resolved = os.path.realpath(path)
            for prefix in _SYSTEM_TARGET_PREFIXES:
                if resolved == prefix or resolved.startswith(prefix + "/"):
                    raise SandboxError(
                        f"root view mask {path!r} resolves over system path {prefix}; refusing"
                    )
    return roots


def _caller_groups(uid: int) -> list[int] | None:
    """The invoking caller's groups: walk sudo's monitor chain to the
    first ancestor running as `uid` and read its groups (bounded)."""

    try:
        pid = os.getppid()
        for _hop in range(6):
            status = Path(f"/proc/{pid}/status").read_text(encoding="utf-8")
            fields = {
                line.split(":", 1)[0]: line.split(":", 1)[1].strip()
                for line in status.splitlines()
                if ":" in line
            }
            # Effective uid: sudo's own processes carry the user's real
            # uid but root effective, with sanitized groups - skip them.
            if int(fields["Uid"].split()[1]) == uid:
                return sorted({int(item) for item in fields["Groups"].split()})
            pid = int(fields["PPid"])
            if pid <= 1:
                break
    except (OSError, ValueError, KeyError):
        return None
    return None


def _remount_nosuid_nodev(target: str, *, strict_fd: bool = False, strict_roots: Sequence[str] = ()) -> None:
    """Remount the TOPMOST bind at `target` NOSUID|NODEV, preserving flags."""

    flags = _MS_BIND | _MS_REMOUNT | _MS_NOSUID | _MS_NODEV
    options: set[str] = set()
    try:
        for line in Path("/proc/self/mounts").read_text(encoding="utf-8").splitlines():
            fields = line.split()
            if len(fields) >= 4 and _unescape(fields[1]) == target:
                options = set(fields[3].split(","))  # later rows are topmost
    except OSError as exc:
        raise SandboxError(f"cannot read the mount table for {target!r}: {exc}") from exc
    if "ro" in options:
        flags |= _MS_RDONLY
    if "noexec" in options:
        flags |= _MS_NOEXEC
    if strict_fd:
        descriptor = _resolve_fd(target, strict_roots, target=True)
        try:
            # Only NOSUID|NODEV, recursively: naming other attributes
            # (an atime mode above all) can make mount_setattr refuse.
            _mount_setattr_fd(
                descriptor, attr_set=_ATTR_NOSUID | _ATTR_NODEV, recursive=True
            )
        finally:
            os.close(descriptor)
    else:
        _mount_at_fd(None, target, None, flags)


def enter_root() -> None:
    """``sudo -n`` side of ``isolation: root``: build the view, drop to the user, run the command.

    The spec arrives on stdin (no path argument, so no root-side lookup or
    unlink of a user path ever happens). The view is built mount-by-fd
    (#1792): targets resolve once with openat2 beneath their trusted
    root and mounts move onto the kept fd, so a swapped name cannot
    redirect a bind, and the build refuses if the fd mount APIs are
    unavailable. Privilege is shed FIRST, keeping
    CAP_SYS_ADMIN only in the permitted set (raised effective only for
    the mount phase, after the namespace is unshared and the spec is
    parsed), and every check on the spec then runs as the invoking user:
    trusted roots, closed schema, no spec mount policy. Dumpable stays
    0 from entry to exec, and the exec is sealed with
    PR_SET_NO_NEW_PRIVS: root mode is no-setuid by decision (#1583), so
    harness launchers that need a setuid step fail honestly here and
    userns remains their mount-sandboxed route.

    No user namespace is created. No PID namespace either: harness
    launchers that talk to the user's systemd manager fail inside one,
    so host processes stay visible. That means the box is NOT a
    containment boundary against same-uid code: a boxed process can
    reach the host view through /proc/<pid>/root|cwd|fd of same-uid
    processes, can ptrace same-uid processes the kernel allows, and
    can start work outside the box through the user's systemd bus
    (the runtime control directory stays reachable by design). The
    box confines mounts, writes and resource limits; it does not
    confine a determined same-uid program (see the user guide's
    "Limits of the box").
    """

    if os.geteuid() != 0:
        raise SandboxError("enter-root must run as root (through sudo -n)")
    # No PDEATHSIG promise in root mode: the uid drop below clears it
    # (#1792, item I3). Test cleanup is the harness's slice stop.
    _prctl(_PR_SET_DUMPABLE, 0)
    uid, gid, db_groups, name = _sudo_identity()
    caller = _caller_groups(uid)
    groups = sorted(set(caller) & set(db_groups)) if caller is not None else db_groups
    os.setgroups(groups)
    os.setresgid(gid, gid, gid)
    _prctl(_PR_SET_KEEPCAPS, 1)
    os.setresuid(uid, uid, uid)
    _set_capabilities_permitted([_CAP_SYS_ADMIN])
    _prctl(_PR_SET_KEEPCAPS, 0)
    # Unshare needs the capability effective; lower it again at once, so
    # parsing and checks run with nothing effective (finding N5).
    _set_capabilities_split([_CAP_SYS_ADMIN], [_CAP_SYS_ADMIN])
    _check(_libc().unshare(_CLONE_NEWNS), "unshare mount namespace")
    _set_capabilities_permitted([_CAP_SYS_ADMIN])
    raw = sys.stdin.buffer.read(_SPEC_BYTES_LIMIT + 1)
    if len(raw) > _SPEC_BYTES_LIMIT:
        raise SandboxError("the root launcher spec is too large; refusing")
    try:
        document = json.loads(raw)
    except ValueError as exc:
        raise SandboxError(f"the root launcher spec is not JSON: {exc}") from exc
    if not isinstance(document, dict):
        raise SandboxError("root sandbox spec must be a JSON object")
    view = _check_root_schema(document)
    trusted = _verify_registered(document, view, uid)
    _force_root_policy(view, trusted, uid)
    _check_root_ownership(_allowed_roots(trusted, view, uid), uid)
    checked_roots = _check_root_paths(view, trusted, uid)
    raw_command = document["command"]
    raw_environment = document["environment"]
    raw_groups = document.get("groups", [])
    if not isinstance(raw_command, list) or not raw_command or not isinstance(raw_environment, dict):
        raise SandboxError("the root launcher spec has no command or environment")
    if not isinstance(raw_groups, list) or not all(isinstance(item, int) for item in raw_groups):
        raise SandboxError("the root launcher spec has invalid groups")
    command = [str(item) for item in raw_command]
    environment = {str(key): str(value) for key, value in raw_environment.items()}
    if document.get("uid") != uid or document.get("gid") != gid:
        raise SandboxError(
            f"the root launcher spec names uid/gid {document.get('uid')}/{document.get('gid')}, "
            f"but sudo was invoked by {name} ({uid}/{gid}); refusing"
        )
    # The group database is an incomplete source on NSS hosts; the
    # caller's own groups (read from its /proc status at the drop) are
    # the authority the claimed set must fit inside.
    if caller is None or not set(raw_groups) <= set(caller):
        raise SandboxError("the root launcher spec groups are not the caller's own; refusing")
    os.umask(int(str(document.get("umask", 0o022))))
    _set_capabilities([_CAP_SYS_ADMIN])
    try:
        build_view(view, strict_fd=True, strict_roots=checked_roots)
        _verify_mount_points(view, records=_STRICT_BIND_RECORDS, roots=checked_roots)
        raw_binds = view.get("binds", [])
        if isinstance(raw_binds, list):
            for pair in raw_binds:
                if isinstance(pair, list) and len(pair) == 2:
                    _remount_nosuid_nodev(str(pair[1]), strict_fd=True, strict_roots=checked_roots)
    except BaseException:
        # No post-bind code may run with the capability held (#1792):
        # drop it before the failure propagates to error handling.
        _set_capabilities([])
        raise
    _set_capabilities([])
    _check_root_paths(view, trusted, uid, built=True, roots_override=checked_roots)
    command = _traced(view, command, environment)
    if os.getuid() != uid or os.geteuid() != uid or os.getgid() != gid:
        raise SandboxError("privilege drop did not take effect")
    _prctl(_PR_SET_NO_NEW_PRIVS, 1)
    os.execvpe(command[0], command, environment)


def _main(argv: Sequence[str] | None = None) -> int:
    """Internal entry points.

    ``sandbox.py enter SPEC-JSON -- COMMAND...``   (userns, as the user)
    ``sandbox.py enter-root``                      (root, through sudo -n; spec on stdin)
    """

    values = list(sys.argv[1:] if argv is None else argv)
    try:
        if len(values) == 1 and values[0] == "enter-root":
            enter_root()
        elif len(values) >= 4 and values[0] == "enter" and values[2] == "--":
            # The user-namespace entry must never run privileged: a sudoers
            # line without pinned args would otherwise exec as root (#1583).
            if os.geteuid() == 0 or "SUDO_UID" in os.environ or "SUDO_GID" in os.environ:
                raise SandboxError("the userns entry refuses to run as root or under sudo")
            loaded: object = json.loads(values[1])
            if not isinstance(loaded, dict):
                raise SandboxError("sandbox spec must be a JSON object")
            enter_userns({str(key): value for key, value in loaded.items()}, values[3:])
        else:
            print(
                "usage: sandbox.py enter SPEC-JSON -- COMMAND... | sandbox.py enter-root (spec on stdin)",
                file=sys.stderr,
            )
            return 2
    except (SandboxError, OSError, KeyError, ValueError) as exc:
        print(f"wrkslots sandbox: {exc}", file=sys.stderr)
        return 126
    return 127


# --------------------------------------------------------------- the launcher


def _interpreter() -> str:
    # The real interpreter, not a wrapper script that forks: the PID must not change.
    return os.path.realpath(sys.executable)


def userns_helper(spec: Mapping[str, object], command: Sequence[str]) -> list[str]:
    """Argv that builds the view in a user namespace, then execs ``command``."""

    if not command:
        raise SandboxError("no command to run")
    return [
        _interpreter(),
        "-I",
        "-S",
        str(Path(__file__).resolve()),
        "enter",
        json.dumps(dict(spec), sort_keys=True),
        "--",
        *command,
    ]


#: Version of the root-helper protocol in this file. The launcher refuses
#: an installed copy whose version differs (#1583, review finding 5).
_HELPER_VERSION = "2"


def _installed_helper_version(path: Path) -> str | None:
    try:
        for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
            if line.startswith("_HELPER_VERSION"):
                return line.partition("=")[2].strip().strip('"').strip("'")
    except OSError:
        return None
    return None


def root_helper() -> list[str]:
    """Argv that builds the view as root through ``sudo -n``, then drops privilege.

    The spec travels on stdin, so the argv is fixed (no path, no wildcard).
    Always ``-I -B``: ``-I`` ignores ``PYTHONDONTWRITEBYTECODE`` and sudo
    scrubs the environment anyway. The root-owned installed copy is the
    only path a sudoers entry may safely name; a version mismatch with
    this file refuses instead of silently running a stale helper.
    """

    script = Path(__file__).resolve()
    if _INSTALLED_HELPER.is_file():
        if _installed_helper_version(_INSTALLED_HELPER) != _HELPER_VERSION:
            raise SandboxError(
                f"installed root helper {_INSTALLED_HELPER} is not version {_HELPER_VERSION}; refusing"
            )
        script = _INSTALLED_HELPER
    elif os.environ.get("WRKSLOTS_ALLOW_CHECKOUT_HELPER") != "1":
        # On a passwordless-sudo host the checkout copy is agent-writable
        # code run as root; only an explicit opt-in may use it (#1583).
        raise SandboxError(
            f"no installed root helper at {_INSTALLED_HELPER}; refusing the "
            "user-writable checkout copy (set WRKSLOTS_ALLOW_CHECKOUT_HELPER=1 to opt in)"
        )
    return ["sudo", "-n", "--", _interpreter(), "-I", "-B", "-S", str(script), "enter-root"]


def _exec_root_helper(argv: Sequence[str], spec_file: str, environment: Mapping[str, str]) -> None:
    """Exec the root helper with the spec file as its stdin (never its argv)."""

    descriptor = os.open(spec_file, os.O_RDONLY | os.O_CLOEXEC)
    try:
        os.dup2(descriptor, 0)
    finally:
        os.close(descriptor)
    os.execvpe(argv[0], list(argv), dict(environment))


def check_sudo(*, deadline_ns: int | None = None) -> None:
    """Fail clearly, never prompt, when ``sudo -n`` cannot run without a password.

    ``deadline_ns`` (CLOCK_MONOTONIC) bounds the probe; a probe still running
    at the deadline is killed and refused.
    """

    if shutil.which("sudo") is None:
        raise SandboxError("isolation root needs sudo, which is not installed")
    what = "`sudo -n true`"
    try:
        result = subprocess.run(
            ["sudo", "-n", "true"],
            capture_output=True,
            text=True,
            check=False,
            timeout=_timeout(deadline_ns, what),
        )
    except subprocess.TimeoutExpired as exc:
        raise _deadline_refusal(what) from exc
    if result.returncode != 0:
        detail = result.stderr.strip() or f"exit {result.returncode}"
        raise SandboxError(
            f"isolation root needs passwordless sudo, and `sudo -n true` failed: {detail}; "
            "use isolation userns or cgroup instead"
        )


def spec_directory() -> Path | None:
    """Private directory for root-launcher specs: in the per-user runtime tmpfs, never on disk."""

    runtime = os.environ.get("XDG_RUNTIME_DIR")
    if not runtime or not os.path.isdir(runtime):
        return None
    return Path(runtime) / "wrkslots"


def ensure_spec_directory(directory: Path) -> None:
    """Create the root-launcher spec directory (0700) and check it is a real directory we own."""

    try:
        directory.mkdir(mode=0o700, exist_ok=True)
        info = os.lstat(directory)
    except OSError as exc:
        raise SandboxError(f"cannot create the root launcher spec directory {directory}: {exc}") from exc
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
        raise SandboxError(
            f"root launcher spec directory {directory} must be a directory owned by uid "
            f"{os.getuid()} with mode 0700; remove it and retry"
        )


_STALE_SPEC_SECONDS = 300


def write_root_spec(
    spec: Mapping[str, object], command: Sequence[str], environment: Mapping[str, str],
    *, registered: Mapping[str, object] | None = None,
) -> str:
    """Write the root helper's input to a 0600 file in the user's runtime tmpfs.

    The directory is private (0700) and read-only inside every box, so a boxed
    process cannot tamper with another launch's spec. The helper deletes the
    file as soon as it has read it; specs a failed sudo left behind are removed
    by the next launch.
    """

    directory = spec_directory()
    if directory is None:
        raise SandboxError("isolation root needs XDG_RUNTIME_DIR (a per-user tmpfs) for its launch spec")
    directory.mkdir(mode=0o700, exist_ok=True)
    info = directory.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid():
        raise SandboxError(f"{directory} is not a directory owned by this user")
    os.chmod(directory, 0o700)
    now = time.time()
    for stale in directory.glob("sandbox-*.json"):
        with contextlib.suppress(OSError):
            if now - stale.lstat().st_mtime > _STALE_SPEC_SECONDS:
                stale.unlink()
    umask = os.umask(0o077)
    os.umask(umask)
    document = {
        "view": dict(spec),
        "command": list(command),
        "environment": dict(environment),
        "uid": os.getuid(),
        "gid": os.getgid(),
        "groups": sorted(set(os.getgroups())),
        "umask": umask,
        # What the root helper must find registered before it builds
        # anything (#1583): {"kind": "slot", "slot", "slot_path", "control"}
        # or {"kind": "box", "control"} for a coordinator box.
        "registered": dict(registered) if registered is not None else None,
    }
    descriptor, path = tempfile.mkstemp(prefix="sandbox-", suffix=".json", dir=directory)
    try:
        os.fchmod(descriptor, 0o600)
        with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
            json.dump(document, handle)
    except BaseException:
        with contextlib.suppress(OSError):
            os.unlink(path)
        raise
    return path


def scope_in_place_available() -> bool:
    """Whether a process can join a transient scope without forking (the D-Bus path)."""

    if shutil.which("busctl") is None:
        return False
    result = subprocess.run(
        ["busctl", "--user", "status", "org.freedesktop.systemd1"],
        capture_output=True,
        text=True,
        check=False,
        timeout=10,
    )
    return result.returncode == 0


def place_in_scope(unit: str, slice_unit: str) -> bool:
    """Move THIS process into a new transient scope without forking.

    Keeping the PID matters: a terminal multiplexer that checks whether a pane's
    own shell is at its prompt (Herdr) must still see the same process after
    ``exec wrkslots run ...`` replaces the pane's shell. Returns False when the
    user manager's D-Bus API is unavailable, so the caller can fall back to
    ``systemd-run --scope`` (which forks).
    """

    if shutil.which("busctl") is None:
        return False
    result = subprocess.run(
        [
            "busctl",
            "--user",
            "call",
            "org.freedesktop.systemd1",
            "/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager",
            "StartTransientUnit",
            "ssa(sv)a(sa(sv))",
            unit,
            "fail",
            "2",
            "PIDs",
            "au",
            "1",
            str(os.getpid()),
            "Slice",
            "s",
            slice_unit,
            "0",
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        return False
    for _ in range(100):
        try:
            if f"/{unit}" in Path("/proc/self/cgroup").read_text(encoding="utf-8"):
                return True
        except OSError:
            break
        time.sleep(0.01)
    return False


def run(
    view: SlotView,
    settings: SandboxSettings,
    command: Sequence[str],
    *,
    cwd: Path | None = None,
    print_only: bool = False,
    unit: str | None = None,
) -> int:
    """Replace this process with ``command`` boxed to the slot (or coordinator box).

    Returns only for ``print_only``.
    """

    isolation = settings.isolation
    if isolation not in ISOLATIONS:
        raise SandboxError(f"isolation must be one of {', '.join(ISOLATIONS)}")
    if not command:
        raise SandboxError("no command to run")
    if shutil.which("systemd-run") is None:
        raise SandboxError("systemd-run is not available; the sandbox needs a systemd user manager")
    home = Path(os.environ.get("HOME", str(Path.home())))
    kind = "box" if view.coordinator else "run"
    unit_name = unit or f"wrkslots-{kind}-{systemd_escape(view.slot)}-{os.getpid()}.scope"
    slot_slice = slice_name(view.slot_type, view.slot)
    workdir = cwd or view.slot_path
    if print_only:
        spec = build_spec(view, settings, home, workdir)
        print(f"# isolation {isolation}; scope {unit_name} in {slot_slice}")
        for line in slice_limit_properties(settings.limits):
            print(f"# slice {slot_slice}: {line}")
        for key, value in settings.env:
            print(f"# env {key}={_expand_home(value, str(home))}")
        if isolation == "userns":
            print(" \\\n  ".join(_quote(part) for part in userns_helper(spec, command)))
        elif isolation == "root":
            print(" ".join(_quote(part) for part in root_helper()))
            print(json.dumps({"view": spec, "command": list(command)}, indent=2, sort_keys=True))
        else:
            print(" ".join(_quote(part) for part in command))
        return 0
    if not view.coordinator and isolation != "cgroup" and os.environ.get("WRKSLOTS_BOX"):
        # The coordinator box masks slot-state (other slots' private $HOME
        # layers, which hold seeded credential copies), so a slot's layer cannot
        # be prepared from inside it.
        raise SandboxError(
            f"cannot box a command into slot {view.slot} from inside a coordinator box: the "
            "coordinator box hides every slot's private state. Launch slot agents with "
            "`agentctl start ... --slot` (their terminal panes start outside this box), or run "
            "`wrkslots run` outside the box"
        )
    if isolation == "root":
        check_sudo()
    if isolation != "cgroup":
        try:
            prepare_state(view, settings, home)
        except OSError as exc:
            raise SandboxError(f"cannot prepare the private state of {view.slot}: {exc}") from exc
    for line in apply_slice_limits(view, settings.limits):
        print(f"wrkslots run: limits {line}", file=sys.stderr)
    environment = child_environment(view, settings, os.environ)
    root_spec: str | None = None
    if isolation == "userns":
        argv = userns_helper(build_spec(view, settings, home, workdir), command)
    elif isolation == "root":
        root_spec = write_root_spec(
            build_spec(view, settings, home, workdir), command, environment,
            registered=_registered_claim(view),
        )
        argv = root_helper()
    else:
        os.chdir(workdir)
        argv = list(command)
    # The scope is established BEFORE sudo in root mode, so the root helper and
    # the command inherit the slot's cgroup.
    try:
        if place_in_scope(unit_name, slot_slice):
            if root_spec is not None:
                _exec_root_helper(argv, root_spec, environment)
            os.execvpe(argv[0], argv, environment)
    except OSError:
        if root_spec is not None:
            with contextlib.suppress(OSError):
                os.unlink(root_spec)
        raise
    print(
        "wrkslots run: the user manager's D-Bus API is unavailable, so the command runs under "
        "`systemd-run --scope`; its process ID differs from the caller's",
        file=sys.stderr,
    )
    fallback = [
        "systemd-run",
        "--user",
        "--scope",
        "--quiet",
        "--collect",
        f"--unit={unit_name}",
        f"--slice={slot_slice}",
        "--",
        *argv,
    ]
    if root_spec is not None:
        _exec_root_helper(fallback, root_spec, environment)
    os.execvpe(fallback[0], fallback, environment)


def _slice_chain(slice_unit: str) -> list[str]:
    """``a-b-c.slice`` -> ``[a.slice, a-b.slice, a-b-c.slice]``: its cgroup path below the manager."""

    parts = slice_unit.removesuffix(".slice").split("-")
    return ["-".join(parts[: index + 1]) + ".slice" for index in range(len(parts))]


def verify_in_scope(
    view: SlotView,
    unit: str,
    expected: SlicePlacement,
    *,
    deadline_ns: int | None = None,
    cgroup_file: Path = Path("/proc/self/cgroup"),
) -> None:
    """Refuse unless this process is in ``unit`` under exactly the ``expected`` slices.

    The slices are never rediscovered: :func:`parent_slice` is not called. The
    triple must be this slot's (the names :func:`place_only` derives from its
    ``enclosing`` value), and the process's cgroup below the user manager must
    be exactly that slot slice's chain followed by ``unit``.
    """

    if not unit.endswith(".scope") or "/" in unit or unit == ".scope":
        raise SandboxError(f"--in-scope needs a scope unit name such as run-1.scope, not {unit!r}")
    enclosing = expected.enclosing
    if enclosing is not None and (not enclosing.endswith(".slice") or "/" in enclosing):
        raise SandboxError(f"the expected enclosing slice {enclosing!r} is not a slice name")
    parent = NO_ENCLOSING_SLICE if enclosing is None else enclosing
    derived = (root_slice(parent), slice_name(view.slot_type, view.slot, parent, deadline_ns=deadline_ns))
    if (expected.all_slots, expected.slot) != derived:
        raise SandboxError(
            f"the expected slices {expected.all_slots}, {expected.slot} are not slot {view.slot}'s "
            f"under {enclosing or 'no enclosing slice'} ({derived[0]}, {derived[1]})"
        )
    try:
        text = cgroup_file.read_text(encoding="utf-8")
    except OSError as exc:
        raise SandboxError(f"cannot read {cgroup_file}: {exc}") from exc
    path = next((line[3:] for line in text.splitlines() if line.startswith("0::")), None)
    if path is None:
        raise SandboxError(f"{cgroup_file} has no unified (0::) cgroup line")
    parts = path.split("/")
    manager = next((index for index, part in enumerate(parts) if part.startswith("user@")), None)
    below = [] if manager is None else parts[manager + 1 :]
    wanted = [*_slice_chain(expected.slot), unit]
    if manager is None or below != wanted:
        raise SandboxError(
            f"this process is in {path}, not in {unit} directly under {expected.slot} "
            f"({'/'.join(wanted)} below the user manager); refusing to enter the box"
        )


def in_scope_plan(
    view: SlotView,
    settings: SandboxSettings,
    command: Sequence[str],
    *,
    unit: str,
    expected: SlicePlacement,
    cwd: Path | None = None,
    slot_writable: bool = True,
    deadline_ns: int | None = None,
    expected_writable: Mapping[str, object] | None = None,
    git_config_policy: str = "inherit",
    expected_masks: Mapping[str, object] | None = None,
    cgroup_file: Path = Path("/proc/self/cgroup"),
) -> dict[str, object]:
    """What :func:`enter_in_place` would do, writing nothing in the slot's state.

    Verifies the scope and slices exactly as the real entry does, then returns
    the view spec, its complete writable set, its complete masks
    (:func:`mask_report`), and the helper argv. With ``expected_writable``,
    refuses unless that writable set is exactly it; with ``expected_masks``,
    refuses unless the mask report is exactly it.
    """

    _check_in_scope_request(view, settings, command)
    verify_in_scope(view, unit, expected, deadline_ns=deadline_ns, cgroup_file=cgroup_file)
    home = Path(os.environ.get("HOME", str(Path.home())))
    spec = build_spec(
        view, settings, home, cwd or view.slot_path, slot_writable=slot_writable,
        git_config_policy=git_config_policy,
    )
    # Nothing is prepared here; the real entry creates the private
    # directories first, and their binds are part of its writable set.
    writable = writable_set(spec, created_sources=planned_private_sources(view, settings))
    _check_expected_writable(writable, expected_writable)
    masks = mask_report(spec)
    _check_expected_masks(masks, expected_masks)
    return {
        "unit": unit,
        "slices": expected.to_obj(),
        "writable": writable,
        "masks": masks,
        "helper": root_helper(),
        "command": list(command),
        "view": spec,
    }


def _check_expected_writable(actual: Mapping[str, object], expected: Mapping[str, object] | None) -> None:
    """Refuse unless the writable set about to be entered is exactly the one the caller accepted.

    ``expected`` is the ``writable`` object an earlier ``--print`` reported; it is
    compared whole (every bind, in order, and every other member), so a set that
    gained, lost or changed any member is refused. None means no expectation.
    """

    if expected is not None and dict(actual) != dict(expected):
        raise SandboxError(
            "the writable set this entry would have differs from the expected one: "
            f"{json.dumps(actual, sort_keys=True)} is not {json.dumps(expected, sort_keys=True)}; "
            "refusing to enter the box"
        )


def _check_expected_masks(actual: Mapping[str, object], expected: Mapping[str, object] | None) -> None:
    """Refuse unless the masks about to be applied are exactly the ones the caller accepted.

    ``expected`` is the ``masks`` object an earlier ``--print`` reported: the
    complete ordered mask list, the Git configuration policy, and its paths,
    compared whole, separately from the writable set. None means no expectation.
    """

    if expected is not None and dict(actual) != dict(expected):
        raise SandboxError(
            "the masks this entry would apply differ from the expected ones: "
            f"{json.dumps(actual, sort_keys=True)} is not {json.dumps(expected, sort_keys=True)}; "
            "refusing to enter the box"
        )


def _check_in_scope_request(view: SlotView, settings: SandboxSettings, command: Sequence[str]) -> None:
    if settings.isolation != "root":
        raise SandboxError(
            f"--in-scope builds the view through the root helper and needs isolation root, not "
            f"{settings.isolation}"
        )
    if view.coordinator:
        raise SandboxError("--in-scope enters one slot's box, not a coordinator box")
    if not command:
        raise SandboxError("no command to run")


def enter_in_place(
    view: SlotView,
    settings: SandboxSettings,
    command: Sequence[str],
    *,
    unit: str,
    expected: SlicePlacement,
    cwd: Path | None = None,
    slot_writable: bool = True,
    deadline_ns: int | None = None,
    expected_writable: Mapping[str, object] | None = None,
    git_config_policy: str = "inherit",
    expected_masks: Mapping[str, object] | None = None,
    cgroup_file: Path = Path("/proc/self/cgroup"),
) -> None:
    """Enter the slot's box from inside the caller's own scope; never creates or joins a scope.

    For a caller that placed itself in ``unit`` under the slices
    :func:`place_only` printed. Verifies that placement without rediscovering
    any slice, then runs the ordinary root-isolation steps in order --
    :func:`check_sudo`, :func:`prepare_state`, :func:`child_environment`,
    :func:`write_root_spec`, :func:`root_helper` -- and execs the helper, which
    (:func:`enter_root`) unshares the mount namespace, drops privilege and
    execs ``command``. No slice limit is applied here: place_only did that.
    With ``expected_writable`` (the ``writable`` object a ``--print`` reported),
    the writable set is compared whole before the private state is prepared
    and again from the spec about to be written; any difference refuses and
    nothing is executed. ``expected_masks`` (the ``masks`` object a ``--print``
    reported) is compared the same way, at the same two points, separately.
    With ``git_config_policy="isolated"`` the helper masks the fixed Git
    configuration locations strictly and confirms them inside the view before
    the command starts. Returns only by raising.
    """

    _check_in_scope_request(view, settings, command)
    if os.environ.get("WRKSLOTS_BOX"):
        raise SandboxError(
            f"cannot box a command into slot {view.slot} from inside a coordinator box: the "
            "coordinator box hides every slot's private state; run it outside the box"
        )
    verify_in_scope(view, unit, expected, deadline_ns=deadline_ns, cgroup_file=cgroup_file)
    home = Path(os.environ.get("HOME", str(Path.home())))
    workdir = cwd or view.slot_path
    planned = planned_private_sources(view, settings)
    if expected_writable is not None:
        # Before anything is written: the same set the --print report derived.
        _check_expected_writable(
            writable_set(build_spec(view, settings, home, workdir, slot_writable=slot_writable,
                                    git_config_policy=git_config_policy),
                         created_sources=planned),
            expected_writable,
        )
    if expected_masks is not None:
        _check_expected_masks(
            mask_report(build_spec(view, settings, home, workdir, slot_writable=slot_writable,
                                   git_config_policy=git_config_policy)),
            expected_masks,
        )
    check_sudo(deadline_ns=deadline_ns)
    try:
        prepare_state(view, settings, home)
    except OSError as exc:
        raise SandboxError(f"cannot prepare the private state of {view.slot}: {exc}") from exc
    environment = child_environment(view, settings, os.environ)
    spec = build_spec(view, settings, home, workdir, slot_writable=slot_writable,
                      git_config_policy=git_config_policy)
    # And from the spec the helper will actually build.
    _check_expected_writable(writable_set(spec, created_sources=planned), expected_writable)
    _check_expected_masks(mask_report(spec), expected_masks)
    spec_file = write_root_spec(spec, command, environment, registered=_registered_claim(view))
    argv = root_helper()
    try:
        _timeout(deadline_ns, "entering the box")
        _exec_root_helper(argv, spec_file, environment)
    except BaseException:
        with contextlib.suppress(OSError):
            os.unlink(spec_file)
        raise


def _quote(value: str) -> str:
    import shlex

    return shlex.quote(value)


if __name__ == "__main__":
    raise SystemExit(_main())
