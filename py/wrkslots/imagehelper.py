"""Privileged operations on slot images, run through ``sudo -n`` (internal).

``python -I imagehelper.py OPERATION ARGS...`` as root, invoked by
:mod:`wrkslots.slotimage` for the kernel backend. Paths come from the
invoking user, so nothing is trusted by name:

* the invoking user is SUDO_UID, as recorded by sudo;
* every path is opened once with O_NOFOLLOW and checked through its
  descriptor (type, owner, and for a mount point: an empty, unmounted
  directory owned by the user);
* the operation then acts on the descriptor (``/proc/self/fd/N``), never on
  the path again, so a path swapped after the check cannot redirect it;
* operations on an existing mount name the image too, and require the root
  of a loop mount whose device the kernel reports (LOOP_GET_STATUS64) as
  backed by exactly that user-owned file, by device and inode. The backing
  path sysfs renders is never trusted: it is relative to the mount namespace
  that attached the device and may not resolve at all once that has exited;
* an image is never attached to a second loop device. ``mount`` reuses the one
  device that already holds the image (a copy of an earlier mount in another
  mount namespace keeps it bound), re-checked on a descriptor held across
  mount(2), so the kernel returns the same file system; with two or more it
  refuses. ``umount`` and ``grow`` refuse an image attached twice unless
  ``umount`` is told to discard the stale copy. ``mount --fresh`` refuses
  whenever any device holds the image: while a stale copy is being
  discarded, the device left holding the image may be that stale copy's.

Operations: ``mount IMAGE MOUNT-POINT [--fresh]``, ``umount IMAGE MOUNT-POINT
[--discard-stale-copies]``, ``rmdir-lost-found IMAGE MOUNT-POINT``,
``trim IMAGE MOUNT-POINT``, ``grow IMAGE MOUNT-POINT``.
"""

from __future__ import annotations

import ctypes
import errno
import fcntl
import os
import stat
import struct
import subprocess
import sys
from collections.abc import Sequence

_LOOP_SET_FD = 0x4C00
_LOOP_CLR_FD = 0x4C01
_LOOP_SET_STATUS64 = 0x4C04
_LOOP_GET_STATUS64 = 0x4C05
_LOOP_SET_CAPACITY = 0x4C07
_LOOP_CONFIGURE = 0x4C0A
_LOOP_CTL_GET_FREE = 0x4C82
_LO_FLAGS_READ_ONLY = 1
_LO_FLAGS_AUTOCLEAR = 4
_FITRIM = 0xC0185879
_LOOP_INFO64 = "=QQQQQIIII64s64s32sQQ"
_O_PATH = 0o10000000
_MS_NOSUID = 2
_MS_NODEV = 4
_MS_NOATIME = 1024
_EXT4_ROOT_INODE = 2
_LOOP_MAJOR = 7


class HelperError(RuntimeError):
    """A privileged image operation was refused or failed."""


def _libc() -> ctypes.CDLL:
    return ctypes.CDLL(None, use_errno=True)


def _check(result: int, what: str) -> None:
    if result != 0:
        error = ctypes.get_errno()
        raise HelperError(f"{what}: {os.strerror(error)}")


def _invoking_uid() -> int:
    if os.geteuid() != 0:
        raise HelperError("the image helper must run as root (through sudo -n)")
    try:
        uid = int(os.environ["SUDO_UID"])
    except (KeyError, ValueError) as exc:
        raise HelperError("the image helper must run through sudo (SUDO_UID is missing)") from exc
    if uid == 0:
        raise HelperError("refusing to manage slot images for root")
    return uid


def _open_mount_point(path: str, uid: int) -> int:
    """Pin a directory that is about to become a mount point: real, empty, unmounted, the user's."""

    descriptor = os.open(path, _O_PATH | os.O_NOFOLLOW | os.O_DIRECTORY | os.O_CLOEXEC)
    try:
        info = os.fstat(descriptor)
        parent = os.stat("..", dir_fd=descriptor, follow_symlinks=False)
        if not stat.S_ISDIR(info.st_mode) or info.st_uid != uid:
            raise HelperError(f"mount point is not a directory owned by the invoking user: {path}")
        if info.st_dev != parent.st_dev or (info.st_dev, info.st_ino) == (parent.st_dev, parent.st_ino):
            raise HelperError(f"mount point already has a file system mounted on it: {path}")
        if os.listdir(f"/proc/self/fd/{descriptor}"):
            raise HelperError(f"mount point is not empty: {path}")
    except BaseException:
        os.close(descriptor)
        raise
    return descriptor


