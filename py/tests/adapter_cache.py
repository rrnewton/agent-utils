"""A content-keyed host cache for the Rust adapter executables the Python differentials run (#2340).

Three helpers release-build an adapter executable into a private target under
``rs/target/validation-adapters/``: ``tests.rust_binary`` (agentctl and the fake chat plugin),
``wrkslots.tests.rust_binary`` (wrkslotsd) and ``tests.wrkviz_rust_binary`` (wrkvizd). Each
gate runs in its own worktree, so those targets are usually cold, and Cargo keys a workspace
crate's artifacts by its source path, so even a host-shared target would rebuild the crate
itself, which is most of the cost. Many gates build an executable whose inputs are identical to
one already built on this host; this cache returns that one instead.

The key is a SHA-256 over everything that decides the executable:

- the cache name, the packages and binaries, and the Cargo arguments;
- ``rustc -vV`` and ``cargo -V``;
- the Git blob ids of ``rs/Cargo.lock`` and ``rs/Cargo.toml``;
- the Git tree id of every workspace crate in the packages' dependency closure (from
  ``cargo metadata``);
- the blob ids of the extra inputs: files in the repository outside those crates that the build
  reads (wrkviz embeds files from ``py/wrkviz/`` and ``common/docs/wrkviz/``). They are learned
  from Cargo's dep-info files at each publish and kept per cache name in ``index.json``;
- the values of the environment variables the build's scripts declare with
  ``rerun-if-env-changed`` (learned the same way), and of the ``CARGO_*`` and ``RUST*``
  variables that can change the output;
- the worktree's own path, only for a build whose executable holds that path (learned the same
  way, per cache name). wrkvizd does: it finds a built gh-paced through
  ``env!("CARGO_MANIFEST_DIR")``, so an executable built in another worktree would look there.

If any of those paths has uncommitted changes, including untracked files, there is no key: the
helper builds exactly as before and nothing is published.

An entry is ``<root>/<name>/<key>/`` holding the executables (mode 0555) and ``manifest.json``,
which is written before the directory is renamed into place, so a reader never sees a partial
entry. The manifest records where under the helper's target directory each executable was
built. A hit copies each verified executable back to that place (unless the file there already
hashes the same), hashes the copy, and returns it; the entry itself is never run, and only its
mtime, which is its age, is refreshed. An adapter may find its own files from its executable's
path (wrkslotsd looks for ``py/wrkslots/__main__.py`` above it), so a hit must leave the
executable exactly where a Cargo build in this worktree would.

Pruning deletes entries only while no gate ticket is queued, running or deferred on this host
(gate.py's admission directory is empty). While any ticket exists and the cache is at its size
cap, or the disk is short of its free-space floor, nothing new is published: the cache never
grows past its cap and never loses an entry under a running ticket.

Environment: ``AGENT_UTILS_ADAPTER_CACHE`` (``off`` disables the cache; any other non-empty
value is the cache root, default ``$XDG_CACHE_HOME/agent-utils/validation-adapters``),
``AGENT_UTILS_ADAPTER_CACHE_MAX_BYTES`` (default 1.5 GiB),
``AGENT_UTILS_ADAPTER_CACHE_MAX_ENTRIES`` (per name, default 12),
``AGENT_UTILS_ADAPTER_CACHE_MAX_AGE_DAYS`` (default 7) and
``AGENT_UTILS_ADAPTER_CACHE_MIN_FREE_BYTES`` (default 20 GiB).

Run as a script, ``prune`` prunes now (refusing while a ticket exists) and ``stats`` prints
each entry.
"""

from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import time
import uuid
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
#: 2: entries record each executable's place in the target and a hit runs a copy there.
VERSION = 2
MANIFEST = "manifest.json"
INDEX = "index.json"

#: ``CARGO_*`` variables that never change what Cargo builds.
_OUTPUT_NEUTRAL = frozenset({
    "CARGO_BUILD_JOBS", "CARGO_TARGET_DIR", "CARGO_HOME", "CARGO_MAKEFLAGS",
    "CARGO_HTTP_TIMEOUT", "CARGO_NET_RETRY", "CARGO_NET_OFFLINE", "RUSTUP_HOME",
})
_OUTPUT_NEUTRAL_PREFIXES = ("CARGO_TERM_", "CARGO_HTTP_", "CARGO_NET_", "CARGO_REGISTRIES_")


