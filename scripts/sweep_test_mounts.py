#!/usr/bin/env python3
"""Find, and with --apply release, loop mounts and loop devices that dead tests left behind.

The slot-image tests mount ext4 images on loop devices under a temporary directory. A test
killed outright (a cancelled or timed-out validation step) never reaches its teardown, so the
mounts stay, pytest cannot delete its temporary tree through them, and a deleted backing file
keeps its blocks until the loop device is detached (#1064 test-temp-and-mount-leaks).

An entry is released only when every proof holds, and each is printed:

- its mount point lies inside a test's temporary root: a directory directly under a temporary
  directory (``$TMPDIR`` or /tmp, and /var/tmp) whose name a test runner gives it --
  ``tmpXXXXXXXX`` (Python's default), ``pytest-of-<user>``, ``agent-utils-pytest-*`` --
  and the mount point is a slot-image fixture path (``.../worktrees/...``);
- its loop device's backing file is deleted, or lies inside the same root;
- no process references the root (working directory, root, an open file or a mapping);
- the root has not been modified for --min-age minutes, or pytest has renamed it to
  ``pytest-of-<user>/garbage-<uuid>`` (it does that only to delete a finished session, and its
  repeated deletion attempts keep touching the tree).

Anything else on a loop device under a temporary directory is listed as left alone, with the
reason. A dry run is the default: it prints the plan and changes nothing. --apply releases each
verified device on its own: it proves the device again from the live tables immediately before
touching it (tests start all the time) and skips it, logged, when a proof no longer holds; then
``sudo -n umount`` for each of its mount points, and ``sudo -n losetup -d`` if it is still
attached. Every action is appended, with the proofs that justified it, to --log, and the first
release that fails stops the run.

Examples:
    python3 scripts/sweep_test_mounts.py
    python3 scripts/sweep_test_mounts.py --apply
    python3 scripts/sweep_test_mounts.py --self-test
"""

from __future__ import annotations

import argparse
import datetime as dt
import os
import re
import subprocess
import sys
import tempfile
import time
from collections.abc import Callable, Iterable, Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path

DEFAULT_LOG = Path("~/.local/state/agent-utils/sweep-test-mounts.log")
DEFAULT_MIN_AGE_MINUTES = 60.0

#: Names a test runner gives the temporary roots it creates.
TEST_ROOT_NAMES = (
    re.compile(r"tmp[a-z0-9_]{8}"),
    re.compile(r"pytest-of-[A-Za-z0-9_.-]+"),
    re.compile(r"agent-utils-pytest-\d+-\d+-[a-z0-9_]{8}"),
)
PYTEST_USER_ROOT = TEST_ROOT_NAMES[1]
PYTEST_SESSION = re.compile(r"pytest-\d+|garbage-[0-9a-f-]+")
PYTEST_GARBAGE = re.compile(r"pytest-of-[^/]+/garbage-[0-9a-f-]+")


@dataclass
class Device:
    """One attached loop device and where it is mounted."""

    name: str
    backing: str
    mount_points: list[str] = field(default_factory=list)


@dataclass
class Verdict:
    """Whether a device may be released, and why."""

    device: Device
    root: str | None
    proofs: list[str]
    refusal: str | None

    @property
    def verified(self) -> bool:
        return self.refusal is None


def temp_directories() -> list[str]:
    """The temporary directories to search, each with its real path."""

    found: list[str] = []
    for candidate in (tempfile.gettempdir(), "/tmp", "/var/tmp"):
        for path in (candidate, os.path.realpath(candidate)):
            if path not in found and os.path.isdir(path):
                found.append(path)
    return found


def loop_devices(mountinfo: str, sys_block: Path = Path("/sys/block")) -> dict[str, Device]:
    """Attached loop devices with their backing files and mount points."""

    devices: dict[str, Device] = {}
    try:
        names = sorted(entry.name for entry in sys_block.iterdir() if entry.name.startswith("loop"))
    except OSError:
        names = []
    for name in names:
        try:
            backing = (sys_block / name / "loop" / "backing_file").read_text().strip()
        except OSError:
            continue
        devices[f"/dev/{name}"] = Device(f"/dev/{name}", backing)
    for line in mountinfo.splitlines():
        fields = line.split()
        if "-" not in fields:
            continue
        separator = fields.index("-")
        source = fields[separator + 2] if len(fields) > separator + 2 else ""
        if source in devices:
            devices[source].mount_points.append(_unescape(fields[4]))
    return devices