def _loop_status(descriptor: int) -> tuple[tuple[int, int], int, int, int, int]:
    """``((major, minor), inode, offset, size limit, flags)`` of a bound loop device's backing file.

    The kernel encodes ``lo_device`` as ``new_encode_dev`` (huge_encode_dev).
    """

    raw = fcntl.ioctl(descriptor, _LOOP_GET_STATUS64, bytes(struct.calcsize(_LOOP_INFO64)))
    fields = struct.unpack(_LOOP_INFO64, raw)
    device = int(fields[0])
    decoded = ((device & 0xFFF00) >> 8, (device & 0xFF) | ((device >> 12) & 0xFFF00))
    return decoded, int(fields[1]), int(fields[3]), int(fields[4]), int(fields[8])


def _backs(status: tuple[tuple[int, int], int, int, int, int], image: os.stat_result) -> bool:
    return status[0] == (os.major(image.st_dev), os.minor(image.st_dev)) and status[1] == image.st_ino


def _attachments(image: os.stat_result) -> list[tuple[str, tuple[tuple[int, int], int, int, int, int]]]:
    """Every bound loop device whose backing file is the file ``image`` describes."""

    found: list[tuple[str, tuple[tuple[int, int], int, int, int, int]]] = []
    for entry in sorted(os.listdir("/sys/block")):
        if not entry.startswith("loop") or not entry[4:].isdigit():
            continue
        if not os.path.exists(f"/sys/block/{entry}/loop/backing_file"):
            continue  # not bound
        name = f"/dev/{entry}"
        try:
            descriptor = os.open(name, os.O_RDONLY | os.O_CLOEXEC)
        except OSError as exc:
            if exc.errno in (errno.ENOENT, errno.ENXIO, errno.ENODEV):
                continue  # detached, or being detached, meanwhile
            raise HelperError(f"cannot inspect {name}: {os.strerror(exc.errno or 0)}") from exc
        try:
            status = _loop_status(descriptor)
        except OSError as exc:
            if exc.errno == errno.ENXIO:
                continue  # unbound or running down: it holds no file system any more
            raise HelperError(f"cannot read the status of {name}: {os.strerror(exc.errno or 0)}") from exc
        finally:
            os.close(descriptor)
        if _backs(status, image):
            found.append((name, status))
    return found