def _default_root() -> Path | None:
    setting = os.environ.get("AGENT_UTILS_ADAPTER_CACHE", "")
    if setting == "off":
        return None
    if setting:
        return Path(setting)
    base = os.environ.get("XDG_CACHE_HOME") or str(Path.home() / ".cache")
    return Path(base) / "agent-utils" / "validation-adapters"


#: Resolved once, when the helpers import this module: a test that later points HOME at a
#: temporary directory must not get a cache of its own there.
ROOT = _default_root()


def _limit(name: str, default: float) -> float:
    try:
        return float(os.environ.get(name, "") or default)
    except ValueError:
        return default


def _note(message: str) -> None:
    print(f"adapter-cache: {message}", file=sys.stderr, flush=True)


@dataclass(frozen=True)
class Spec:
    """One cached build: the Cargo packages it builds and the executables it must produce."""

    name: str
    packages: tuple[str, ...]
    binaries: tuple[str, ...]
    cargo_args: tuple[str, ...]
    env: Mapping[str, str]


@dataclass(frozen=True)
class Inputs:
    """What a key is computed from; ``None`` from :func:`inputs` means there is no key."""

    key: str
    extras: tuple[str, ...]


def _git(repo: Path, *args: str) -> str:
    return subprocess.run(
        ["git", "-C", str(repo), *args], check=True, capture_output=True, text=True, timeout=120,
    ).stdout


def cargo_metadata(repo: Path) -> dict[str, object]:
    """The workspace's resolved package graph, offline and against the committed lock."""
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--offline", "--locked",
         "--manifest-path", str(repo / "rs" / "Cargo.toml")],
        cwd=repo, check=True, capture_output=True, text=True, timeout=300,
    ).stdout
    data: object = json.loads(out)
    if not isinstance(data, dict):
        raise ValueError("cargo metadata did not print an object")
    return data


def toolchain(repo: Path) -> str:
    """``rustc -vV`` and ``cargo -V`` as the workspace sees them (directory overrides included)."""
    parts = []
    for command in (["rustc", "-vV"], ["cargo", "-V"]):
        parts.append(subprocess.run(
            command, cwd=repo / "rs", check=True, capture_output=True, text=True, timeout=120,
        ).stdout)
    return "".join(parts)


def crate_dirs(metadata: Mapping[str, object], repo: Path, packages: Sequence[str]) -> list[str]:
    """Repository-relative directories of the workspace crates ``packages`` are built from.

    Normal and build dependencies are followed through workspace (path) packages only; registry
    and Git dependencies are pinned by ``Cargo.lock``. Dev-only dependencies are not built.
    """
    raw_packages = metadata.get("packages")
    resolve = metadata.get("resolve")
    if not isinstance(raw_packages, list) or not isinstance(resolve, dict):
        raise ValueError("cargo metadata lacks packages or resolve")
    local: dict[str, tuple[str, str]] = {}
    for package in raw_packages:
        if isinstance(package, dict) and package.get("source") is None:
            manifest = Path(str(package["manifest_path"])).resolve()
            local[str(package["id"])] = (str(package["name"]), str(manifest.parent))
    deps: dict[str, list[str]] = {}
    nodes = resolve.get("nodes")
    for node in nodes if isinstance(nodes, list) else []:
        if not isinstance(node, dict):
            continue
        wanted = []
        for dep in node.get("deps") or []:
            kinds = dep.get("dep_kinds") or [{"kind": None}]
            if any(kind.get("kind") != "dev" for kind in kinds):
                wanted.append(str(dep["pkg"]))
        deps[str(node["id"])] = wanted
    by_name = {name: package_id for package_id, (name, _) in local.items()}
    missing = [name for name in packages if name not in by_name]
    if missing:
        raise ValueError(f"not workspace packages: {missing}")
    seen: set[str] = set()
    todo = [by_name[name] for name in packages]
    while todo:
        package_id = todo.pop()
        if package_id in seen or package_id not in local:
            continue
        seen.add(package_id)
        todo.extend(deps.get(package_id, []))
    root = repo.resolve()
    return sorted(str(Path(local[package_id][1]).relative_to(root)) for package_id in seen)


