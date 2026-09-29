"""Disk-image representation for wrkslots slots.

A slot normally stores its checkouts as ordinary directories on the host file
system. With the image representation, the whole slot directory is instead the
mount point of one sparse ext4 image file. The host sees a handful of files per
slot, whatever the agent writes inside it:

    <control>/slot-images/<slot-type>/<slot>/
        IMAGE.json   representation record (location, backend, ceiling)
        slot.img     sparse ext4 image mounted at the slot directory
        state.img    sparse ext4 image for the sandbox's per-slot private HOME directories
        state/       mount point of state.img

Why this helps: a slot's millions of small build files are metadata inside the
image rather than inside the host file system, a runaway writer fills its own
image instead of the host, and reclaim unmounts and deletes one file instead of
walking a tree.

The representation is a storage choice only. Lifecycle policy (ownership,
heartbeats, salvage, reclaim) is unchanged and lives in ``cli.py``.

Nothing here changes host configuration. Mounting uses either:

* ``kernel``: ``sudo -n mount -o loop`` (fast, needs passwordless sudo), or
* ``fuse``: ``fuse2fs`` as the invoking user (no privilege, slower metadata).

``auto`` picks ``kernel`` when ``sudo -n`` works and falls back to ``fuse``.
Images are sparse and are never pre-reserved: the ceiling is an upper bound on
the file's apparent size, not an allocation. Deleted blocks are returned to the
host (``discard`` for kernel mounts, ``fstrim`` on unmount for FUSE mounts).
"""

from __future__ import annotations

import dataclasses
import datetime as _dt
import hashlib
import json
import os
import shutil
import stat as _stat
import subprocess
import sys
import time
import re
from collections.abc import Callable
from pathlib import Path
from typing import Mapping, Sequence

from wrkslots import sandbox

SCHEMA = "wrkslots-slot-image/v1"
IMAGES_DIRECTORY = "slot-images"
RECORD_NAME = "IMAGE.json"
SLOT_IMAGE_NAME = "slot.img"
STATE_IMAGE_NAME = "state.img"
STATE_MOUNT_NAME = "state"
BACKENDS = ("auto", "kernel", "fuse")
DEFAULT_CEILING_BYTES = 512 * 1024**3
DEFAULT_STATE_CEILING_BYTES = 128 * 1024**3
#: Entries mkfs creates that a fresh image may still contain. Anything else at
#: the image root is slot content and blocks destruction.
REPRESENTATION_RESIDUE = frozenset({"lost+found"})
#: The state image holds the sandbox's per-slot private $HOME directories
#: (home_private). /tmp is a fresh tmpfs per launch and is never stored.
STATE_SUBDIRECTORIES = ("home",)


class ImageError(RuntimeError):
    """A slot-image operation could not be completed safely."""


@dataclasses.dataclass(frozen=True)
class ImageSettings:
    """Configuration for newly created slot images."""

    ceiling_bytes: int = DEFAULT_CEILING_BYTES
    state_ceiling_bytes: int = DEFAULT_STATE_CEILING_BYTES
    backend: str = "auto"


#: The comment written above each ``image`` key in a literate configuration.
IMAGE_DOCS: dict[str, str] = {
    "ceiling_bytes": (
        f"Apparent size of each new slot image in bytes (default "
        f"{DEFAULT_CEILING_BYTES // 1024**3} GiB). Images are sparse: an upper bound, not a "
        "reservation. Raise one slot later with `wrkslots image grow`."
    ),
    "state_ceiling_bytes": (
        f"Apparent size of each slot's state image (its private $HOME layer) in bytes "
        f"(default {DEFAULT_STATE_CEILING_BYTES // 1024**3} GiB)."
    ),
    "backend": (
        "How slot images are mounted: kernel (sudo -n mount -o loop), fuse (fuse2fs, no "
        "privilege, slower for metadata-heavy work), or auto (default: kernel when sudo -n "
        "works, else fuse)."
    ),
}


@dataclasses.dataclass(frozen=True)
class SlotImage:
    """One slot's image directory and its durable representation record."""

    directory: Path
    slot: str
    slot_type: str
    location: Path
    ceiling_bytes: int
    state_ceiling_bytes: int
    backend: str
    created_at: str
    phase: str

    @property
    def slot_image(self) -> Path:
        """The sparse image mounted at the slot directory."""

        return self.directory / SLOT_IMAGE_NAME

    @property
    def state_image(self) -> Path:
        """The sparse image holding the sandbox's per-slot private HOME directories."""

        return self.directory / STATE_IMAGE_NAME

    @property
    def state_mount(self) -> Path:
        """Where the state image is mounted."""

        return self.directory / STATE_MOUNT_NAME

    @property
    def record_path(self) -> Path:
        """The durable representation record, IMAGE.json."""

        return self.directory / RECORD_NAME