def _owned_image(image_path: str, uid: int) -> os.stat_result:
    """The identity of a regular image file the invoking user owns, opened without following links."""

    descriptor = os.open(image_path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        info = os.fstat(descriptor)
    finally:
        os.close(descriptor)
    if not stat.S_ISREG(info.st_mode) or info.st_uid != uid:
        raise HelperError(f"image is not a regular file owned by the invoking user: {image_path}")
    return info


def _refuse_double_attachment(image_path: str, image: os.stat_result, what: str) -> None:
    attached = _attachments(image)
    if len(attached) > 1:
        names = ", ".join(name for name, _status in attached)
        raise HelperError(
            f"{image_path} is attached to {len(attached)} loop devices ({names}): two independent "
            f"file systems on one image file; refusing to {what}"
        )


def _loop_name(device: int) -> str:
    base = f"/sys/dev/block/{os.major(device)}:{os.minor(device)}/uevent"
    with open(base, encoding="utf-8") as handle:
        for line in handle:
            if line.startswith("DEVNAME="):
                return "/dev/" + line.split("=", 1)[1].strip()
    raise HelperError("cannot name the loop device")


def _open_owned_mount(
    image_path: str, path: str, uid: int, flags: int = _O_PATH
) -> tuple[int, str, os.stat_result]:
    """Pin the root of IMAGE's loop mount at PATH: (descriptor, loop device name, image identity).

    The device is checked with LOOP_GET_STATUS64 against fstat of the image,
    so it does not matter how (or whether) sysfs can render its backing path.
    """

    descriptor = os.open(path, flags | os.O_NOFOLLOW | os.O_DIRECTORY | os.O_CLOEXEC)
    try:
        info = os.fstat(descriptor)
        if os.major(info.st_dev) != _LOOP_MAJOR or info.st_ino != _EXT4_ROOT_INODE:
            raise HelperError(f"{path} is not the root of a loop-mounted slot image")
        image = _owned_image(image_path, uid)
        name = _loop_name(info.st_dev)
        loop = os.open(name, os.O_RDONLY | os.O_CLOEXEC)
        try:
            status = _loop_status(loop)
        except OSError as exc:
            raise HelperError(f"{path} is not mounted from a bound loop device") from exc
        finally:
            os.close(loop)
        if not _backs(status, image):
            raise HelperError(f"the loop device mounted at {path} ({name}) is not backed by {image_path}")
    except BaseException:
        os.close(descriptor)
        raise
    return descriptor, name, image


def _configure_loop(image: int) -> tuple[int, str]:
    """Attach an open image file to a free loop device with autoclear; return (fd, name)."""

    control = os.open("/dev/loop-control", os.O_RDWR | os.O_CLOEXEC)
    try:
        for _attempt in range(8):
            number = fcntl.ioctl(control, _LOOP_CTL_GET_FREE)
            name = f"/dev/loop{number}"
            device = os.open(name, os.O_RDWR | os.O_CLOEXEC)
            info = struct.pack(_LOOP_INFO64, 0, 0, 0, 0, 0, 0, 0, 0, _LO_FLAGS_AUTOCLEAR, b"", b"", b"", 0, 0)
            try:
                config = struct.pack("=II", image, 0) + info + bytes(64)
                fcntl.ioctl(device, _LOOP_CONFIGURE, config)
                return device, name
            except OSError as exc:
                if exc.errno == errno.EBUSY:
                    os.close(device)
                    continue
                if exc.errno not in (errno.EINVAL, errno.ENOTTY):
                    os.close(device)
                    raise
            try:
                fcntl.ioctl(device, _LOOP_SET_FD, image)
            except OSError as exc:
                os.close(device)
                if exc.errno == errno.EBUSY:
                    continue
                raise
            try:
                fcntl.ioctl(device, _LOOP_SET_STATUS64, info)
            except OSError:
                fcntl.ioctl(device, _LOOP_CLR_FD)
                os.close(device)
                raise
            return device, name
    finally:
        os.close(control)
    raise HelperError("no free loop device")


def _reuse_loop(
    image_path: str, info: os.stat_result, *, fresh: bool = False
) -> tuple[int, str] | None:
    """Open the one loop device that already holds the image, re-checked on that descriptor.

    None when no device holds it. Refuses two or more, and a device bound with
    an offset, a size limit, or read-only, which is not a slot image mount.
    With ``fresh``, refuses any device at all.
    """

    attached = _attachments(info)
    if not attached:
        return None
    if fresh:
        names = ", ".join(name for name, _status in attached)
        raise HelperError(
            f"{image_path} is still attached to {names}, which may hold a stale copy of it; "
            "refusing to mount it through that device (--fresh)"
        )
    _refuse_double_attachment(image_path, info, "mount it again")
    name, _status = attached[0]
    device = os.open(name, os.O_RDWR | os.O_CLOEXEC)
    try:
        # The device may have been released and bound to another file since the
        # scan; the check that counts is the one on the descriptor held across
        # mount(2), as for a device this helper configures itself.
        try:
            status = _loop_status(device)
        except OSError as exc:
            raise HelperError(f"{name} was released while it was checked; retry") from exc
        if not _backs(status, info):
            raise HelperError(f"{name} changed while it was checked; retry")
        if status[2] or status[3] or status[4] & _LO_FLAGS_READ_ONLY:
            raise HelperError(
                f"{image_path} is attached to {name} with an offset, a size limit or read-only; "
                "refusing to mount that device"
            )
    except BaseException:
        os.close(device)
        raise
    return device, name


def mount(image_path: str, mount_point: str, *options: str) -> None:
    """Mount a user-owned image on a pinned, empty directory, never through a second loop device.

    Prints ``reused /dev/loopN`` when another mount (typically a copy in another
    mount namespace) already held the image's device, else ``attached /dev/loopN``.
    With ``--fresh`` it never reuses a device: it refuses while any holds the image.
    """
    if any(option != "--fresh" for option in options):
        raise HelperError(f"unknown mount option: {' '.join(options)}")
    uid = _invoking_uid()
    image = os.open(image_path, os.O_RDWR | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        info = os.fstat(image)
        if not stat.S_ISREG(info.st_mode) or info.st_uid != uid or info.st_nlink != 1:
            raise HelperError(f"image is not a singly linked regular file owned by the invoking user: {image_path}")
        target = _open_mount_point(mount_point, uid)
        try:
            reused = _reuse_loop(image_path, info, fresh="--fresh" in options)
            device, name = reused if reused is not None else _configure_loop(image)
            try:
                libc = _libc()
                libc.mount.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_char_p, ctypes.c_ulong, ctypes.c_char_p]
                result = libc.mount(
                    name.encode(),
                    f"/proc/self/fd/{target}".encode(),
                    b"ext4",
                    _MS_NOSUID | _MS_NODEV | _MS_NOATIME,
                    b"discard",
                )
                if result != 0:
                    error = ctypes.get_errno()
                    if reused is None:
                        # Only a device this helper attached is detached again;
                        # a reused one belongs to the mount that already holds it.
                        fcntl.ioctl(device, _LOOP_CLR_FD)
                    raise HelperError(f"mount {image_path} on {mount_point}: {os.strerror(error)}")
                mounted = os.lstat(mount_point)
                if mounted.st_dev != os.fstat(device).st_rdev:
                    raise HelperError(f"{mount_point} does not show {name} after mounting it; inspect it")
            finally:
                os.close(device)
        finally:
            os.close(target)
    finally:
        os.close(image)
    print(("reused " if reused is not None else "attached ") + name)


def umount(image_path: str, mount_point: str, *options: str) -> None:
    """Unmount a user-owned loop-mounted slot image.

    Refuses an image attached to two loop devices unless ``--discard-stale-copies``
    is given: the other device is a second, stale file system on the same file,
    and releasing this one is how that stale copy would become the only one.
    """
    if any(option != "--discard-stale-copies" for option in options):
        raise HelperError(f"unknown umount option: {' '.join(options)}")
    uid = _invoking_uid()
    descriptor, _name, image = _open_owned_mount(image_path, mount_point, uid)
    identity = os.fstat(descriptor)
    os.close(descriptor)
    if "--discard-stale-copies" not in options:
        _refuse_double_attachment(image_path, image, "unmount it")
    # An open descriptor would keep the mount busy. Re-check the same mount by
    # path without following a symbolic link, then detach exactly that path.
    again = os.lstat(mount_point)
    if (again.st_dev, again.st_ino) != (identity.st_dev, identity.st_ino):
        raise HelperError(f"{mount_point} changed while it was checked")
    _check(_libc().umount2(mount_point.encode(), 8), f"umount {mount_point}")  # UMOUNT_NOFOLLOW


def rmdir_lost_found(image_path: str, mount_point: str) -> None:
    """Remove the root-owned lost+found directory mkfs leaves at a slot image's root."""
    uid = _invoking_uid()
    descriptor, _name, _image = _open_owned_mount(image_path, mount_point, uid)
    try:
        os.rmdir("lost+found", dir_fd=descriptor)
    except FileNotFoundError:
        pass
    finally:
        os.close(descriptor)


def trim(image_path: str, mount_point: str) -> None:
    """Return blocks freed inside a mounted slot image to the host (FITRIM)."""
    uid = _invoking_uid()
    descriptor, _name, _image = _open_owned_mount(image_path, mount_point, uid, os.O_RDONLY)
    try:
        fcntl.ioctl(descriptor, _FITRIM, struct.pack("=QQQ", 0, 0xFFFFFFFFFFFFFFFF, 0))
    finally:
        os.close(descriptor)


def grow(image_path: str, mount_point: str) -> None:
    """Pick up a larger image file on its loop device and grow the file system online."""
    uid = _invoking_uid()
    descriptor, name, image = _open_owned_mount(image_path, mount_point, uid, os.O_RDONLY)
    try:
        _refuse_double_attachment(image_path, image, "grow it")
        device = os.open(name, os.O_RDONLY | os.O_CLOEXEC)
        try:
            fcntl.ioctl(device, _LOOP_SET_CAPACITY)
        finally:
            os.close(device)
        result = subprocess.run([_system_tool("resize2fs"), name], capture_output=True, text=True, check=False, timeout=600)
        if result.returncode != 0:
            raise HelperError(f"resize2fs {name}: {(result.stderr or result.stdout).strip()}")
    finally:
        os.close(descriptor)


def _system_tool(name: str) -> str:
    """A root-owned system executable by absolute path: never resolved on the caller's PATH."""

    for directory in ("/usr/sbin", "/sbin", "/usr/bin", "/bin"):
        candidate = os.path.join(directory, name)
        try:
            info = os.stat(candidate)
        except OSError:
            continue
        if info.st_uid == 0 and not info.st_mode & 0o022 and os.access(candidate, os.X_OK):
            return candidate
    raise HelperError(f"{name} was not found as a root-owned executable in the system directories")


_OPERATIONS = {
    "mount": (mount, 2, 3),
    "umount": (umount, 2, 3),
    "rmdir-lost-found": (rmdir_lost_found, 2, 2),
    "trim": (trim, 2, 2),
    "grow": (grow, 2, 2),
}
_USAGE = (
    "usage: imagehelper.py mount IMAGE MOUNT-POINT [--fresh] | umount IMAGE MOUNT-POINT "
    "[--discard-stale-copies] | rmdir-lost-found|trim|grow IMAGE MOUNT-POINT"
)


def _main(argv: Sequence[str] | None = None) -> int:
    """Run one operation named on the command line; return the exit status."""
    values = list(sys.argv[1:] if argv is None else argv)
    if (
        not values
        or values[0] not in _OPERATIONS
        or not _OPERATIONS[values[0]][1] <= len(values) - 1 <= _OPERATIONS[values[0]][2]
    ):
        print(_USAGE, file=sys.stderr)
        return 2
    operation, _least, _most = _OPERATIONS[values[0]]
    try:
        operation(*values[1:])
    except (HelperError, OSError) as exc:
        print(f"wrkslots image helper: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(_main())
