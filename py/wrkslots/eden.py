"""Eden checkout support for wrkslots (#1204 wrkslots-eden-checkouts).

An Eden checkout is a second checkout kind beside git worktrees. This
module holds the pure, fake-friendly logic: host-wide cap accounting,
redirection and usage parsing, Sapling salvage commands, and the
remove verification. Process execution is injected as a runner so
unit tests never touch a real daemon; ``cli.py`` supplies the real
runner. Nothing here creates or removes a checkout by itself.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path

#: Per-host cap on OUTER Eden checkouts (#1566). Checkouts inside the
#: worktrees image are bounded by the image's disk and are reported
#: separately, never counted here.
HOST_CAP = 3

#: Fixed markers stored in the git-shaped fields of an Eden row. The
#: live Eden facts (state dir, revision, targets) are always queried
#: from the daemon, never trusted from these markers.
EDEN_BRANCH = "eden"
EDEN_REMOTE = "eden"
EDEN_LANDED_REF = "refs/remotes/eden/main"

#: Runner: argv -> (return code, stdout, stderr).
Runner = Callable[[Sequence[str]], tuple[int, str, str]]


class EdenError(RuntimeError):
    """An Eden operation could not be completed safely."""


class UnqueryableEdenError(EdenError):
    """The daemon's answer cannot be trusted, so it must not be started blindly.

    Raised when ``eden info`` cannot be run, times out, returns output
    that does not parse, or answers for a different mount: in every such
    case the daemon's state is unknown rather than down.
    """


class EdenDaemonDownError(EdenError):
    """``eden info`` ran but the daemon does not answer, so it may be started."""


#: Per-call budgets for the restart verb (#1977): a hung FUSE query must
#: refuse its own slot, never stall the remaining slots.
INFO_TIMEOUT_SECONDS = 15.0
START_TIMEOUT_SECONDS = 60.0


@dataclass(frozen=True)
class Redirection:
    """One redirected path of an Eden checkout."""

    repo_path: str
    kind: str
    target: str


@dataclass(frozen=True)
class CheckoutInfo:
    """Facts ``eden info`` reports for one mount."""

    mount: str
    state_dir: str
    backing_repo: str
    revision: str


def config_dirs_from_cmdlines(cmdlines: Sequence[str]) -> tuple[str, ...]:
    """Eden state dirs named by running daemons' ``--edenDir`` flags."""

    found: list[str] = []
    for line in cmdlines:
        parts = line.split()
        for index, part in enumerate(parts):
            if part == "--edenDir" and index + 1 < len(parts):
                value = parts[index + 1]
                if value not in found:
                    found.append(value)
            elif part.startswith("--edenDir="):
                value = part.partition("=")[2]
                if value and value not in found:
                    found.append(value)
    return tuple(sorted(found))


def discover_config_dirs(
    cmdlines: Sequence[str], known: Sequence[str]
) -> tuple[str, ...]:
    """Every Eden state dir to count: daemon-reported plus known ones."""

    merged = list(config_dirs_from_cmdlines(cmdlines))
    for value in known:
        if value not in merged:
            merged.append(value)
    return tuple(sorted(merged))


def parse_mount_list(stdout: str) -> tuple[str, ...]:
    """Mount paths from ``eden list`` output, one per line."""

    return tuple(
        line.strip() for line in stdout.splitlines() if line.strip()
    )


def checkouts_by_state_dir(
    runner: Runner, config_dirs: Sequence[str]
) -> tuple[tuple[str, str], ...]:
    """Every Eden mount on the host with the state dir that lists it.

    Deduplicated by mount path; the first state dir that lists a mount
    owns it. Fail-closed: a state dir whose daemon cannot be queried
    raises, so an uncertain count never reads as room under the cap.
    """

    pairs: list[tuple[str, str]] = []
    seen: set[str] = set()
    for config_dir in config_dirs:
        rc, stdout, stderr = runner(
            ["eden", "--config-dir", config_dir, "list"]
        )
        if rc != 0:
            raise EdenError(
                f"cannot list Eden checkouts in {config_dir}: "
                f"{stderr.strip() or stdout.strip() or 'eden list failed'}"
            )
        for mount in parse_mount_list(stdout):
            if mount not in seen:
                seen.add(mount)
                pairs.append((mount, config_dir))
    return tuple(pairs)


def not_mounted_mounts(stdout: str) -> tuple[str, ...]:
    """Mounts ``eden list`` reports as ``(not mounted)`` (#1805).

    Eden keeps a checkout in the list after removing it, rendered as
    ``<mount> (not mounted)``; such a checkout has no working copy left
    to probe, so salvage prechecks against it can only fail.
    """

    suffix = " (not mounted)"
    found: list[str] = []
    for line in stdout.splitlines():
        stripped = line.strip()
        if stripped.endswith(suffix):
            found.append(stripped[: -len(suffix)])
    return tuple(found)


def count_checkouts(runner: Runner, config_dirs: Sequence[str]) -> tuple[str, ...]:
    """All Eden mounts on the host, deduplicated by mount path.

    Fail-closed like :func:`checkouts_by_state_dir`.
    """

    return tuple(mount for mount, _state in checkouts_by_state_dir(runner, config_dirs))