@dataclasses.dataclass(frozen=True)
class MountEntry:
    """One line of /proc/self/mountinfo, reduced to what this module needs."""

    mount_point: Path
    fstype: str
    source: str


# --------------------------------------------------------------------------- paths


def images_root(control: Path) -> Path:
    """Directory holding every slot's image directory."""

    return control / IMAGES_DIRECTORY


def image_directory(control: Path, slot_type: str, slot: str) -> Path:
    """Image directory for one slot."""

    return images_root(control) / slot_type / slot


def _now() -> str:
    return _dt.datetime.now(_dt.timezone.utc).replace(microsecond=0).isoformat()


# ------------------------------------------------------------------------ records


def _record_to_obj(image: SlotImage) -> dict[str, object]:
    return {
        "schema": SCHEMA,
        "slot": image.slot,
        "slot_type": image.slot_type,
        "location": str(image.location),
        "ceiling_bytes": image.ceiling_bytes,
        "state_ceiling_bytes": image.state_ceiling_bytes,
        "backend": image.backend,
        "created_at": image.created_at,
        "phase": image.phase,
    }


def _write_record(image: SlotImage) -> None:
    temporary = image.record_path.with_name(RECORD_NAME + ".tmp")
    data = json.dumps(_record_to_obj(image), indent=2, sort_keys=True) + "\n"
    with open(temporary, "w", encoding="utf-8") as handle:
        handle.write(data)
        handle.flush()
        os.fsync(handle.fileno())
    os.replace(temporary, image.record_path)
    descriptor = os.open(image.directory, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def _field(raw: Mapping[str, object], key: str, kind: type, path: Path) -> object:
    value = raw.get(key)
    if not isinstance(value, kind) or isinstance(value, bool):
        raise ImageError(f"slot image record {path} has an invalid {key!r}")
    return value


#: Per control directory, the function giving a slot's canonical directory
#: (``slot_type, slot -> path``). Registered by the configuration loader; an
#: image record's location is checked against it, never trusted as written.
_LAYOUTS: dict[Path, Callable[[str, str], Path]] = {}


def register_layout(control: Path, slot_directory: Callable[[str, str], Path]) -> None:
    """Tell image records where each slot of this control directory may live."""

    _LAYOUTS[Path(os.path.abspath(control))] = slot_directory


def _check_location(directory: Path, slot_type: str, slot: str, location: Path) -> None:
    """Refuse a recorded location wrkslots itself would not have chosen for this slot.

    Allowed: the slot's canonical directory; a sibling named ``.<slot>.<suffix>``
    (the path fences used by removal and recovery); and the conversion staging
    directory ``<images>/<type>/.<slot>.convert-<hex>``.
    """

    record = directory / RECORD_NAME
    if directory.name != slot or directory.parent.name != slot_type:
        raise ImageError(f"slot image record {record} names another slot ({slot_type}/{slot})")
    control = Path(os.path.abspath(directory.parent.parent.parent))
    layout = _LAYOUTS.get(control)
    if layout is None:
        raise ImageError(f"no slot layout is registered for {control}; load the configuration first")
    if not location.is_absolute() or os.path.normpath(location) != str(location):
        raise ImageError(f"slot image record {record} has a non-canonical location {location}")
    expected = Path(os.path.abspath(layout(slot_type, slot)))
    if location == expected:
        return
    sibling = re.fullmatch(rf"\.{re.escape(slot)}\.[A-Za-z0-9._-]+", location.name) is not None
    if sibling and location.parent == expected.parent:
        return
    staging = re.fullmatch(rf"\.{re.escape(slot)}\.convert-[0-9a-f]{{8}}", location.name) is not None
    if staging and location.parent == directory.parent:
        return
    raise ImageError(
        f"slot image record {record} places slot {slot_type}/{slot} at {location}, not at {expected}; "
        "refusing to mount or remove anything there"
    )


def read_record(directory: Path) -> SlotImage:
    """Read and validate one image directory's IMAGE.json (its location included)."""

    path = directory / RECORD_NAME
    try:
        raw_value: object = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise ImageError(f"cannot read slot image record {path}: {exc}") from exc
    if not isinstance(raw_value, dict):
        raise ImageError(f"slot image record {path} is not an object")
    raw: dict[str, object] = {str(key): value for key, value in raw_value.items()}
    if raw.get("schema") != SCHEMA:
        raise ImageError(f"slot image record {path} has unsupported schema {raw.get('schema')!r}")
    location = Path(str(_field(raw, "location", str, path)))
    if not location.is_absolute():
        raise ImageError(f"slot image record {path} has a relative location")
    _check_location(directory, str(_field(raw, "slot_type", str, path)), str(_field(raw, "slot", str, path)), location)
    return SlotImage(
        directory=directory,
        slot=str(_field(raw, "slot", str, path)),
        slot_type=str(_field(raw, "slot_type", str, path)),
        location=location,
        ceiling_bytes=int(str(_field(raw, "ceiling_bytes", int, path))),
        state_ceiling_bytes=int(str(_field(raw, "state_ceiling_bytes", int, path))),
        backend=str(_field(raw, "backend", str, path)),
        created_at=str(_field(raw, "created_at", str, path)),
        phase=str(_field(raw, "phase", str, path)),
    )


def load(control: Path, slot_type: str, slot: str) -> SlotImage | None:
    """Return the slot's image record, or None when the slot is not image-backed."""

    directory = image_directory(control, slot_type, slot)
    if not (directory / RECORD_NAME).exists():
        return None
    return read_record(directory)


def all_images(control: Path) -> list[SlotImage]:
    """Every image record under the project's control directory."""

    root = images_root(control)
    found: list[SlotImage] = []
    if not root.is_dir():
        return found
    for type_directory in sorted(root.iterdir()):
        if not type_directory.is_dir() or type_directory.is_symlink():
            continue
        for directory in sorted(type_directory.iterdir()):
            if (directory / RECORD_NAME).exists():
                found.append(read_record(directory))
    return found


def image_for_location(control: Path, location: Path) -> SlotImage | None:
    """Return the image whose recorded location is exactly ``location``."""

    target = Path(os.path.abspath(location))
    for image in all_images(control):
        if Path(os.path.abspath(image.location)) == target:
            return image
    return None


# ------------------------------------------------------------------ subprocesses


def _run(argv: Sequence[str], *, cwd: Path | None = None, timeout: float = 120.0) -> str:
    try:
        result = subprocess.run(
            list(argv),
            cwd=cwd,
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise ImageError(f"{argv[0]} failed to run: {exc}") from exc
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip().splitlines()
        raise ImageError(
            f"{' '.join(argv[:3])} exited {result.returncode}: "
            f"{detail[-1] if detail else 'no output'}"
        )
    return result.stdout


def in_box() -> bool:
    """Whether this process runs in a wrkslots box's own mount namespace.

    A box binds the root launcher's spec directory onto itself, so that path is
    a mount point only inside a box; the environment marker covers a box built
    without a per-user runtime directory.
    """

    if os.environ.get("WRKSLOTS_SANDBOX") in ("userns", "root"):
        return True
    directory = sandbox.spec_directory()
    return directory is not None and os.path.ismount(directory)


_BOX_MOUNT_HINT = (
    "image mounts requested from inside a box run in the host's mount namespace through the "
    "user's systemd manager (systemd-run --user); if that is unavailable, create and remove "
    "image slots outside the box"
)


def _host_argv(argv: Sequence[str]) -> list[str]:
    """``argv``, run in the host's mount namespace.

    Outside a box that is simply here. Inside one, a mount made here would land
    in the box's private namespace, invisible to the host and to every worker
    launched into the slot, so the command runs as a transient service of the
    user's systemd manager, which lives in the host namespace. The box's slave
    propagation then shows the result inside the box too.
    """

    if not in_box():
        return list(argv)
    systemd_run = shutil.which("systemd-run")
    if systemd_run is None:
        raise ImageError(f"systemd-run is not installed; {_BOX_MOUNT_HINT}")
    return [
        systemd_run,
        "--user",
        "--quiet",
        "--collect",
        "--wait",
        "--pipe",
        "--service-type=exec",
        "--",
        *argv,
    ]


def _run_on_host(argv: Sequence[str], *, timeout: float = 120.0) -> str:
    try:
        return _run(_host_argv(argv), timeout=timeout)
    except ImageError as exc:
        if in_box():
            raise ImageError(f"{exc} ({_BOX_MOUNT_HINT})") from exc
        raise


def _privileged_argv() -> list[str]:
    helper = Path(__file__).resolve().with_name("imagehelper.py")
    sudo = shutil.which("sudo") or "sudo"
    return [sudo, "-n", "--", os.path.realpath(sys.executable), "-I", str(helper)]


def _privileged(operation: str, *arguments: str, timeout: float = 120.0) -> str:
    """Run one kernel-backend operation through the checking helper as root, in the host namespace."""

    return _run_on_host([*_privileged_argv(), operation, *arguments], timeout=timeout)


def sudo_available() -> bool:
    """Whether `sudo -n` works without a password prompt (where image mounts run)."""

    try:
        argv = _host_argv([shutil.which("sudo") or "sudo", "-n", "true"])
        return subprocess.run(argv, capture_output=True, timeout=30, check=False).returncode == 0
    except (OSError, subprocess.TimeoutExpired, ImageError):
        return False


def fuse_available() -> bool:
    """Whether fuse2fs and a usable /dev/fuse are present."""

    return shutil.which("fuse2fs") is not None and os.access("/dev/fuse", os.R_OK | os.W_OK)


def resolve_backend(preference: str) -> str:
    """Pick the concrete backend for ``preference`` on this host, or refuse."""

    override = os.environ.get("WRKSLOTS_IMAGE_BACKEND")
    if override:
        preference = override
    if preference not in BACKENDS:
        raise ImageError(f"image backend must be one of {', '.join(BACKENDS)}, not {preference!r}")
    if preference in ("auto", "kernel") and sudo_available():
        return "kernel"
    if preference == "kernel":
        raise ImageError("kernel image backend needs passwordless sudo (sudo -n true failed)")
    if fuse_available():
        return "fuse"
    raise ImageError(
        "no image backend is usable: sudo -n is unavailable and fuse2fs or /dev/fuse is missing"
    )


# ---------------------------------------------------------------------- mountinfo


def _unescape(value: str) -> str:
    for escaped, character in (("\\040", " "), ("\\011", "\t"), ("\\012", "\n"), ("\\134", "\\")):
        value = value.replace(escaped, character)
    return value


def mount_table(mountinfo: Path = Path("/proc/self/mountinfo")) -> list[MountEntry]:
    """Parse the calling process's mount table."""

    entries: list[MountEntry] = []
    try:
        text = mountinfo.read_text(encoding="utf-8", errors="surrogateescape")
    except OSError as exc:
        raise ImageError(f"cannot read {mountinfo}: {exc}") from exc
    for line in text.splitlines():
        left, separator, right = line.partition(" - ")
        if not separator:
            continue
        left_fields = left.split()
        right_fields = right.split()
        if len(left_fields) < 5 or len(right_fields) < 2:
            continue
        entries.append(
            MountEntry(
                mount_point=Path(_unescape(left_fields[4])),
                fstype=right_fields[0],
                source=_unescape(right_fields[1]),
            )
        )
    return entries


def _loop_backing_file(device: str) -> Path | None:
    name = os.path.basename(device)
    if not name.startswith("loop"):
        return None
    try:
        backing = Path(f"/sys/block/{name}/loop/backing_file").read_text(encoding="utf-8").strip()
    except OSError:
        return None
    return Path(backing.removesuffix(" (deleted)"))


def _same_file(left: Path, right: Path) -> bool:
    try:
        a = os.stat(left)
        b = os.stat(right)
    except OSError:
        return False
    return (a.st_dev, a.st_ino) == (b.st_dev, b.st_ino)


def mounted_image_at(
    mount_point: Path, table: Sequence[MountEntry] | None = None
) -> tuple[str, Path] | None:
    """Return ``(backend, image)`` for the topmost image mount at ``mount_point``."""

    target = Path(os.path.abspath(mount_point))
    entries = table if table is not None else mount_table()
    result: tuple[str, Path] | None = None
    for entry in entries:
        if entry.mount_point != target:
            continue
        if entry.fstype == "ext4":
            backing = _loop_backing_file(entry.source)
            result = ("kernel", backing) if backing is not None else None
        elif entry.fstype == "fuse.ext4" or entry.fstype.startswith("fuse"):
            result = ("fuse", Path(entry.source))
        else:
            result = None
    return result


def is_mounted(image_file: Path, mount_point: Path, table: Sequence[MountEntry] | None = None) -> bool:
    """Whether exactly ``image_file`` is mounted at ``mount_point``."""

    mounted = mounted_image_at(mount_point, table)
    return mounted is not None and _same_file(mounted[1], image_file)


def owned_mounts(control: Path) -> frozenset[tuple[Path, str]]:
    """Mount points and sources that are this project's own image mounts.

    The process-use scan treats any mount inside a slot as use. A slot's own
    representation mount is not use, so callers exclude exactly these lines.
    """

    owned: set[tuple[Path, str]] = set()
    try:
        images = all_images(control)
        table = mount_table()
    except ImageError:
        return frozenset()
    for image in images:
        for image_file, mount_point in (
            (image.slot_image, image.location),
            (image.state_image, image.state_mount),
        ):
            target = Path(os.path.abspath(mount_point))
            for entry in table:
                if entry.mount_point != target:
                    continue
                if entry.fstype == "ext4":
                    backing = _loop_backing_file(entry.source)
                    if backing is not None and _same_file(backing, image_file):
                        owned.add((target, entry.source))
                elif entry.fstype.startswith("fuse") and _same_file(Path(entry.source), image_file):
                    owned.add((target, entry.source))
    return frozenset(owned)


# ---------------------------------------------------------------- image lifecycle


def make_image_file(path: Path, size_bytes: int) -> None:
    """Create a sparse, no-copy-on-write ext4 image owned by the invoking user."""

    if path.exists() or path.is_symlink():
        raise ImageError(f"image file already exists: {path}")
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    os.close(descriptor)
    # No-copy-on-write keeps a btrfs host from fragmenting the image into one
    # extent per guest write. It must be set while the file is empty; hosts that
    # are not btrfs reject it harmlessly.
    subprocess.run(["chattr", "+C", str(path)], capture_output=True, check=False)
    os.truncate(path, size_bytes)
    _run(
        [
            "mkfs.ext4",
            "-q",
            "-F",
            "-m",
            "0",
            "-J",
            "size=64",
            "-E",
            f"root_owner={os.getuid()}:{os.getgid()},nodiscard,lazy_itable_init=1",
            str(path),
        ],
        timeout=300,
    )


def _fuse_mount(image_file: Path, mount_point: Path) -> None:
    argv = ["fuse2fs", str(image_file), str(mount_point)]
    # Run the FUSE server in its own transient user service so it outlives the
    # command (or agent sandbox) that mounted it.
    if shutil.which("systemd-run") is not None:
        digest = hashlib.sha256(os.path.abspath(image_file).encode()).hexdigest()[:12]
        unit = f"wrkslots-fuse-{digest}"
        # Nested in the caller's own enclosing slice, like every slot slice:
        # wrkslots never moves work out of a slice site policy placed it in.
        images_slice = sandbox.root_slice().removesuffix(".slice") + "-images.slice"
        probe = subprocess.run(
            [
                "systemd-run",
                "--user",
                "--quiet",
                "--collect",
                f"--unit={unit}-{os.getpid()}-{int(time.time())}",
                f"--slice={images_slice}",
                f"--working-directory={image_file.parent}",
                "--",
                "fuse2fs",
                "-f",
                str(image_file),
                str(mount_point),
            ],
            capture_output=True,
            text=True,
            check=False,
            timeout=30,
        )
        if probe.returncode == 0:
            for _ in range(200):
                if is_mounted(image_file, mount_point):
                    return
                time.sleep(0.05)
            raise ImageError(f"fuse2fs did not mount {image_file} at {mount_point} within 10s")
    if in_box():
        # A daemonizing fuse2fs run here would mount into the box's own namespace.
        raise ImageError(f"cannot start fuse2fs as a user service for {image_file}; {_BOX_MOUNT_HINT}")
    _run(argv, cwd=image_file.parent)
    if not is_mounted(image_file, mount_point):
        raise ImageError(f"fuse2fs exited without mounting {image_file} at {mount_point}")


def mount(image_file: Path, mount_point: Path, backend_preference: str) -> str:
    """Mount ``image_file`` at ``mount_point``; return the backend used."""

    if is_mounted(image_file, mount_point):
        mounted = mounted_image_at(mount_point)
        return mounted[0] if mounted is not None else backend_preference
    if mounted_image_at(mount_point) is not None or os.path.ismount(mount_point):
        raise ImageError(f"{mount_point} already has a different file system mounted on it")
    if mount_point.is_symlink() or not mount_point.is_dir():
        raise ImageError(f"image mount point is not a real directory: {mount_point}")
    if any(mount_point.iterdir()):
        raise ImageError(f"image mount point is not empty: {mount_point}")
    backend = resolve_backend(backend_preference)
    if _fuse_servers(image_file):
        raise ImageError(f"a fuse2fs process still serves {image_file}; refusing a second mount")
    if backend == "kernel":
        _privileged("mount", str(image_file), str(mount_point))
    else:
        _fuse_mount(image_file, mount_point)
    if not is_mounted(image_file, mount_point):
        raise ImageError(f"mount reported success but {image_file} is not mounted at {mount_point}")
    return backend


def _fuse_servers(image_file: Path) -> list[int]:
    """PIDs of fuse2fs processes serving ``image_file``."""

    target = os.path.abspath(image_file)
    found: list[int] = []
    for entry in os.listdir("/proc"):
        if not entry.isdigit():
            continue
        try:
            with open(f"/proc/{entry}/cmdline", "rb") as handle:
                argv = handle.read().split(b"\0")
        except OSError:
            continue
        if not argv or not os.path.basename(argv[0]).startswith(b"fuse2fs"):
            continue
        try:
            cwd = os.readlink(f"/proc/{entry}/cwd")
        except OSError:
            cwd = ""
        for raw in argv[1:]:
            text = raw.decode(errors="surrogateescape")
            if text and os.path.abspath(os.path.join(cwd, text)) == target:
                found.append(int(entry))
                break
    return found


def unmount(image_file: Path, mount_point: Path) -> None:
    """Unmount ``image_file`` from ``mount_point`` if it is mounted there."""

    mounted = mounted_image_at(mount_point)
    if mounted is None or not _same_file(mounted[1], image_file):
        return
    backend = mounted[0]
    if backend == "kernel":
        _privileged("umount", str(mount_point))
    else:
        subprocess.run(["fstrim", str(mount_point)], capture_output=True, check=False)
        _run_on_host([shutil.which("fusermount") or "fusermount", "-u", str(mount_point)])
        for _ in range(200):
            if not is_mounted(image_file, mount_point):
                break
            time.sleep(0.05)
        # The kernel detaches the mount before the FUSE server has written
        # its cached metadata back to the image. Mounting the image again
        # while the old server is still flushing loses those writes (a file
        # deleted just before unmount reappears), so wait for it to exit.
        deadline = time.monotonic() + 120
        while _fuse_servers(image_file):
            if time.monotonic() > deadline:
                raise ImageError(f"fuse2fs for {image_file} did not exit after unmount")
            time.sleep(0.05)
    if is_mounted(image_file, mount_point):
        raise ImageError(f"{mount_point} is still mounted after unmount")


def _prepare_fresh_root(mount_point: Path) -> None:
    lost = mount_point / "lost+found"
    if lost.is_dir() and not lost.is_symlink():
        try:
            lost.rmdir()
        except OSError:
            # lost+found belongs to root on a kernel mount. It is harmless
            # residue, but a nested checkout or `git worktree add` needs an
            # empty root, so remove it with the same privilege that mounted it.
            _privileged("rmdir-lost-found", str(mount_point))


def provision(
    control: Path,
    slot_type: str,
    slot: str,
    location: Path,
    settings: ImageSettings,
) -> SlotImage:
    """Create both images and mount the slot image at ``location``.

    ``location`` must not exist yet; it is created as the (empty) mount point.
    The caller is responsible for lifecycle journaling; a partially provisioned
    image directory is recognized by its ``provisioning`` phase and completed or
    destroyed by :func:`ensure_mounted` and :func:`destroy`.
    """

    directory = image_directory(control, slot_type, slot)
    if directory.exists() or directory.is_symlink():
        raise ImageError(f"slot image directory already exists: {directory}")
    backend = resolve_backend(settings.backend)
    directory.mkdir(parents=True, mode=0o700)
    image = SlotImage(
        directory=directory,
        slot=slot,
        slot_type=slot_type,
        location=Path(os.path.abspath(location)),
        ceiling_bytes=settings.ceiling_bytes,
        state_ceiling_bytes=settings.state_ceiling_bytes,
        backend=backend,
        created_at=_now(),
        phase="provisioning",
    )
    _write_record(image)
    make_image_file(image.slot_image, image.ceiling_bytes)
    make_image_file(image.state_image, image.state_ceiling_bytes)
    image.state_mount.mkdir(mode=0o700)
    location.mkdir(mode=0o755)
    mount(image.slot_image, image.location, backend)
    _prepare_fresh_root(image.location)
    mount(image.state_image, image.state_mount, backend)
    _prepare_fresh_root(image.state_mount)
    for name in STATE_SUBDIRECTORIES:
        (image.state_mount / name).mkdir(mode=0o700, exist_ok=True)
    ready = dataclasses.replace(image, phase="ready")
    _write_record(ready)
    return ready


def ensure_mounted(image: SlotImage) -> list[str]:
    """Mount whichever of a slot's images is not mounted; return what was done."""

    actions: list[str] = []
    if image.phase != "ready":
        return actions
    for image_file, mount_point in (
        (image.slot_image, image.location),
        (image.state_image, image.state_mount),
    ):
        if is_mounted(image_file, mount_point):
            continue
        if not image_file.exists():
            raise ImageError(f"slot image file is missing: {image_file}")
        if not mount_point.exists():
            mount_point.mkdir(mode=0o755 if mount_point == image.location else 0o700)
        mount(image_file, mount_point, image.backend)
        actions.append(f"mounted {image_file.name} at {mount_point}")
    return actions


def ensure_all_mounted(control: Path) -> list[str]:
    """Mount every image-backed slot that is not mounted; return what was done."""

    actions: list[str] = []
    for image in all_images(control):
        actions.extend(ensure_mounted(image))
    return actions


def relocate(image: SlotImage, destination: Path) -> SlotImage:
    """Move a mounted slot image from its location to ``destination``.

    A mount point cannot be renamed, so this unmounts, renames the now-empty
    directory, records the new location, and mounts again. The unmount fails
    while anything still uses the file system, which is the correct refusal.
    """

    source = image.location
    destination = Path(os.path.abspath(destination))
    if destination.exists() or destination.is_symlink():
        raise ImageError(f"relocation destination already exists: {destination}")
    unmount(image.slot_image, source)
    if any(source.iterdir()):
        raise ImageError(f"slot mount point {source} is not empty after unmount")
    moved = dataclasses.replace(image, location=destination)
    _write_record(moved)
    os.rename(source, destination)
    mount(moved.slot_image, destination, image.backend)
    return moved


def root_entries(image: SlotImage) -> list[str]:
    """Names at the root of a slot's mounted image."""

    ensure_mounted(image)
    return sorted(entry.name for entry in image.location.iterdir())


def destroy(image: SlotImage, *, allow_content: bool = False) -> None:
    """Unmount and delete a slot's images, then remove its mount point.

    Refuses while the slot image still holds anything but mkfs residue, unless
    ``allow_content`` is set by a caller that has already salvaged it.
    """

    if image.phase == "ready":
        ensure_mounted(image)
        leftover = [
            name for name in root_entries(image) if name not in REPRESENTATION_RESIDUE
        ]
        if leftover and not allow_content:
            raise ImageError(
                f"slot image at {image.location} is not empty: {', '.join(leftover[:5])}"
            )
    unmount(image.state_image, image.state_mount)
    unmount(image.slot_image, image.location)
    if image.location.exists() and not image.location.is_symlink():
        if any(image.location.iterdir()):
            raise ImageError(f"slot mount point {image.location} has content on the host side")
        image.location.rmdir()
    for path in (image.slot_image, image.state_image, image.record_path):
        try:
            path.unlink()
        except FileNotFoundError:
            pass
    tmp_record = image.directory / (RECORD_NAME + ".tmp")
    if tmp_record.exists():
        tmp_record.unlink()
    if image.state_mount.exists():
        image.state_mount.rmdir()
    image.directory.rmdir()


def usage(image: SlotImage) -> dict[str, object]:
    """Report how much of the host each image costs and how full it is inside."""

    report: dict[str, object] = {
        "name": image.slot,
        "slot_type": image.slot_type,
        "location": str(image.location),
        "phase": image.phase,
    }
    table = mount_table()
    for label, image_file, mount_point in (
        ("slot", image.slot_image, image.location),
        ("state", image.state_image, image.state_mount),
    ):
        entry: dict[str, object] = {"image": str(image_file)}
        try:
            stat = os.stat(image_file)
            entry["ceiling_bytes"] = stat.st_size
            entry["host_allocated_bytes"] = stat.st_blocks * 512
        except OSError as exc:
            entry["error"] = str(exc)
        mounted = mounted_image_at(mount_point, table)
        if mounted is not None and _same_file(mounted[1], image_file):
            entry["mounted"] = True
            entry["backend"] = mounted[0]
            vfs = os.statvfs(mount_point)
            entry["used_bytes"] = (vfs.f_blocks - vfs.f_bfree) * vfs.f_frsize
            entry["free_bytes"] = vfs.f_bavail * vfs.f_frsize
            entry["used_inodes"] = vfs.f_files - vfs.f_ffree
        else:
            entry["mounted"] = False
        report[label] = entry
    return report


def trim(image: SlotImage) -> list[str]:
    """Return freed blocks to the host now (FUSE mounts have no online discard)."""

    done: list[str] = []
    for mount_point in (image.location, image.state_mount):
        mounted = mounted_image_at(mount_point)
        if mounted is None:
            continue
        if mounted[0] == "kernel":
            argv = _host_argv([*_privileged_argv(), "trim", str(mount_point)])
        else:
            argv = ["fstrim", str(mount_point)]
        result = subprocess.run(argv, capture_output=True, text=True, check=False)
        done.append(f"{mount_point}: {'trimmed' if result.returncode == 0 else result.stderr.strip()}")
    return done


def grow(image: SlotImage, new_ceiling_bytes: int) -> SlotImage:
    """Raise the slot image's ceiling (a larger sparse size), online if possible."""

    current = os.stat(image.slot_image).st_size
    if new_ceiling_bytes <= current:
        raise ImageError(
            f"new ceiling {new_ceiling_bytes} must exceed the current {current} bytes; "
            "shrinking is not supported"
        )
    mounted = mounted_image_at(image.location)
    if mounted is not None and mounted[0] == "fuse":
        unmount(image.slot_image, image.location)
        mounted = None
    os.truncate(image.slot_image, new_ceiling_bytes)
    if mounted is not None and mounted[0] == "kernel":
        _privileged("grow", str(image.location), timeout=600)
    else:
        _run(["e2fsck", "-p", "-f", str(image.slot_image)], timeout=600)
        _run(["resize2fs", str(image.slot_image)], timeout=600)
    grown = dataclasses.replace(image, ceiling_bytes=new_ceiling_bytes)
    _write_record(grown)
    ensure_mounted(grown)
    return grown


def discard_files(image: SlotImage) -> None:
    """Unmount and delete a slot's images without inspecting or moving content.

    Only for callers that have already copied the slot's content elsewhere (the
    convert-to-worktree path). The mount point itself is left to the caller.
    """

    unmount(image.state_image, image.state_mount)
    unmount(image.slot_image, image.location)
    for path in (image.slot_image, image.state_image, image.record_path):
        try:
            path.unlink()
        except FileNotFoundError:
            pass
    if image.state_mount.exists():
        image.state_mount.rmdir()
    image.directory.rmdir()


def tree_listing(root: Path) -> list[str]:
    """A canonical listing used to prove two trees hold the same content.

    Modification times are compared at one-second resolution: fuse2fs stores
    whole seconds, and the copy is proven by path, type, size, and mode first.
    """

    result = subprocess.run(
        ["find", str(root), "-mindepth", "1", "-printf", r"%P\t%y\t%s\t%m\t%T@\t%l\n"],
        capture_output=True,
        text=True,
        errors="surrogateescape",
        check=False,
    )
    if result.returncode != 0:
        raise ImageError(f"cannot list {root}: {result.stderr.strip()[-300:]}")
    rows: list[str] = []
    for row in result.stdout.splitlines():
        fields = row.split("\t")
        if len(fields) == 6:
            if fields[1] == "d":
                fields[2] = "-"  # directory sizes differ between file systems
            fields[4] = fields[4].split(".", 1)[0]
        rows.append("\t".join(fields))
    rows.sort()
    return rows


def _restore_metadata(source: Path, destination: Path) -> None:
    """Reapply modes and times from ``source``; some FUSE servers drop them on copy."""

    for directory, names, files in os.walk(source, topdown=False):
        relative = os.path.relpath(directory, source)
        for name in (*files, *names, "."):
            if name == "." and relative == ".":
                continue
            origin = os.path.normpath(os.path.join(directory, name))
            target = os.path.normpath(os.path.join(destination, relative, name))
            info = os.lstat(origin)
            if not _stat.S_ISLNK(info.st_mode):
                os.chmod(target, _stat.S_IMODE(info.st_mode))
            os.utime(target, ns=(info.st_atime_ns, info.st_mtime_ns), follow_symlinks=False)


def copy_tree(source: Path, destination: Path) -> None:
    """Copy the contents of ``source`` into the existing ``destination``; verify."""

    _run(["cp", "-a", "--reflink=auto", f"{source}/.", f"{destination}/"], timeout=24 * 3600)
    _restore_metadata(source, destination)
    before = [row for row in tree_listing(source) if row.split("\t", 1)[0] not in REPRESENTATION_RESIDUE]
    after = [row for row in tree_listing(destination) if row.split("\t", 1)[0] not in REPRESENTATION_RESIDUE]
    if before != after:
        difference = sorted(set(before) ^ set(after))[:5]
        raise ImageError(f"copy of {source} does not match the original: {difference}")