def _keyed_env(spec: Spec, declared: Sequence[str]) -> list[tuple[str, str | None]]:
    names = {
        name for name in spec.env
        if (name.startswith("CARGO_") or name.startswith("RUST"))
        and name not in _OUTPUT_NEUTRAL and not name.startswith(_OUTPUT_NEUTRAL_PREFIXES)
    }
    names.update(declared)
    return [(name, spec.env.get(name)) for name in sorted(names)]


#: What ``index.json`` learns per cache name: extra input paths, declared variables, and the
#: executables found holding their build worktree's path.
_INDEX_FIELDS = ("extras", "env", "worktree_bound")


def _index(root: Path, name: str) -> dict[str, list[str]]:
    result: dict[str, list[str]] = {field: [] for field in _INDEX_FIELDS}
    try:
        data: object = json.loads((root / name / INDEX).read_text())
    except (OSError, ValueError):
        return result
    if isinstance(data, dict):
        for field in result:
            value = data.get(field)
            if isinstance(value, list):
                result[field] = sorted({str(item) for item in value})
    return result


def inputs(spec: Spec, repo: Path, root: Path) -> Inputs | None:
    """The key for ``spec`` at ``repo``'s HEAD, or ``None`` when a key input is uncommitted."""
    index = _index(root, spec.name)
    crates = crate_dirs(cargo_metadata(repo), repo, spec.packages)
    workspace = ["rs/Cargo.lock", "rs/Cargo.toml"]
    extras = [path for path in index["extras"] if not any(
        path == crate or path.startswith(crate + "/") for crate in crates)]
    paths = [*workspace, *crates, *extras]
    if _git(repo, "status", "--porcelain", "--untracked-files=all", "--", *paths).strip():
        return None
    listed = {}
    for line in _git(repo, "ls-tree", "HEAD", "--", *paths).splitlines():
        meta, _, path = line.partition("\t")
        listed[path] = meta.split()[-1]
    document = {
        "version": VERSION,
        "name": spec.name,
        "packages": list(spec.packages),
        "binaries": list(spec.binaries),
        "cargo_args": list(spec.cargo_args),
        "toolchain": toolchain(repo),
        "objects": [[path, listed.get(path)] for path in sorted(set(paths))],
        "env": _keyed_env(spec, index["env"]),
    }
    if index["worktree_bound"]:
        # An executable that holds its build worktree's path is reused only in that worktree.
        document["worktree"] = str(repo.resolve())
    digest = hashlib.sha256(json.dumps(document, sort_keys=True).encode()).hexdigest()
    return Inputs(key=digest, extras=tuple(extras))


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def _verified(spec: Spec, key: str, entry: Path) -> dict[str, Path] | None:
    """The entry's executables if its manifest names ``key`` and every file hashes as recorded."""
    try:
        manifest: object = json.loads((entry / MANIFEST).read_text())
    except (OSError, ValueError):
        return None
    if not isinstance(manifest, dict) or manifest.get("key") != key:
        return None
    hashes = manifest.get("sha256")
    if not isinstance(hashes, dict):
        return None
    found = {binary: entry / binary for binary in spec.binaries}
    for binary, path in found.items():
        try:
            if not os.access(path, os.X_OK) or _sha256(path) != hashes.get(binary):
                return None
        except OSError:
            return None
    return found


def lookup(spec: Spec, key: str, root: Path) -> dict[str, Path] | None:
    """The cached executables for ``key``, or ``None``; a hit refreshes the entry's age.

    Every executable is hashed against the manifest first, so a corrupted or altered entry is
    never run.
    """
    entry = root / spec.name / key
    if not (entry / MANIFEST).is_file():
        return None
    found = _verified(spec, key, entry)
    if found is None:
        _note(f"{spec.name}: entry {key[:12]} fails verification; ignored")
        return None
    try:
        os.utime(entry)
    except OSError:
        pass
    return found