def partition_checkouts(
    mounts: Sequence[str],
    image_roots: Sequence[str],
    state_dirs: Mapping[str, str] | None = None,
) -> tuple[tuple[str, ...], tuple[str, ...]]:
    """Split mounts into (outer, in-image) (#1566).

    A mount counts in-image when its path, or the state dir of the
    daemon that lists it (``state_dirs[mount]``), lies inside an image
    root. The state dir is what decides where a checkout's storage
    lives: since #1730 ``create --eden`` mounts every Eden slot outside
    the project at ``eden_mount_root`` while the slot's daemon state,
    overlay and redirection targets stay in the slot, so a path-only
    test counts every new in-image checkout outer. A mount with no
    known state dir is judged by its path alone, as before.

    Symlinks are resolved on both sides, so an alias into the image
    counts in-image and an alias out counts outer; the same checkout
    reported under two alias spellings counts once. Bind mounts are
    invisible to path resolution, so callers must pass recorded bind
    targets as roots; image content bound to an unrecorded host path
    then still counts outer, which is the conservative direction.
    Empty ``image_roots`` counts every mount outer.
    """

    roots = tuple(os.path.realpath(root) for root in image_roots)
    states = state_dirs or {}
    outer: list[str] = []
    in_image: list[str] = []
    seen: set[str] = set()
    for mount in mounts:
        resolved = os.path.realpath(mount)
        if resolved in seen:
            continue
        seen.add(resolved)
        candidates = [resolved]
        state = states.get(mount)
        if state is not None:
            candidates.append(os.path.realpath(state))
        if any(is_inside(path, root) for path in candidates for root in roots):
            in_image.append(mount)
        else:
            outer.append(mount)
    return tuple(outer), tuple(in_image)


def assert_under_cap(
    mounts: Sequence[str],
    cap: int = HOST_CAP,
    in_image: Sequence[str] = (),
) -> None:
    """Refuse when the host already holds ``cap`` OUTER checkouts.

    ``mounts`` is the outer list; ``in_image`` mounts never count
    against the cap and are named in the refusal so flooding cannot
    hide behind them.
    """

    if len(mounts) >= cap:
        extra = (
            f"; in-image not counted: {len(in_image)}: " + ", ".join(in_image)
            if in_image
            else ""
        )
        raise EdenError(
            f"host Eden cap reached: {len(mounts)} outer checkout(s) "
            f"at cap {cap}: " + ", ".join(mounts) + extra
        )


def parse_info(stdout: str) -> CheckoutInfo:
    """Parse ``eden info <mount>`` JSON."""

    try:
        raw = json.loads(stdout)
    except json.JSONDecodeError as exc:
        raise EdenError(f"eden info returned invalid JSON: {exc}") from exc
    if not isinstance(raw, dict):
        raise EdenError("eden info returned a non-object JSON value")
    fields: dict[str, str] = {}
    for key in ("mount", "state_dir", "backing_repo", "checked_out_revision"):
        value = raw.get(key)
        if not isinstance(value, str) or not value:
            raise EdenError(f"eden info is missing {key}")
        fields[key] = value
    return CheckoutInfo(
        mount=fields["mount"],
        state_dir=fields["state_dir"],
        backing_repo=fields["backing_repo"],
        revision=fields["checked_out_revision"],
    )


def parse_redirections(stdout: str) -> tuple[Redirection, ...]:
    """Parse ``eden redirect list --json`` output."""

    try:
        raw = json.loads(stdout)
    except json.JSONDecodeError as exc:
        raise EdenError(f"eden redirect list returned invalid JSON: {exc}") from exc
    if not isinstance(raw, list):
        raise EdenError("eden redirect list returned a non-list JSON value")
    result: list[Redirection] = []
    for item in raw:
        if not isinstance(item, dict):
            raise EdenError("eden redirect list entry is not an object")
        repo_path = item.get("repo_path")
        kind = item.get("type")
        target = item.get("target")
        if not all(isinstance(v, str) and v for v in (repo_path, kind, target)):
            raise EdenError("eden redirect list entry lacks repo_path/type/target")
        result.append(
            Redirection(repo_path=str(repo_path), kind=str(kind), target=str(target))
        )
    return tuple(result)


def parse_orphaned_bytes(stdout: str) -> int:
    """Orphaned-redirection bytes from ``eden du --json`` output."""

    try:
        raw = json.loads(stdout)
    except json.JSONDecodeError as exc:
        raise EdenError(f"eden du returned invalid JSON: {exc}") from exc
    if not isinstance(raw, dict):
        raise EdenError("eden du returned a non-object JSON value")
    value = raw.get("orphaned_redirections")
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        raise EdenError("eden du lacks a valid orphaned_redirections count")
    return value


def salvage_needed(sl_status: str, current: str, recorded: str) -> bool:
    """Whether Sapling work must be preserved before removal."""

    return bool(sl_status.strip()) or current != recorded


def salvage_commands(
    mount: str, dest: Path
) -> tuple[tuple[tuple[str, ...], str | None], ...]:
    """Commands preserving uncommitted work as a patch plus a bundle.

    Each entry is ``(argv, output_file)``: when ``output_file`` is not
    None the caller writes the command's stdout to that file (``sl
    diff`` has no ``-o`` option); the other commands write their own
    files. The bundle holds the draft stack; the patch holds the
    working-copy diff. Both land in the salvage archive and are read
    back by the caller before removal proceeds, exactly like git
    bundles.
    """

    patch = str(dest / "working-copy.patch")
    bundle = str(dest / "draft-stack.bundle")
    return (
        (("sl", "-R", mount, "diff", "--git"), patch),
        (("sl", "-R", mount, "export", "--git", "-o", patch + ".stack", "."), None),
        (("sl", "-R", mount, "bundle", "--all", bundle), None),
    )


