"""Idempotent bring-up of the server -> workspace -> tab -> pane chain.

Idempotent means: running this twice in a row does the work once and reuses it thereafter, and
running it against partially-existing state completes only the missing part. Each level is resolved
by LOOKING FOR IT (by label) before creating it, so two agents racing to bring up the same project
converge on at most one extra tab rather than a growing pile.

The cache is a pure optimisation and is never trusted. Cached ids are re-validated against the live
session (the pane still exists, and still belongs to a tab with the expected label) before use; any
mismatch discards the cache and re-resolves from labels. A cache that could go stale and be believed
would be a proxy for the real session state, which is precisely the failure mode to avoid.
"""

from __future__ import annotations

import json
import os
import tempfile
import fcntl
from collections.abc import Iterator
from contextlib import contextmanager
from dataclasses import dataclass

from herdr_run.client import HerdrClient
from herdr_run.config import Config, render_tab_name
from herdr_run.errors import HerdrUnavailable
from herdr_run.jsonx import as_mapping, opt_str
from herdr_run.state import open_lock_file, session_lock_path

__all__ = ["Target", "resolve_target", "tab_label_for", "cache_path"]


@dataclass(frozen=True)
class Target:
    """The resolved pane, plus what had to be created to get there (for the audit record)."""

    workspace_id: str
    tab_id: str
    pane_id: str
    workspace_label: str
    tab_label: str
    created: tuple[str, ...]
    from_cache: bool


def tab_label_for(config: Config, agent: str) -> str:
    """Expand the project's restricted tab-name schema."""
    project = os.path.basename(os.path.abspath(config.project_root)) or "project"
    return render_tab_name(config.tab_name, agent=agent, project=project)


def cache_path(config: Config) -> str:
    """Path of the resolved-session cache. A pure optimisation; always re-validated before use."""
    return os.path.join(config.project_root, config.spool_dir, "session-cache.json")


def _load_cache(path: str, key: str) -> tuple[str, str, str] | None:
    try:
        with open(path, encoding="utf-8") as handle:
            document: object = json.load(handle)
    except (OSError, UnicodeError, json.JSONDecodeError):
        return None
    try:
        entries = as_mapping(document, "session cache")
        entry = entries.get(key)
        if not isinstance(entry, dict):
            return None
        record = as_mapping(entry, "session cache entry")
        workspace = opt_str(record, "workspace_id")
        tab = opt_str(record, "tab_id")
        pane = opt_str(record, "pane_id")
    except TypeError:
        return None
    if workspace is None or tab is None or pane is None:
        return None
    return workspace, tab, pane


def _store_cache(path: str, key: str, target: Target) -> None:
    parent = os.path.dirname(path)
    os.makedirs(parent, mode=0o700, exist_ok=True)
    # Serialize the whole read/modify/write. Without this, two agents resolving different tabs can
    # both read the old document and the later rename silently loses the earlier cache entry.
    flags = (
        os.O_CREAT
        | os.O_RDWR
        | getattr(os, "O_CLOEXEC", 0)
        | getattr(os, "O_NOFOLLOW", 0)
    )
    lock_descriptor = os.open(f"{path}.lock", flags, 0o600)
    try:
        os.fchmod(lock_descriptor, 0o600)
    except OSError:
        os.close(lock_descriptor)
        raise
    with os.fdopen(lock_descriptor, "r+b") as lock:
        fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
        entries: dict[str, object] = {}
        try:
            with open(path, encoding="utf-8") as handle:
                existing: object = json.load(handle)
            if isinstance(existing, dict):
                entries = as_mapping(existing, "session cache")
        except (OSError, UnicodeError, json.JSONDecodeError, TypeError):
            entries = {}
        entries[key] = {
            "workspace_id": target.workspace_id,
            "tab_id": target.tab_id,
            "pane_id": target.pane_id,
            "workspace_label": target.workspace_label,
            "tab_label": target.tab_label,
        }
        # A unique temporary plus rename keeps readers from seeing a torn file, including callers
        # in another thread of this process (a PID-only temporary name did not).
        temporary = ""
        try:
            with tempfile.NamedTemporaryFile(
                mode="w",
                encoding="utf-8",
                dir=parent,
                prefix=".session-cache.",
                delete=False,
            ) as handle:
                temporary = handle.name
                os.fchmod(handle.fileno(), 0o600)
                json.dump(entries, handle, indent=2, sort_keys=True)
                handle.write("\n")
            os.replace(temporary, path)
            os.chmod(path, 0o600)
        finally:
            if temporary:
                try:
                    os.unlink(temporary)
                except FileNotFoundError:
                    pass


def _pane_of_tab(client: HerdrClient, workspace_id: str, tab_id: str) -> str:
    panes = [pane for pane in client.panes(workspace_id) if pane.tab_id == tab_id]
    if not panes:
        raise HerdrUnavailable(f"tab {tab_id} has no pane")
    if len(panes) > 1:
        # A split tab is ambiguous: "the" pane is no longer well defined, and picking one silently
        # would mean typing into whichever pane happened to sort first.
        ids = ", ".join(pane.pane_id for pane in panes)
        raise HerdrUnavailable(
            f"tab {tab_id} has {len(panes)} panes ({ids}); herdr-run needs an unsplit tab. "
            "Close the extra panes or point tab_name at a different tab."
        )
    return panes[0].pane_id