def place(spec: Spec, entry: Path, target_dir: Path) -> dict[str, Path] | None:
    """Copy a verified entry's executables to where the build wrote them under ``target_dir``.

    Returns those paths, or ``None`` when the manifest names no place inside ``target_dir`` or a
    copy does not hash as recorded. A file already there that hashes the same is kept. Each copy
    is written beside its destination and renamed over it, so a process that runs the path sees
    the old executable or the new one, never a partial file.
    """
    try:
        manifest: object = json.loads((entry / MANIFEST).read_text())
    except (OSError, ValueError):
        return None
    if not isinstance(manifest, dict):
        return None
    paths, hashes = manifest.get("paths"), manifest.get("sha256")
    if not isinstance(paths, dict) or not isinstance(hashes, dict):
        return None
    placed: dict[str, Path] = {}
    for binary in spec.binaries:
        relative, expected = paths.get(binary), hashes.get(binary)
        if (
            not isinstance(relative, str) or not isinstance(expected, str)
            or Path(relative).is_absolute() or ".." in Path(relative).parts
        ):
            return None
        destination = target_dir / relative
        try:
            if os.access(destination, os.X_OK) and _sha256(destination) == expected:
                placed[binary] = destination
                continue
            destination.parent.mkdir(parents=True, exist_ok=True)
            temporary = destination.with_name(
                f".{destination.name}.{os.getpid()}.{uuid.uuid4().hex[:8]}"
            )
            try:
                shutil.copyfile(entry / binary, temporary)
                os.chmod(temporary, 0o755)
                if _sha256(temporary) != expected:
                    _note(f"{spec.name}: the copy of {binary} does not hash as recorded")
                    return None
                os.replace(temporary, destination)
            finally:
                temporary.unlink(missing_ok=True)
        except OSError as error:
            _note(f"{spec.name}: cannot place {binary} at {destination}: {error}")
            return None
        placed[binary] = destination
    return placed


def _embeds_worktree(executable: Path, repo: Path) -> bool:
    """Whether ``executable`` holds ``repo``'s absolute path (so it is reused only there).

    A crate that reads ``env!("CARGO_MANIFEST_DIR")`` outside its tests compiles its source
    path into the executable; Cargo rebuilds it in each worktree, so the cache must too. An
    executable this cannot read counts as holding the path: never shared on a guess.
    """
    needles = {os.fsencode(str(repo.resolve())), os.fsencode(str(repo.absolute()))}
    keep = max(len(needle) for needle in needles) - 1
    tail = b""
    try:
        with executable.open("rb") as handle:
            for block in iter(lambda: handle.read(1 << 20), b""):
                window = tail + block
                if any(needle in window for needle in needles):
                    return True
                tail = window[-keep:] if keep > 0 else b""
    except OSError as error:
        _note(f"cannot scan {executable} for its worktree path ({error}); reused only here")
        return True
    return False


def _dep_info_paths(path: Path) -> list[Path]:
    """The prerequisites a Cargo ``.d`` file lists (``target: dep dep ...``)."""
    try:
        text = path.read_text()
    except OSError:
        return []
    text = text.replace("\\\n", " ")
    found: list[Path] = []
    for line in text.splitlines():
        head, sep, rest = line.partition(": ")
        if not sep:
            continue
        token = ""
        for word in rest.split(" "):
            if word.endswith("\\"):
                token += word[:-1] + " "
                continue
            token += word
            if token:
                found.append(Path(token))
            token = ""
    return found