def _unescape(path: str) -> str:
    return re.sub(r"\\([0-7]{3})", lambda match: chr(int(match.group(1), 8)), path)


def test_root(path: str, temp_dirs: Sequence[str]) -> str | None:
    """The test root `path` lies in, or None when it lies in none."""

    for temp in temp_dirs:
        prefix = temp.rstrip("/") + "/"
        if path.startswith(prefix):
            parts = path[len(prefix):].split("/")
            if not any(pattern.fullmatch(parts[0]) for pattern in TEST_ROOT_NAMES):
                continue
            if PYTEST_USER_ROOT.fullmatch(parts[0]):
                # pytest-of-<user> holds every session's directory; the root of one run is the
                # session (pytest-<n>) or the garbage-<uuid> name pytest renames it to.
                if len(parts) < 2 or not PYTEST_SESSION.fullmatch(parts[1]):
                    return None
                return prefix + "/".join(parts[:2])
            return prefix + parts[0]
    return None


def split_root(root: str, temp_dirs: Sequence[str]) -> tuple[int, str]:
    """The index of the temporary directory `root` lies in, and its name below it.

    The name is one component (``tmpabcdefgh``) or, under ``pytest-of-<user>``, two.
    """

    for index, temp in enumerate(temp_dirs):
        prefix = temp.rstrip("/") + "/"
        if root.startswith(prefix):
            return index, root[len(prefix):]
    raise ValueError(f"{root} is not under a temporary directory")


def device_root(device: Device, temp_dirs: Sequence[str]) -> str | None:
    """The one test root a device's mount points (or its backing file) lie in.

    The same root is often seen through several bind mounts of one temporary directory (/tmp,
    /var/tmp and the volume behind them, which may be anywhere). So a root is named by its
    mount points under the temporary directories, and every other mount point must be the same
    path below a directory of the same name: another view of it, not another place.
    """

    places = device.mount_points or [device.backing.removesuffix(" (deleted)")]
    inside = [(place, test_root(place, temp_dirs)) for place in places]
    roots = {root for _, root in inside if root is not None}
    if len(roots) != 1:
        names = {split_root(root, temp_dirs)[1] for root in roots}
        if len(names) != 1:
            return None
    root = min(roots, key=lambda r: split_root(r, temp_dirs)[0])
    name = split_root(root, temp_dirs)[1]
    below = {place[len(found) :] for place, found in inside if found is not None}
    for place, found in inside:
        if found is None and not any(place.endswith("/" + name + rest) for rest in below):
            return None
    return root


def root_views(root: str, temp_dirs: Sequence[str]) -> list[str]:
    """Every path the root `root` may be seen at: its name under each temporary directory."""

    name = split_root(root, temp_dirs)[1]
    return [temp.rstrip("/") + "/" + name for temp in temp_dirs]


def referencing_processes(
    roots: Iterable[str], temp_dirs: Sequence[str], proc: Path = Path("/proc")
) -> dict[str, list[int]]:
    """For each root, the processes whose cwd, root, files or maps use it in any of its views."""

    wanted = {root: {view + "/" for path in root_views(root, temp_dirs)
                     for view in (path, os.path.realpath(path))}
              for root in roots}
    found: dict[str, list[int]] = {root: [] for root in wanted}
    if not wanted:
        return found
    for entry in proc.iterdir():
        if not entry.name.isdigit():
            continue
        paths: list[str] = []
        for link in ("cwd", "root", "exe"):
            try:
                paths.append(os.readlink(entry / link))
            except OSError:
                pass
        try:
            for fd in (entry / "fd").iterdir():
                try:
                    paths.append(os.readlink(fd))
                except OSError:
                    pass
        except OSError:
            pass
        try:
            for line in (entry / "maps").read_text(errors="replace").splitlines():
                parts = line.split(None, 5)
                if len(parts) == 6:
                    paths.append(parts[5])
        except OSError:
            pass
        for root, prefixes in wanted.items():
            if any((path + "/").startswith(prefix) for path in paths for prefix in prefixes):
                found[root].append(int(entry.name))
    return found