def _enforce_pane_cap(client: HerdrClient, config: Config, workspace_id: str) -> None:
    """Refuse to open ANOTHER tab once the workspace is already at ``max_panes``.

    Agents are coined continuously and every one that runs a command leaves a tab behind; nothing
    closes them, so the command workspace grows without bound until the Herdr server itself becomes
    the bottleneck (measured: 260 panes, >1000% CPU, every control call timing out). A ceiling turns
    that slow collapse into one legible refusal naming the tool that fixes it.

    Checked ONLY on the create path. An agent whose tab already exists must never be locked out of
    it -- a cap that can break work in progress is a cap that gets switched off -- and a cap that
    refused an existing tab would also make the failure arrive at a random later moment rather than
    when a new tab is actually being added.
    """
    if config.max_panes <= 0:
        return
    existing = len(client.panes(workspace_id))
    if existing < config.max_panes:
        return
    source = config.source_path or "your .herdr-run.yaml"
    raise HerdrUnavailable(
        f"workspace {config.workspace!r} already holds {existing} pane(s) and max_panes is "
        f"{config.max_panes}; refusing to open a tab for this agent. Run 'herdr-run reap' to see "
        f"which tabs are provably finished and can be closed, or raise max_panes in {source} "
        "(0 disables the cap)."
    )


def _cache_still_valid(
    client: HerdrClient,
    cached: tuple[str, str, str],
    workspace_label: str,
    tab_label: str,
) -> bool:
    workspace_id, tab_id, pane_id = cached
    if not client.pane_exists(pane_id):
        return False
    try:
        # IDs may be reused after a server restart. Validate the workspace label as well as the
        # pane/tab relationship before trusting the cached destination.
        if client.workspace_id_for_label(workspace_label) != workspace_id:
            return False
        live_tab = client.tab_id_for_label(workspace_id, tab_label)
    except HerdrUnavailable:
        return False
    if live_tab != tab_id:
        return False
    return any(
        pane.pane_id == pane_id
        and pane.tab_id == tab_id
        and pane.workspace_id == workspace_id
        for pane in client.panes(workspace_id)
    )


@contextmanager
def _session_resolution_lock() -> Iterator[None]:
    """Serialize all account-global Herdr session discovery and creation."""
    path = session_lock_path()
    handle = open_lock_file(path)
    try:
        try:
            fcntl.flock(handle.fileno(), fcntl.LOCK_EX)
        except OSError as exc:
            raise HerdrUnavailable(
                f"cannot lock Herdr session resolution at {path}: {exc}"
            ) from exc
        yield
    finally:
        # Closing releases `flock` even when the protected operation raised.
        handle.close()


def resolve_target(
    client: HerdrClient, config: Config, agent: str, *, use_cache: bool = True
) -> Target:
    """Bring up (or reuse) the workspace/tab/pane for ``agent`` and return the resolved ids."""
    with _session_resolution_lock():
        return _resolve_target_locked(client, config, agent, use_cache=use_cache)


def _resolve_target_locked(
    client: HerdrClient, config: Config, agent: str, *, use_cache: bool
) -> Target:
    """Resolve a target while the account-global session lock is held."""
    tab_label = tab_label_for(config, agent)
    cwd = config.cwd or config.project_root
    if not os.path.isabs(cwd):
        cwd = os.path.abspath(os.path.join(config.project_root, cwd))
    key = f"{config.workspace}\x00{tab_label}"
    path = cache_path(config)

    created: list[str] = []
    if client.ensure_server():
        created.append("server")

    if use_cache and not created:
        cached = _load_cache(path, key)
        if cached is not None and _cache_still_valid(
            client, cached, config.workspace, tab_label
        ):
            return Target(
                workspace_id=cached[0],
                tab_id=cached[1],
                pane_id=cached[2],
                workspace_label=config.workspace,
                tab_label=tab_label,
                created=(),
                from_cache=True,
            )

    workspace_id = client.workspace_id_for_label(config.workspace)
    tab_id: str | None = None
    if workspace_id is None:
        # A new workspace arrives with one default tab; rename it into our schema instead of adding
        # a second one, so the common first-run case leaves exactly one tab behind.
        workspace_id, root_tab_id, _root_pane_id = client.create_workspace(
            label=config.workspace, cwd=cwd
        )
        client.rename_tab(root_tab_id, tab_label)
        tab_id = root_tab_id
        created.extend(["workspace", "tab"])
    else:
        tab_id = client.tab_id_for_label(workspace_id, tab_label)
        if tab_id is None:
            _enforce_pane_cap(client, config, workspace_id)
            tab_id = client.create_tab(
                workspace_id=workspace_id, label=tab_label, cwd=cwd
            )
            created.append("tab")

    pane_id = _pane_of_tab(client, workspace_id, tab_id)
    target = Target(
        workspace_id=workspace_id,
        tab_id=tab_id,
        pane_id=pane_id,
        workspace_label=config.workspace,
        tab_label=tab_label,
        created=tuple(created),
        from_cache=False,
    )
    try:
        _store_cache(path, key, target)
    except OSError:
        # The cache is an optimisation; failing to persist it must never fail the run.
        pass
    return target