def is_inside(path: str, root: str) -> bool:
    """Whether ``path`` is ``root`` or lives beneath it."""

    return path == root or path.startswith(root.rstrip("/") + "/")


#: Sapling store features whose content is fetched on demand: a backing
#: carrying either is a lazy store, so later reads may go to its upstream.
LAZY_REQUIRES = frozenset({"lazychangelog", "remotefilelog"})


def backing_repo_problem(path: Path) -> str | None:
    """Why ``path`` is not a self-contained Sapling backing repo, or None.

    Filesystem-only (no sl/eden execution), so create can fail closed
    before writing anything and tests need no daemon.
    """

    if not path.exists():
        return f"eden backing repo not seeded: {path}"
    if path.is_symlink() or not path.is_dir():
        return f"eden backing repo is not a directory: {path}"
    if not (path / ".hg" / "requires").is_file() or not (path / ".hg" / "store").is_dir():
        return f"not a Sapling backing repo: {path} (no .hg/requires or .hg/store)"
    if (path / ".hg" / "sharedpath").exists():
        return f"eden backing repo is a shared clone, not a self-contained store: {path}"
    return None


def _hgrc_default_path(hgrc: Path) -> str | None:
    """The ``[paths] default`` value in an hgrc, tolerating %include lines."""

    try:
        lines = hgrc.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return None
    in_paths = False
    for raw in lines:
        line = raw.strip()
        if line.startswith("["):
            in_paths = line == "[paths]"
            continue
        if in_paths and "=" in line:
            key, _, value = line.partition("=")
            if key.strip() == "default":
                return value.strip() or None
    return None


def backing_facts(path: Path) -> dict[str, object]:
    """Observable backing facts for seed reports: laziness and upstream.

    A lazy store (either lazy feature in its requires files) still
    fetches commit/file content from its default path on demand,
    exactly like the host backing it was cloned from; reports must
    say so instead of implying the copy is complete.
    """

    requires: list[str] = []
    for name in (path / ".hg" / "requires", path / ".hg" / "store" / "requires"):
        try:
            requires.extend(
                line.strip()
                for line in name.read_text(
                    encoding="utf-8", errors="replace"
                ).splitlines()
                if line.strip()
            )
        except OSError:
            continue
    return {
        "requires": tuple(requires),
        "lazy": bool(LAZY_REQUIRES.intersection(requires)),
        "default_path": _hgrc_default_path(path / ".hg" / "hgrc"),
    }


def measure_tree(path: Path) -> tuple[int, int]:
    """(total bytes, file count) under ``path`` without following symlinks."""

    total = 0
    files = 0
    for directory, _subdirs, names in os.walk(path):
        for name in names:
            try:
                total += os.lstat(os.path.join(directory, name)).st_size
            except OSError:
                continue
            files += 1
    return total, files


def _set_hgrc_value(hgrc: Path, section: str, key: str, value: str) -> None:
    """Set ``key`` under ``[section]`` in an hgrc, preserving other lines."""

    try:
        lines = hgrc.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        lines = []
    out: list[str] = []
    in_section = False
    done = False
    header = f"[{section}]"
    for raw in lines:
        line = raw.strip()
        if line.startswith("["):
            if in_section and not done:
                out.append(f"{key} = {value}")
                done = True
            in_section = line == header
            out.append(raw)
            continue
        if in_section and "=" in line and line.partition("=")[0].strip() == key:
            if not done:
                out.append(f"{key} = {value}")
                done = True
            continue
        out.append(raw)
    if not done:
        if not in_section:
            out.append(header)
        out.append(f"{key} = {value}")
    hgrc.write_text("\n".join(out) + "\n", encoding="utf-8")


def _set_hgrc_default_path(hgrc: Path, value: str) -> None:
    """Set ``[paths] default`` in an hgrc, preserving every other line."""

    _set_hgrc_value(hgrc, "paths", "default", value)


def _is_remote_path(value: str) -> bool:
    """Whether a ``[paths] default`` names a remote, not a local path."""

    if value.startswith(("/", ".", "~")):
        return False
    return not Path(value).exists()


def seed_backing_repo(
    runner: Runner, source: Path, staging: Path, cachepath: Path
) -> None:
    """Lazily clone ``source``'s upstream into ``staging`` (#1804).

    A lazy no-update clone of the source's ``[paths] default`` (never a
    full local clone of the source), so the seeded backing is megabytes,
    not the whole store; its ``[remotefilelog] cachepath`` points at the
    in-image ``cachepath``, written only into the new clone's own hgrc.
    Refuses when the source records no default, or a local one: there
    is then nothing remote to be lazy against. Proves the clone stands
    alone (no shared store, its default is the upstream, never the
    source checkout, and a standalone log read). The caller owns the
    staging directory, the atomic rename into place, and reporting.
    """

    problem = backing_repo_problem(source)
    if problem is not None:
        raise EdenError(problem)
    source_default_obj = backing_facts(source)["default_path"]
    source_default = (
        source_default_obj if isinstance(source_default_obj, str) else None
    )
    if source_default is None:
        raise EdenError(
            f"backing source {source} records no [paths] default; a lazy "
            "seed needs a remote upstream to clone from (staging kept "
            "for inspection)"
        )
    if not _is_remote_path(source_default):
        raise EdenError(
            f"backing source {source} default path {source_default!r} is "
            "a local path, not a remote; a lazy seed clones the upstream, "
            "so name a remote there first (staging kept for inspection)"
        )
    rc, stdout, stderr = runner(
        ["sl", "clone", "-U", source_default, str(staging)]
    )
    if rc != 0:
        raise EdenError(
            f"sl clone of the backing repo failed: "
            f"{stderr.strip() or stdout.strip() or 'sl clone failed'}"
        )
    problem = backing_repo_problem(staging)
    if problem is not None:
        raise EdenError(problem)
    if backing_facts(staging)["default_path"] == str(source):
        _set_hgrc_default_path(staging / ".hg" / "hgrc", source_default)
    _set_hgrc_value(
        staging / ".hg" / "hgrc", "remotefilelog", "cachepath", str(cachepath)
    )
    rc, stdout, stderr = runner(
        ["sl", "-R", str(staging), "log", "-l", "1", "-T", "{node}"]
    )
    if rc != 0 or not stdout.strip():
        raise EdenError(
            f"seeded backing at {staging} does not read standalone: "
            f"{stderr.strip() or stdout.strip() or 'sl log failed'}"
        )