def classify(
    devices: Mapping[str, Device],
    temp_dirs: Sequence[str],
    users: Mapping[str, list[int]],
    ages: Mapping[str, float | None],
    min_age_seconds: float,
) -> list[Verdict]:
    """A verdict for each device mounted or backed under a temporary directory."""

    verdicts: list[Verdict] = []
    for device in devices.values():
        places = [*device.mount_points, device.backing.removesuffix(" (deleted)")]
        if not any(place.startswith(tuple(t.rstrip("/") + "/" for t in temp_dirs)) for place in places):
            continue  # not a temporary-directory device at all: never considered
        proofs: list[str] = []
        refusal = None
        root = device_root(device, temp_dirs)
        if root is None:
            refusal = "not inside a test's temporary root (by its name)"
        elif device.mount_points and not all("/worktrees/" in point for point in device.mount_points):
            refusal = "mount point is not a slot-image fixture path"
        else:
            name = split_root(root, temp_dirs)[1]
            proofs.append(f"inside test root {root}")
            deleted = device.backing.endswith(" (deleted)")
            if deleted:
                proofs.append(f"backing file deleted: {device.backing}")
            elif f"/{name}/" in device.backing:
                proofs.append(f"backing file inside the root: {device.backing}")
            else:
                refusal = f"backing file {device.backing} is neither deleted nor inside {root}"
            if refusal is None:
                holders = users.get(root, [])
                age = ages.get(root)
                if holders:
                    refusal = f"in use by process(es) {holders}"
                elif PYTEST_GARBAGE.fullmatch(name):
                    # pytest renames a session to garbage-<uuid> only to delete it, and retries
                    # that deletion at every later start, so the root's age says nothing here.
                    proofs.append("no process references the root")
                    proofs.append("pytest renamed the session to garbage-* for deletion")
                elif age is not None and age < min_age_seconds:
                    refusal = f"root modified {age / 60:.0f} min ago (under {min_age_seconds / 60:.0f})"
                else:
                    proofs.append("no process references the root")
                    if age is not None:
                        proofs.append(f"root untouched for {age / 3600:.1f} h")
        verdicts.append(Verdict(device, root, proofs, refusal))
    return verdicts


def release(
    verdict: Verdict,
    run: Callable[[list[str]], int],
    log: Callable[[str], None],
    mounted: Callable[[str], bool],
    attached: Callable[[str], bool],
) -> bool:
    """Unmount each of a verified device's mount points, then detach it; True when it is gone."""

    proof = "; ".join(verdict.proofs)
    for point in sorted(verdict.device.mount_points, key=len, reverse=True):
        if not mounted(point):
            continue
        code = run(["sudo", "-n", "umount", point])
        log(f"umount {point} ({verdict.device.name}) exit {code} -- {proof}")
        if code != 0 and mounted(point):
            return False
    if attached(verdict.device.name):
        code = run(["sudo", "-n", "losetup", "-d", verdict.device.name])
        log(f"losetup -d {verdict.device.name} exit {code} -- {proof}")
        if code != 0 and attached(verdict.device.name):
            return False
    return True


def _mounted(point: str) -> bool:
    return any(_unescape(line.split()[4]) == point
               for line in Path("/proc/self/mountinfo").read_text().splitlines())


def _attached(device: str) -> bool:
    return Path(f"/sys/block/{device.rsplit('/', 1)[1]}/loop/backing_file").exists()