def learn(
    built: Mapping[str, Path], target_dir: Path, repo: Path, crates: Sequence[str],
) -> tuple[list[str], list[str]]:
    """Extra inputs and declared environment variables, from the build's own records.

    Extra inputs are the files in ``repo`` outside ``crates`` and ``target_dir`` that Cargo's
    dep-info for each executable lists (``include_str!`` targets, for example). Declared
    variables are every ``rerun-if-env-changed`` a build script under ``target_dir`` printed.
    """
    root = repo.resolve()
    target = target_dir.resolve()
    extras: set[str] = set()
    for executable in built.values():
        for dependency in _dep_info_paths(executable.with_name(executable.name + ".d")):
            try:
                resolved = dependency.resolve()
                relative = resolved.relative_to(root)
            except (OSError, ValueError):
                continue
            if resolved == target or target in resolved.parents:
                continue
            text = str(relative)
            if not any(text == crate or text.startswith(crate + "/") for crate in crates):
                extras.add(text)
    env: set[str] = set()
    for output in target.glob("*/build/*/output"):
        try:
            lines = output.read_text(errors="replace").splitlines()
        except OSError:
            continue
        for line in lines:
            for prefix in ("cargo:rerun-if-env-changed=", "cargo::rerun-if-env-changed="):
                if line.startswith(prefix):
                    env.add(line[len(prefix):].strip())
    return sorted(extras), sorted(env)


def _write_json(path: Path, value: object) -> None:
    temporary = path.with_name(f".{path.name}.{os.getpid()}.{uuid.uuid4().hex}")
    with temporary.open("w") as handle:
        json.dump(value, handle, indent=2, sort_keys=True)
        handle.write("\n")
        handle.flush()
        os.fsync(handle.fileno())
    os.replace(temporary, path)


def tickets_present(admission_dir: Path | None = None) -> bool:
    """Whether any gate ticket is queued, running or deferred on this host.

    The directory is gate.py's own (``GATE_ADMISSION_DIR`` or its default). Anything unreadable
    counts as present, so a doubt never lets a prune run.
    """
    if admission_dir is None:
        try:
            spec = importlib.util.spec_from_file_location(
                "_adapter_cache_gate", REPO_ROOT / "scripts" / "gate.py")
            if spec is None or spec.loader is None:
                return True
            gate = sys.modules.get(spec.name) or importlib.util.module_from_spec(spec)
            if spec.name not in sys.modules:
                sys.modules[spec.name] = gate  # its dataclasses look their module up by name
                spec.loader.exec_module(gate)
            directory: object = gate.state_dir()
        except Exception:  # noqa: BLE001 - any failure to read gate.py means "assume busy"
            return True
        if not isinstance(directory, Path):
            return True
        admission_dir = directory
    for folder in ("queue", "runs", "deferred"):
        try:
            if any(not item.name.startswith(".") for item in (admission_dir / folder).iterdir()):
                return True
        except FileNotFoundError:
            continue
        except OSError:
            return True
    return False


def _size(path: Path) -> int:
    total = 0
    for dirpath, _, filenames in os.walk(path):
        for filename in filenames:
            try:
                total += (Path(dirpath) / filename).lstat().st_size
            except OSError:
                pass
    return total


def _entries(root: Path) -> list[tuple[float, Path]]:
    found = []
    for name_dir in root.iterdir() if root.is_dir() else []:
        if not name_dir.is_dir():
            continue
        for entry in name_dir.iterdir():
            if entry.is_dir():
                try:
                    found.append((entry.stat().st_mtime, entry))
                except OSError:
                    pass
    return sorted(found)


def _remove(entry: Path) -> None:
    for dirpath, dirnames, filenames in os.walk(entry):
        for name in [*dirnames, *filenames]:
            try:
                os.chmod(Path(dirpath) / name, 0o755, follow_symlinks=False)
            except (OSError, NotImplementedError):
                pass
    shutil.rmtree(entry, ignore_errors=True)