def _run_with_timeout(
    runner: Runner, argv: Sequence[str], timeout: float
) -> tuple[int, str, str]:
    """Run one Eden call under ``timeout`` seconds.

    The real CLI runner accepts a ``timeout`` keyword; simple fake
    runners that take only ``argv`` are called without it. A timeout
    raised by either form propagates as ``subprocess.TimeoutExpired``
    for the caller to turn into that slot's refusal.
    """

    try:
        return runner(argv, timeout=timeout)  # type: ignore[call-arg]
    except TypeError:
        return runner(argv)


def eden_state_containment_problem(
    config_dir: str, slot_dir: str, slot_root: str | None
) -> str | None:
    """Why starting a daemon from ``config_dir`` would leave its slot, or None.

    ``<slot>/.eden-state`` is inspected with ``lstat`` and its realpath
    must stay inside our slot directory, so a state dir symlinked into
    another tree refuses instead of starting a daemon against it. For
    absolute mounts the caller also passes the project's slot root and
    the derived slot directory's realpath must stay inside it, so a
    symlinked slot cannot redirect the restart outside the project. A
    missing state dir is not a problem here; the caller skips it.
    """

    try:
        os.lstat(config_dir)
    except FileNotFoundError:
        return None
    except OSError as exc:
        return f"cannot inspect .eden-state at {config_dir}: {exc}"
    resolved_state = os.path.realpath(config_dir)
    resolved_slot = os.path.realpath(slot_dir)
    if not is_inside(resolved_state, resolved_slot):
        return (
            f".eden-state at {config_dir} resolves to {resolved_state}, "
            f"outside its slot directory {slot_dir}; refusing to start a "
            "daemon against another tree"
        )
    if slot_root is not None:
        resolved_root = os.path.realpath(slot_root)
        if not is_inside(resolved_slot, resolved_root):
            return (
                f"slot directory {slot_dir} resolves to {resolved_slot}, "
                f"outside the project slot root {slot_root}; refusing to "
                "start a daemon outside this project's slots"
            )
    return None


def restart_slot_daemon(
    runner: Runner, config_dir: str, mount: str, *, dry_run: bool = False
) -> str:
    """Restart one slot's Eden daemon when it is dead (#1977).

    Returns ``already-healthy`` when ``eden info`` already answers for
    ``mount``, ``restarted`` when a start made it answer, or, with
    ``dry_run``, ``would-restart`` for a dead daemon (no start is run).
    Starting uses ``eden --config-dir <dir> start --if-not-running``, the
    per-config-dir start the host Eden CLI provides (``eden start
    --help``); provisioning started this daemon implicitly through
    ``eden clone`` under the same ``--config-dir``, and removal stops
    it with ``eden stop`` under it. No checkout is created, so this
    never consumes host-cap room beyond the checkout that already
    exists and counts. An answer that cannot be parsed, names another
    mount, cannot be run, or times out is unqueryable
    (:class:`UnqueryableEdenError`) and refuses rather than starting
    blindly; only a daemon proven down (:class:`EdenDaemonDownError`)
    is started. Every call is bounded (``info`` by
    ``INFO_TIMEOUT_SECONDS``, ``start`` by ``START_TIMEOUT_SECONDS``)
    and a timeout refuses this slot alone. After a failed start the
    daemon is queried once more before refusing, because another
    starter may have won the race in the meantime; a daemon that still
    does not answer after a start refuses too.
    """

    def query() -> CheckoutInfo:
        try:
            rc, stdout, stderr = _run_with_timeout(
                runner,
                ["eden", "--config-dir", config_dir, "info", mount],
                INFO_TIMEOUT_SECONDS,
            )
        except OSError as exc:
            raise UnqueryableEdenError(
                f"cannot query Eden daemon for {mount}: {exc}"
            ) from exc
        except subprocess.TimeoutExpired as exc:
            raise UnqueryableEdenError(
                f"cannot query Eden daemon for {mount}: eden info timed "
                f"out after {INFO_TIMEOUT_SECONDS:g}s"
            ) from exc
        if rc != 0:
            raise EdenDaemonDownError(
                f"Eden daemon for {mount} does not answer: "
                f"{stderr.strip() or stdout.strip() or 'eden info failed'}"
            )
        try:
            info = parse_info(stdout)
        except EdenError as exc:
            raise UnqueryableEdenError(
                f"cannot query Eden daemon for {mount}: {exc}"
            ) from exc
        if info.mount != mount:
            raise UnqueryableEdenError(
                f"eden info for {mount} reports mount {info.mount!r}; "
                "refusing to treat another checkout's daemon as this slot's"
            )
        return info

    try:
        query()
    except UnqueryableEdenError:
        # Unknown is not dead: never start blindly on an answer that
        # could not be trusted.
        raise
    except EdenDaemonDownError as initial:
        if dry_run:
            return "would-restart"
        start_error: EdenError | None = None
        try:
            rc, stdout, stderr = _run_with_timeout(
                runner,
                ["eden", "--config-dir", config_dir, "start", "--if-not-running"],
                START_TIMEOUT_SECONDS,
            )
        except OSError as exc:
            start_error = EdenError(
                f"cannot start Eden daemon for {mount}: {exc}"
            )
        except subprocess.TimeoutExpired:
            start_error = EdenError(
                f"eden start timed out for {mount} after "
                f"{START_TIMEOUT_SECONDS:g}s; daemon before start: {initial}"
            )
        else:
            if rc == 0:
                try:
                    query()
                except EdenError as after:
                    raise EdenError(
                        f"Eden daemon for {mount} did not answer after "
                        f"start: {after}"
                    ) from after
                return "restarted"
            start_error = EdenError(
                f"eden start failed for {mount}: "
                f"{stderr.strip() or stdout.strip() or 'eden start failed'}; "
                f"daemon before start: {initial}"
            )
        # The start failed, but another starter may have won meanwhile:
        # query once more before refusing this slot.
        try:
            query()
        except EdenError as after:
            raise EdenError(
                f"{start_error}; re-query after the failed start: {after}"
            ) from after
        return "restarted"
    return "already-healthy"