def _age(root: str, now: float) -> float | None:
    try:
        return now - os.lstat(root).st_mtime
    except OSError:
        return None


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__.split("\n", 1)[0],
        epilog=__doc__.split("\n", 2)[2],
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument("--apply", action="store_true",
                        help="release every verified device (default: dry run, change nothing)")
    parser.add_argument("--min-age", type=float, default=DEFAULT_MIN_AGE_MINUTES,
                        help=f"minutes a root must be untouched (default {DEFAULT_MIN_AGE_MINUTES:g})")
    parser.add_argument("--log", type=Path, default=DEFAULT_LOG,
                        help=f"file --apply appends each action to (default {DEFAULT_LOG})")
    parser.add_argument("--self-test", action="store_true", help="run the built-in checks")
    args = parser.parse_args(argv)
    if args.self_test:
        return _self_test()

    now = time.time()
    temps = temp_directories()
    devices = loop_devices(Path("/proc/self/mountinfo").read_text())
    roots = {root for root in (device_root(device, temps) for device in devices.values()) if root}
    verdicts = classify(devices, temps, referencing_processes(roots, temps),
                        {root: _age(root, now) for root in roots}, args.min_age * 60)
    for verdict in verdicts:
        points = ", ".join(verdict.device.mount_points) or "not mounted"
        if verdict.verified:
            print(f"RELEASE {verdict.device.name} ({points}): " + "; ".join(verdict.proofs))
        else:
            print(f"LEAVE   {verdict.device.name} ({points}): {verdict.refusal}")
    verified = [verdict for verdict in verdicts if verdict.verified]
    if not args.apply:
        print(f"sweep-test-mounts: dry run: {len(verified)} device(s) would be released; "
              "nothing was changed (--apply releases them)")
        return 0
    log_path = args.log.expanduser()
    log_path.parent.mkdir(parents=True, exist_ok=True)

    def log(message: str) -> None:
        stamp = dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
        line = f"{stamp} {message}"
        print(line)
        with log_path.open("a", encoding="utf-8") as handle:
            handle.write(line + "\n")

    released = skipped = 0
    for verdict in verified:
        # Tests start all the time: prove each device again immediately before touching it.
        fresh = recheck(verdict.device.name, args.min_age * 60)
        if fresh is None or not fresh.verified:
            reason = "no longer attached" if fresh is None else fresh.refusal
            log(f"skip {verdict.device.name} -- proof no longer holds: {reason}")
            skipped += 1
            continue
        if not release(fresh, lambda command: subprocess.run(command).returncode,
                       log, _mounted, _attached):
            log(f"stop: releasing {verdict.device.name} failed; no further device is touched")
            print(f"sweep-test-mounts: released {released}, skipped {skipped}, stopped at "
                  f"{verdict.device.name}; log {log_path}")
            return 1
        released += 1
    print(f"sweep-test-mounts: released {released} of {len(verified)} device(s), skipped "
          f"{skipped}; log {log_path}")
    return 0


def recheck(name: str, min_age_seconds: float) -> Verdict | None:
    """A fresh verdict for one device from the live tables, or None once it is gone."""

    temps = temp_directories()
    devices = loop_devices(Path("/proc/self/mountinfo").read_text())
    if name not in devices:
        return None
    one = {name: devices[name]}
    root = device_root(devices[name], temps)
    roots = {root} if root else set()
    verdicts = classify(one, temps, referencing_processes(roots, temps),
                        {r: _age(r, time.time()) for r in roots}, min_age_seconds)
    return verdicts[0] if verdicts else None