def prune(root: Path, admission_dir: Path | None = None, now: float | None = None) -> list[Path]:
    """Remove stale, surplus and oversize entries; nothing while a gate ticket exists."""
    if not root.is_dir() or tickets_present(admission_dir):
        return []
    clock = time.time() if now is None else now
    max_age = _limit("AGENT_UTILS_ADAPTER_CACHE_MAX_AGE_DAYS", 7) * 86400
    max_entries = int(_limit("AGENT_UTILS_ADAPTER_CACHE_MAX_ENTRIES", 12))
    max_bytes = _limit("AGENT_UTILS_ADAPTER_CACHE_MAX_BYTES", 1.5 * 2**30)
    removed: list[Path] = []
    entries = _entries(root)
    per_name: dict[Path, int] = {}
    for mtime, entry in reversed(entries):  # newest first
        if entry.name.startswith("."):
            # A damaged entry set aside, or a staging directory a publisher left behind; a
            # staging directory younger than an hour may still be in use.
            if entry.name.startswith(".corrupt-") or clock - mtime > 3600:
                _remove(entry)
                removed.append(entry)
            continue
        count = per_name[entry.parent] = per_name.get(entry.parent, 0) + 1
        if clock - mtime > max_age or count > max_entries:
            _remove(entry)
            removed.append(entry)
    remaining = [entry for _, entry in entries if entry not in removed]
    total = _size(root)
    for entry in remaining:  # oldest first
        if total <= max_bytes:
            break
        if entry.name.startswith("."):
            continue
        total -= _size(entry)
        _remove(entry)
        removed.append(entry)
    return removed


def publish(
    spec: Spec, built: Mapping[str, Path], target_dir: Path, repo: Path, root: Path,
    admission_dir: Path | None = None,
) -> Path | None:
    """Publish a clean build's executables under the key its inputs give; the entry, or None."""
    target = target_dir.resolve()
    places: dict[str, str] = {}
    for binary in spec.binaries:
        try:
            places[binary] = str(built[binary].resolve().relative_to(target))
        except ValueError:
            _note(f"{spec.name}: not published, {binary} was not built under {target_dir}")
            return None
    crates = crate_dirs(cargo_metadata(repo), repo, spec.packages)
    extras, env = learn(built, target_dir, repo, crates)
    bound = [binary for binary in spec.binaries if _embeds_worktree(built[binary], repo)]
    (root / spec.name).mkdir(parents=True, exist_ok=True)
    index = _index(root, spec.name)
    learned = {
        "extras": sorted({*index["extras"], *extras}),
        "env": sorted({*index["env"], *env}),
        "worktree_bound": sorted({*index["worktree_bound"], *bound}),
    }
    if learned != index:
        _write_json(root / spec.name / INDEX, learned)
    # The key is computed after learning, so the next lookup on this tree finds this entry.
    keyed = inputs(spec, repo, root)
    if keyed is None:
        return None
    entry = root / spec.name / keyed.key
    if entry.exists():
        if _verified(spec, keyed.key, entry) is not None:
            return entry
        # A damaged entry is moved aside (pruned later, never deleted here) and replaced.
        os.rename(entry, entry.with_name(f".corrupt-{keyed.key}-{uuid.uuid4().hex[:8]}"))
    busy = tickets_present(admission_dir)
    if not busy:
        prune(root, admission_dir)
    incoming = sum(path.stat().st_size for path in built.values())
    max_bytes = _limit("AGENT_UTILS_ADAPTER_CACHE_MAX_BYTES", 1.5 * 2**30)
    if _size(root) + incoming > max_bytes:
        _note(f"{spec.name}: not published, the cache is at its {max_bytes / 2**30:.2f} GiB cap"
              + (" and a gate ticket exists, so nothing is pruned" if busy else ""))
        return None
    min_free = _limit("AGENT_UTILS_ADAPTER_CACHE_MIN_FREE_BYTES", 20 * 2**30)
    if shutil.disk_usage(root).free - incoming < min_free:
        _note(f"{spec.name}: not published, under {min_free / 2**30:.0f} GiB of disk would be free")
        return None
    staging = root / spec.name / f".tmp-{keyed.key}-{os.getpid()}-{uuid.uuid4().hex[:8]}"
    staging.mkdir()
    try:
        for binary in spec.binaries:
            copied = staging / binary
            shutil.copyfile(built[binary], copied)
            os.chmod(copied, 0o555)
        _write_json(staging / MANIFEST, {
            "key": keyed.key, "version": VERSION, "name": spec.name,
            "packages": list(spec.packages), "binaries": list(spec.binaries),
            "sha256": {binary: _sha256(staging / binary) for binary in spec.binaries},
            "paths": places, "extras": list(keyed.extras), "created": time.time(),
        })
        try:
            os.rename(staging, entry)
        except OSError:
            if _verified(spec, keyed.key, entry) is None:
                raise
            # Another process published this key first: keep its complete entry.
            _remove(staging)
    except BaseException:
        _remove(staging)
        raise
    return entry