def provision_checkout(
    runner: Runner,
    config_dir: str,
    backing: str,
    mount: str,
    rev: str | None,
) -> tuple[CheckoutInfo, tuple[Redirection, ...]]:
    """Clone one checkout and prove its redirections stay in bounds.

    The caller owns the mount's parent directory and cleanup on
    failure; this only runs the daemon commands and verifies.
    """

    argv = ["eden", "--config-dir", config_dir, "clone", backing, mount]
    if rev is not None:
        argv += ["--rev", rev]
    rc, stdout, stderr = runner(argv)
    if rc != 0:
        raise EdenError(f"eden clone failed: {stderr.strip() or stdout.strip()}")
    rc, stdout, stderr = runner(
        ["eden", "--config-dir", config_dir, "info", mount]
    )
    if rc != 0:
        raise EdenError(f"eden info failed after clone: {stderr.strip()}")
    info = parse_info(stdout)
    rc, stdout, stderr = runner(
        ["eden", "--config-dir", config_dir, "redirect", "list",
         "--mount", mount, "--json"]
    )
    if rc != 0:
        raise EdenError(f"eden redirect list failed: {stderr.strip()}")
    return info, parse_redirections(stdout)


def remove_sequence(
    runner: Runner,
    config_dir: str,
    mount: str,
    targets: Sequence[str],
    existing_paths: Sequence[str],
    orphaned_before: int,
) -> tuple[str, ...]:
    """Run Eden removal and return orphan problems (empty is clean).

    Stops the per-slot daemon after the mount is gone; the caller
    owns subvolume deletion and must refuse while problems remain.
    """

    rc, _out, stderr = runner(
        ["eden", "--config-dir", config_dir, "remove", "-y", mount]
    )
    if rc != 0:
        return (f"eden remove failed: {stderr.strip()}",)
    runner(["eden", "--config-dir", config_dir, "stop"])
    # Stop and disable this slot's OWN daemon unit and prove it gone
    # (#1702): `eden stop` alone leaves the systemd unit able to
    # restart the daemon, which the removal census then refuses.
    # The shared default daemon (no --config-dir) is never named.
    unit = unit_name(config_dir)
    for verb in ("stop", "disable"):
        rc, _out, stderr = runner(["systemctl", "--user", verb, unit])
        if rc != 0 and "not loaded" not in stderr:
            return (
                f"cannot {verb} the slot's daemon unit {unit}: "
                f"{stderr.strip()}; run 'systemctl --user {verb} {unit}'",
            )
    rc, stdout, _err = runner(["systemctl", "--user", "is-active", unit])
    if stdout.strip() == "active":
        return (
            f"daemon unit {unit} is still active after stop; "
            f"run 'systemctl --user stop {unit}'",
        )
    rc, stdout, stderr = runner(["eden", "--config-dir", config_dir, "list"])
    if rc != 0:
        return (f"eden list failed after remove: {stderr.strip()}",)
    mounts_after = parse_mount_list(stdout)
    rc, stdout, stderr = runner(["eden", "du", "--json", mount])
    orphaned_after = orphaned_before
    if rc == 0:
        try:
            orphaned_after = parse_orphaned_bytes(stdout)
        except EdenError:
            orphaned_after = orphaned_before
    return verify_removed(
        mounts_after, mount, targets, existing_paths,
        orphaned_before, orphaned_after,
    )