def _self_test() -> int:
    temps = ["/tmp", "/data/tmpvol"]
    mounted_dead = "/tmp/tmpdeadroot/project/worktrees/slot01"
    devices = {
        "/dev/loop1": Device("/dev/loop1", "/tmpdeadroot/project/worktrees/slot-images/agent/slot01/slot.img (deleted)",
                             [mounted_dead, "/data/tmpvol/tmpdeadroot/project/worktrees/slot01"]),
        "/dev/loop2": Device("/dev/loop2", "/tmp/tmpliveroot/project/worktrees/slot-images/agent/slot01/slot.img",
                             ["/tmp/tmpliveroot/project/worktrees/slot01"]),
        "/dev/loop3": Device("/dev/loop3", "/tmp/probe894-abcdefgh/project/worktrees/slot-images/a/s/slot.img",
                             ["/tmp/probe894-abcdefgh/project/worktrees/s"]),
        "/dev/loop4": Device("/dev/loop4", "/home/user/work/worktrees/slot-images/agent/real/slot.img",
                             ["/home/user/work/worktrees/slots/real"]),
        "/dev/loop5": Device("/dev/loop5", "/tmp/tmpfreshroo/project/worktrees/slot-images/a/s/slot.img",
                             ["/tmp/tmpfreshroo/project/worktrees/s"]),
        "/dev/loop8": Device("/dev/loop8", "/pytest-of-u/garbage-1a2b/test_x0/project/worktrees/slot-images/a/s/slot.img (deleted)",
                             ["/tmp/pytest-of-u/garbage-1a2b/test_x0/project/worktrees/s",
                              "/data/tmpvol/pytest-of-u/garbage-1a2b/test_x0/project/worktrees/s"]),
        "/dev/loop6": Device("/dev/loop6", "/home/user/elsewhere.img",
                             ["/tmp/tmpforeign1/project/worktrees/s"]),
    }
    users = {"/tmp/tmpliveroot": [4242]}
    ages = {"/tmp/tmpdeadroot": 7200.0, "/tmp/tmpliveroot": 7200.0, "/tmp/tmpfreshroo": 60.0,
            "/tmp/tmpforeign1": 7200.0, "/tmp/pytest-of-u/garbage-1a2b": 60.0}
    assert test_root("/tmp/pytest-of-u/garbage-1a2b-3c/test_x0/project", temps) == "/tmp/pytest-of-u/garbage-1a2b-3c"
    assert test_root("/tmp/pytest-of-u/pytest-17/test_x0", temps) == "/tmp/pytest-of-u/pytest-17"
    assert test_root("/tmp/pytest-of-u/pytest-current/test_x0", temps) is None
    verdicts = {v.device.name: v for v in classify(devices, temps, users, ages, 3600.0)}
    assert "/dev/loop4" not in verdicts, "a device outside temporary directories was considered"
    assert verdicts["/dev/loop1"].verified, verdicts["/dev/loop1"]
    assert verdicts["/dev/loop8"].verified, verdicts["/dev/loop8"]
    assert "inside test root /tmp/pytest-of-u/garbage-1a2b" in verdicts["/dev/loop8"].proofs
    assert "pytest renamed the session to garbage-* for deletion" in verdicts["/dev/loop8"].proofs
    assert "in use" in (verdicts["/dev/loop2"].refusal or ""), verdicts["/dev/loop2"]
    assert "not inside a test's temporary root" in (verdicts["/dev/loop3"].refusal or "")
    assert "min ago" in (verdicts["/dev/loop5"].refusal or ""), verdicts["/dev/loop5"]
    assert "neither deleted nor inside" in (verdicts["/dev/loop6"].refusal or "")
    # Release acts on the verified device alone, one command per mount point, then detaches it.
    commands: list[list[str]] = []
    state = {mounted_dead: True, "/data/tmpvol/tmpdeadroot/project/worktrees/slot01": True,
             "/dev/loop1": True}

    def run(command: list[str]) -> int:
        commands.append(command)
        state[command[-1]] = False
        return 0

    logged: list[str] = []
    assert release(verdicts["/dev/loop1"], run, logged.append,
                   lambda point: state.get(point, False), lambda dev: state.get(dev, False))
    assert [command[2] for command in commands] == ["umount", "umount", "losetup"], commands
    assert all(command[:2] == ["sudo", "-n"] for command in commands)
    assert all("backing file deleted" in line for line in logged), logged
    # Real mountinfo lines: an escaped space, and the separator before the source.
    sys_block = Path(tempfile.mkdtemp(prefix="sweep-test-mounts-self-test-"))
    try:
        (sys_block / "loop7" / "loop").mkdir(parents=True)
        (sys_block / "loop7" / "loop" / "backing_file").write_text("/tmp/tmpabcdefgh/x.img\n")
        info = ("36 25 7:7 / /tmp/tmpabcdefgh/project/worktrees/a\\040b rw - ext4 /dev/loop7 rw\n"
                "37 25 8:1 / /home rw - ext4 /dev/sda1 rw\n")
        parsed = loop_devices(info, sys_block)
        assert parsed["/dev/loop7"].mount_points == ["/tmp/tmpabcdefgh/project/worktrees/a b"]
    finally:
        import shutil
        shutil.rmtree(sys_block)
    print("sweep_test_mounts --self-test: PASSED")
    return 0


if __name__ == "__main__":
    sys.exit(main())