#: One line per cached build this process asked for, for :func:`summary`.
EVENTS: list[str] = []


def summary() -> str:
    """One line for a build step's log: each build's outcome and the cache's size."""
    if ROOT is None:
        return "adapter-cache: disabled"
    try:
        entries = [entry for _, entry in _entries(ROOT) if not entry.name.startswith(".")]
        size = f"{_size(ROOT) / 2**20:.0f} MiB in {len(entries)} entries"
    except OSError:
        size = "size unknown"
    cap = _limit("AGENT_UTILS_ADAPTER_CACHE_MAX_BYTES", 1.5 * 2**30) / 2**30
    return f"adapter-cache: {'; '.join(EVENTS) or 'unused'}; cache {size}, cap {cap:.2f} GiB"


def fetch_or_build(
    spec: Spec, build: Callable[[], dict[str, Path]], target_dir: Path,
    repo: Path = REPO_ROOT, root: Path | None = None,
) -> dict[str, Path]:
    """The executables for ``spec``: from the cache when its inputs match, else from ``build``.

    ``root`` defaults to :data:`ROOT` (``None`` there disables the cache). Any failure of the
    cache itself is reported and falls back to building; a failed build raises as before and
    publishes nothing, and neither does a build during which HEAD moved.
    """
    cache = ROOT if root is None else root
    keyed: Inputs | None = None
    head = ""
    if cache is not None:
        try:
            keyed = inputs(spec, repo, cache)
            head = _git(repo, "rev-parse", "HEAD").strip()
            if keyed is None:
                EVENTS.append(f"{spec.name} built (uncommitted inputs, no key)")
        except Exception as error:  # noqa: BLE001 - the cache must never fail a build
            keyed = None
            EVENTS.append(f"{spec.name} built (no key: {error})")
        if keyed is not None:
            placed: dict[str, Path] | None = None
            try:
                if lookup(spec, keyed.key, cache) is not None:
                    placed = place(spec, cache / spec.name / keyed.key, target_dir)
                    if placed is None:
                        EVENTS.append(f"{spec.name} built (entry {keyed.key[:12]} not placed)")
            except Exception as error:  # noqa: BLE001 - the cache must never fail a build
                EVENTS.append(f"{spec.name} built (entry not placed: {error})")
            if placed is not None:
                EVENTS.append(f"{spec.name} hit {keyed.key[:12]}")
                _note(EVENTS[-1])
                return placed
    built = build()
    if cache is not None and keyed is not None:
        outcome = "not published"
        try:
            if _git(repo, "rev-parse", "HEAD").strip() != head:
                outcome = "not published, HEAD moved during the build"
            elif (entry := publish(spec, built, target_dir, repo, cache)) is not None:
                outcome = f"published {entry.name[:12]}"
        except Exception as error:  # noqa: BLE001 - the cache must never fail a build
            outcome = f"not published ({error})"
        EVENTS.append(f"{spec.name} miss {keyed.key[:12]}, {outcome}")
    if cache is not None and EVENTS:
        _note(EVENTS[-1])
    return built


def main(argv: Sequence[str]) -> int:
    if ROOT is None:
        print("adapter-cache: disabled (AGENT_UTILS_ADAPTER_CACHE=off)")
        return 0
    if argv == ["prune"]:
        if tickets_present():
            print("adapter-cache: a gate ticket is queued or running; nothing pruned")
            return 0
        for entry in prune(ROOT):
            print(f"removed {entry}")
        return 0
    if argv == ["stats"]:
        for mtime, entry in _entries(ROOT):
            stamp = time.strftime("%Y-%m-%d %H:%M", time.localtime(mtime))
            print(f"{stamp}  {_size(entry) / 2**20:8.1f} MiB  {entry.parent.name}/{entry.name}")
        print(f"total {_size(ROOT) / 2**20:.1f} MiB in {ROOT}")
        return 0
    print("usage: adapter_cache.py prune|stats", file=sys.stderr)
    return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