def verify_removed(
    mounts_after: Sequence[str],
    mount: str,
    targets: Sequence[str],
    existing_paths: Sequence[str],
    orphaned_before: int,
    orphaned_after: int,
) -> tuple[str, ...]:
    """Orphan checks after ``eden remove``; empty means fully removed.

    ``eden remove`` leaves buck-out targets behind, so removal is
    only proven when the mount is unlisted, every recorded target
    path is gone, and orphaned-redirection bytes did not grow.
    """

    problems: list[str] = []
    if mount in mounts_after:
        problems.append(f"mount still listed by eden: {mount}")
    for target in targets:
        if target in existing_paths:
            problems.append(f"redirection target still present: {target}")
    if orphaned_after > orphaned_before:
        problems.append(
            f"orphaned redirections grew: {orphaned_before} -> {orphaned_after} bytes"
        )
    return tuple(problems)


def unit_name(config_dir: str) -> str:
    """The per-slot daemon unit: ``edenfs@<escaped state dir>.service`` (#1423)."""

    escaped = config_dir.lstrip("/").replace("-", "\\x2d").replace("/", "-")
    return f"edenfs@{escaped}.service"


#: Per-checkout Sapling draft ledger (#1925). The ledger lives in a
#: sidecar file under the slot's ``.eden-state`` (see cli.py), never in
#: a slot record, so an older wrkslots reader never parses it.
DRAFT_LEDGER_SCHEMA = 1
DRAFT_LEDGER_KIND = "wrkslots-eden-draft-ledger"

_NODE_RE = re.compile(r"^[0-9a-f]{40}$")


@dataclass(frozen=True)
class DraftLedger:
    """Draft nodes one Eden checkout has owned, per wrkslots' own record."""

    slot: str
    checkout: str
    generation: int | None
    nodes: tuple[str, ...]
    captured_at: str | None
    last_error: str | None
    #: Nodes an operator explicitly declared not this slot's, as
    #: (node, declared-by, declared-at); the audit record of a
    #: first-capture judgement (#1925 review, fix round 2).
    disowned: tuple[tuple[str, str, str], ...] = ()


def draft_ledger_payload(ledger: DraftLedger) -> dict[str, object]:
    """The canonical JSON object for one draft ledger sidecar."""

    return {
        "schema": DRAFT_LEDGER_SCHEMA,
        "kind": DRAFT_LEDGER_KIND,
        "slot": ledger.slot,
        "checkout": ledger.checkout,
        "generation": ledger.generation,
        "nodes": list(ledger.nodes),
        "captured_at": ledger.captured_at,
        "last_error": ledger.last_error,
        "disowned": [
            {"node": node, "by": by, "at": at}
            for node, by, at in ledger.disowned
        ],
    }


def draft_ledger_from_obj(raw: object, label: str) -> DraftLedger:
    """Validate one parsed draft ledger object; refuse any other shape."""

    if not isinstance(raw, dict):
        raise EdenError(f"{label} is not a JSON object")
    expected = {"schema", "kind", "slot", "checkout", "generation",
                "nodes", "captured_at", "last_error"}
    # Ledgers written before the disowned record existed are read
    # with an empty one; every other key stays exact.
    if set(raw) not in (expected, expected | {"disowned"}):
        raise EdenError(f"{label} has unexpected or missing keys")
    if raw["schema"] != DRAFT_LEDGER_SCHEMA or raw["kind"] != DRAFT_LEDGER_KIND:
        raise EdenError(f"{label} is not a schema {DRAFT_LEDGER_SCHEMA} draft ledger")
    slot, checkout = raw["slot"], raw["checkout"]
    if not isinstance(slot, str) or not slot or not isinstance(checkout, str) or not checkout:
        raise EdenError(f"{label} lacks a slot or checkout name")
    generation = raw["generation"]
    if generation is not None and (
        not isinstance(generation, int) or isinstance(generation, bool) or generation < 1
    ):
        raise EdenError(f"{label} has an invalid generation")
    nodes_raw = raw["nodes"]
    if not isinstance(nodes_raw, list) or any(
        not isinstance(node, str) or not _NODE_RE.fullmatch(node) for node in nodes_raw
    ):
        raise EdenError(f"{label} holds a node that is not a full Sapling commit id")
    if list(nodes_raw) != sorted(set(nodes_raw)):
        raise EdenError(f"{label} nodes are not sorted and deduplicated")
    captured_at, last_error = raw["captured_at"], raw["last_error"]
    if captured_at is not None and not isinstance(captured_at, str):
        raise EdenError(f"{label} has an invalid captured_at")
    if last_error is not None and not isinstance(last_error, str):
        raise EdenError(f"{label} has an invalid last_error")
    disowned: list[tuple[str, str, str]] = []
    disowned_raw = raw.get("disowned", [])
    if not isinstance(disowned_raw, list):
        raise EdenError(f"{label} has an invalid disowned record")
    for item in disowned_raw:
        if (
            not isinstance(item, dict) or set(item) != {"node", "by", "at"}
            or not isinstance(item["node"], str)
            or not _NODE_RE.fullmatch(item["node"])
            or not isinstance(item["by"], str) or not item["by"]
            or not isinstance(item["at"], str) or not item["at"]
        ):
            raise EdenError(f"{label} holds an invalid disowned entry")
        disowned.append((item["node"], item["by"], item["at"]))
    if disowned != sorted(disowned):
        raise EdenError(f"{label} disowned entries are not sorted")
    if {node for node, _by, _at in disowned} & set(nodes_raw):
        raise EdenError(f"{label} both owns and disowns a node")
    return DraftLedger(
        slot=slot, checkout=checkout, generation=generation,
        nodes=tuple(nodes_raw), captured_at=captured_at, last_error=last_error,
        disowned=tuple(disowned),
    )


def _run_nodes(runner: Runner, mount: str, revset: str) -> tuple[str, ...]:
    rc, stdout, stderr = runner(
        ["sl", "-R", mount, "log", "-r", revset, "-T", "{node}\n"]
    )
    if rc != 0:
        raise EdenError(
            f"Sapling accounting failed (sl -R {mount} log -r {revset!r}): "
            f"{stderr.strip() or stdout.strip()}"
        )
    nodes: list[str] = []
    for line in stdout.splitlines():
        node = line.strip()
        if node and node not in nodes:
            nodes.append(node)
    return tuple(nodes)


def draft_stack_nodes(runner: Runner, mount: str) -> tuple[str, ...]:
    """Draft ancestors of this mount's working-copy parent, only.

    This is the per-checkout attribution primitive (#1925): the
    working-copy parent is per mount even when the backing store is
    shared, so another checkout's drafts enter only when this
    checkout stacked or moved onto them.
    """

    return _run_nodes(runner, mount, "draft() & ::.")


def all_draft_nodes(runner: Runner, mount: str) -> tuple[str, ...]:
    """Every draft in the (possibly shared) backing store; never scoped."""

    return _run_nodes(runner, mount, "draft()")


def successor_nodes(
    runner: Runner, mount: str, nodes: Sequence[str]
) -> tuple[str, ...]:
    """Current successors of ``nodes`` (rewrites such as amend), inclusive.

    Children are deliberately not included: another checkout may
    stack a child on a recorded draft, and that child is not this
    checkout's work merely for descending from it.
    """

    if not nodes:
        return ()
    return _run_nodes(runner, mount, "successors(" + " + ".join(nodes) + ")")


def present_successor_nodes(runner: Runner, mount: str, node: str) -> tuple[str, ...]:
    """Successors of one node, or empty when it is hidden or unknown.

    ``present()`` keeps an unknown node from erroring (or pulling)
    the query, so the caller can tell hidden, unknown and rewritten
    nodes apart instead of silently dropping any of them (#1925
    review D2/D5).
    """

    return _run_nodes(runner, mount, f"successors(present({node}))")


def hidden_node_subject(runner: Runner, mount: str, node: str) -> str | None:
    """The subject of a known-but-hidden node, or None when unknown.

    ``present()`` keeps the query from autopulling an unknown node
    from the store's default path (#1925 review, fix round 2).
    """

    rc, stdout, stderr = runner(
        ["sl", "-R", mount, "log", "--hidden", "-r", f"present({node})",
         "-T", "{node}\n{desc|firstline}"]
    )
    if rc != 0:
        # A failed query is not an unknown node: refuse rather
        # than drop a hidden draft on a transient Sapling error.
        raise EdenError(
            f"Sapling hidden query failed (sl -R {mount} log --hidden "
            f"-r present({node})): {stderr.strip() or stdout.strip()}"
        )
    lines = stdout.splitlines()
    if not lines or lines[0].strip() != node:
        return None
    return lines[1] if len(lines) > 1 else ""


def latest_hidden_successor(runner: Runner, mount: str, node: str) -> tuple[str, str] | None:
    """The newest hidden successor of ``node`` as (node, subject), or None.

    When an amended successor is itself hidden, finish must name
    that successor (the current content), not the stale recorded
    predecessor (#1925 review, fix round 2).
    """

    rc, stdout, _stderr = runner(
        ["sl", "-R", mount, "log", "--hidden", "-r",
         f"successors(present({node}))", "-T", "{node}|{rev}|{desc|firstline}\n"]
    )
    if rc != 0:
        return None
    best: tuple[int, str, str] | None = None
    for line in stdout.splitlines():
        parts = line.split("|", 2)
        if len(parts) != 3 or not _NODE_RE.fullmatch(parts[0]):
            continue
        try:
            rev = int(parts[1])
        except ValueError:
            continue
        if best is None or rev > best[0]:
            best = (rev, parts[0], parts[2])
    if best is None or best[1] == node:
        return None
    return (best[1], best[2])


_JOURNAL_LINE_RE = re.compile(r"^([0-9a-f]{12,40}) -> ([0-9a-f]{12,40})\b")


def journal_visited_nodes(runner: Runner, mount: str) -> tuple[str, ...]:
    """Full nodes this working copy has sat on, from ``sl journal``.

    The journal is per working copy: an Eden mount's Sapling repo
    dir is its own (its ``shared`` file names only bookmarks), and
    the same holds for a sharedpath working copy, so the previous
    locations of ``.`` here are this checkout's alone. Short
    journal ids are expanded through ``present()`` (hidden
    included), never pulled.
    """

    rc, stdout, stderr = runner(["sl", "-R", mount, "journal"])
    if rc != 0:
        raise EdenError(
            f"Sapling journal failed (sl -R {mount} journal): "
            f"{stderr.strip() or stdout.strip()}"
        )
    short: list[str] = []
    for line in stdout.splitlines():
        match = _JOURNAL_LINE_RE.match(line.strip())
        if match:
            for token in match.groups():
                if set(token) != {"0"} and token not in short:
                    short.append(token)
    full: list[str] = []
    for token in short:
        rc, stdout, _stderr = runner(
            ["sl", "-R", mount, "log", "--hidden", "-r", f"present({token})",
             "-T", "{node}\n"]
        )
        if rc != 0:
            continue
        for line in stdout.splitlines():
            node = line.strip()
            if _NODE_RE.fullmatch(node) and node not in full:
                full.append(node)
    return tuple(full)


def node_is_public(runner: Runner, mount: str, node: str) -> bool:
    """Whether ``node`` is in the store's public phase."""

    return bool(_run_nodes(runner, mount, f"public() & present({node})"))


def draft_candidate_info(
    runner: Runner, mount: str
) -> tuple[tuple[str, str, int, str], ...]:
    """Store drafts outside this mount's stack: (node, author, unix date, subject).

    These are the candidates a first capture must judge (#1925
    review D1): every draft in the shared store that is not an
    ancestor of this working copy's parent.
    """

    rc, stdout, stderr = runner(
        ["sl", "-R", mount, "log", "-r", "draft() - ::.", "-T",
         "{node}|{author}|{date|hgdate}|{desc|firstline}\n"]
    )
    if rc != 0:
        raise EdenError(
            f"Sapling accounting failed (sl -R {mount} log -r 'draft() - ::.'): "
            f"{stderr.strip() or stdout.strip()}"
        )
    found: list[tuple[str, str, int, str]] = []
    for line in stdout.splitlines():
        parts = line.split("|", 3)
        if len(parts) != 4 or not _NODE_RE.fullmatch(parts[0]):
            continue
        try:
            when = int(parts[2].split()[0])
        except (ValueError, IndexError):
            when = 0
        found.append((parts[0], parts[1], when, parts[3]))
    return tuple(found)


_DIFFREV_RE = re.compile(r"Differential Revision:\s*(?:\S*/)?D(\d+)")


def _diffrev(message: str) -> str | None:
    match = _DIFFREV_RE.search(message)
    return match.group(1) if match else None


def classify_drafts(
    runner: Runner, mount: str, checkout_name: str, head: str,
    own_nodes: Sequence[str] | None = None,
    own_author: str | None = None,
) -> dict[str, object]:
    """Classify one Eden checkout's Sapling drafts (#1702, #1925).

    With ``own_nodes`` None this is the unscoped store-wide reading:
    the checkout's non-public commits (`draft()`), which on a shared
    backing store includes every other checkout's drafts. With
    ``own_nodes`` given (the per-checkout ledger), only those nodes
    and their current successors are classified.  A draft is landed
    when its Differential Revision already appears in public
    history, or when the node itself is public (landed without a
    revision), open when it carries a bookmark (published for
    review), and unaccounted otherwise. A ledger node whose
    successors are gone is reported as hidden (and stays
    unaccounted) when the store still knows it hidden, and refuses
    as unknown otherwise. With ``own_author`` given, unaccounted
    nodes by another author are also listed as foreign ancestors.
    The caller's --validation text plays no part: only the `sl`
    evidence decides.
    """

    def run(argv: list[str]) -> str:
        rc, stdout, stderr = runner(argv)
        if rc != 0:
            raise EdenError(
                f"Sapling accounting failed ({' '.join(argv)}): "
                f"{stderr.strip() or stdout.strip()}"
            )
        return stdout

    hidden: list[tuple[str, str]] = []
    if own_nodes is None:
        nodes = [
            line.strip()
            for line in run(
                ["sl", "-R", mount, "log", "-r", "draft()", "-T", "{node}\n"]
            ).splitlines()
            if line.strip()
        ]
    else:
        nodes = []
        for ledger_node in own_nodes:
            successors = present_successor_nodes(runner, mount, ledger_node)
            if successors:
                for node in successors:
                    if node not in nodes:
                        nodes.append(node)
                continue
            subject = hidden_node_subject(runner, mount, ledger_node)
            if subject is None:
                raise EdenError(
                    f"Sapling ledger node {ledger_node} is not known to the "
                    f"store at {mount}; it may have been pruned. Repair the "
                    "ledger with 'wrkslots capture-drafts <slot> --rebuild' "
                    "(which lists the store's candidates for explicit "
                    "inclusion) before finishing"
                )
            latest = latest_hidden_successor(runner, mount, ledger_node)
            hidden.append(latest if latest is not None else (ledger_node, subject))
    public_text = run(
        ["sl", "-R", mount, "log", "-r", "public()", "-l", "1000",
         "-T", "{desc}\n"]
    )
    landed_revs = {
        match.group(1) for match in _DIFFREV_RE.finditer(public_text)
    }
    open_nodes = {
        line.strip()
        for line in run(
            ["sl", "-R", mount, "log", "-r", "bookmark()", "-T", "{node}\n"]
        ).splitlines()
        if line.strip()
    }
    landed: list[tuple[str, str]] = []
    opened: list[tuple[str, str]] = []
    unaccounted: list[tuple[str, str]] = list(hidden)
    foreign: list[tuple[str, str]] = []
    for node in nodes:
        message = run(["sl", "-R", mount, "log", "-r", node, "-T", "{desc}"])
        subject = message.splitlines()[0] if message.splitlines() else ""
        rev = _diffrev(message)
        if rev is not None and rev in landed_revs:
            landed.append((node, subject))
        elif own_nodes is not None and node_is_public(runner, mount, node):
            landed.append((node, subject))
        elif node in open_nodes:
            opened.append((node, subject))
        else:
            unaccounted.append((node, subject))
            if own_author is not None:
                author = run(
                    ["sl", "-R", mount, "log", "-r", node, "-T", "{author}"]
                ).strip()
                if author and author != own_author:
                    foreign.append((node, subject))
    return {
        "checkout": checkout_name,
        "head": head,
        "landed": landed,
        "open": opened,
        "unaccounted": unaccounted,
        "hidden": hidden,
        "foreign": foreign,
    }
