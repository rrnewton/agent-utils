"""#350 wrkslots-foreign-mount-ns: image mounts copied into unrelated mount namespaces.

An unrelated sandboxed process that creates a private mount namespace while slot images are
mounted keeps its own copy of every slot mount. These tests cover what wrkslots must do about
that without root: recognise a slot's own loop mount by device and file system identity rather
than a rendered backing path, treat an idle copy of it as storage, never attach an image to a
second loop device, keep Git away from an unmounted image checkout, and never let one checkout's
Git repair write into another checkout's directory. The root end-to-end checks live in
``e2e_foreign_namespace.sh``.

They run in the lifecycle suite's isolated shards: test_lifecycle.py collects the names in
COLLECTED. This module is deliberately not a test file of its own.
"""

from __future__ import annotations

import dataclasses
import fcntl
import os
import shutil
import struct
import subprocess
import types
import uuid
from collections.abc import Sequence
from pathlib import Path

import pytest

from wrkslots import cli, imagecmd, imagehelper, slotimage
from wrkslots.tests import test_lifecycle as t


# --------------------------------------------------------------------------- Git repair isolation


def _git(path: Path, *args: str) -> str:
    return subprocess.run(
        ["git", "-C", str(path), *args], check=True, capture_output=True, text=True
    ).stdout


def _repository_with_two_worktrees(tmp_path: Path) -> tuple[Path, Path, Path]:
    repository = tmp_path / "repo"
    repository.mkdir(parents=True)
    _git(repository, "init", "-q", "--initial-branch=main")
    _git(repository, "config", "user.name", "Wrkslots Test")
    _git(repository, "config", "user.email", "wrkslots@example.invalid")
    (repository / "seed.txt").write_text("seed\n", encoding="utf-8")
    _git(repository, "add", "seed.txt")
    _git(repository, "commit", "-q", "-m", "seed")
    first = tmp_path / "slots" / "first"
    second = tmp_path / "slots" / "second"
    first.parent.mkdir()
    _git(repository, "worktree", "add", "-q", "-b", "first", str(first))
    _git(repository, "worktree", "add", "-q", "-b", "second", str(second))
    return repository, first, second


def _empty_like_an_unmounted_image(directory: Path) -> None:
    """Leave only the empty directory, as an unmounted image's mount point is."""

    for entry in directory.iterdir():
        if entry.is_dir() and not entry.is_symlink():
            shutil.rmtree(entry)
        else:
            entry.unlink()


def test_repairing_one_moved_checkout_never_writes_into_a_sibling(tmp_path: Path) -> None:
    repository, first, second = _repository_with_two_worktrees(tmp_path)
    _empty_like_an_unmounted_image(second)
    moved = first.with_name(".first.fenced.1")
    first.rename(moved)

    cli._GitVcs().repair_worktree(repository, moved)

    assert moved.absolute() in cli._GitVcs().listed_worktrees(repository)
    assert _git(moved, "rev-parse", "--show-toplevel").strip() == str(moved)
    assert list(second.iterdir()) == []


def test_git_worktree_repair_itself_writes_into_an_empty_sibling(tmp_path: Path) -> None:
    """The hazard the targeted repair avoids, pinned so a Git change is noticed."""

    repository, first, second = _repository_with_two_worktrees(tmp_path)
    _empty_like_an_unmounted_image(second)
    moved = first.with_name(".first.fenced.1")
    first.rename(moved)

    _git(repository, "worktree", "repair", str(moved))

    assert [entry.name for entry in second.iterdir()] == [".git"]


def test_targeted_repair_refuses_a_git_file_naming_another_repository(tmp_path: Path) -> None:
    repository, first, _second = _repository_with_two_worktrees(tmp_path)
    other, other_first, _other_second = _repository_with_two_worktrees(tmp_path / "other")
    (first / ".git").write_text(
        f"gitdir: {other / '.git' / 'worktrees' / other_first.name}\n", encoding="utf-8"
    )
    with pytest.raises(cli.Refusal, match="not a linked-worktree directory of"):
        cli._GitVcs().repair_worktree(repository, first)
    (first / ".git").write_text("not a pointer\n", encoding="utf-8")
    with pytest.raises(cli.Refusal, match="invalid .git file"):
        cli._GitVcs().repair_worktree(repository, first)
    (first / ".git").unlink()
    with pytest.raises(cli.Refusal, match="no safe .git file"):
        cli._GitVcs().repair_worktree(repository, first)


def _two_slots_with_an_emptied_sibling(tmp_path: Path) -> tuple[Path, Path]:
    project, repository, _remote = t.make_project(tmp_path)
    assert t.create(project).returncode == 0
    second = t.create(project, slot="slot02", agent="codex-2", branch="codex/second")
    assert second.returncode == 0, second.stderr
    t.commit_task(repository, t.checkout(project), "codex/task")
    sibling = t.checkout(project, "slot02")
    _empty_like_an_unmounted_image(sibling)
    t.mark_owner_dead(project, slot="slot01")
    t.set_liveness(project, "dead")
    return project, sibling


def test_removing_one_slot_never_writes_into_an_emptied_sibling_checkout(
    tmp_path: Path,
) -> None:
    project, sibling = _two_slots_with_an_emptied_sibling(tmp_path)

    removed = t.remove(project)

    assert removed.returncode == 0, removed.stderr
    assert not t.checkout(project).exists()
    assert list(sibling.iterdir()) == []


def test_a_rolled_back_fence_never_writes_into_an_emptied_sibling_checkout(
    tmp_path: Path,
) -> None:
    project, sibling = _two_slots_with_an_emptied_sibling(tmp_path)

    refused = t._remove_with_patch(
        project,
        """
        original = cli._assert_salvage_still_matches
        def refuse(*args, **kwargs):
            # Only the recheck after the path fence names the moved paths; a
            # refusal there rolls the fence back.
            if "paths" not in kwargs:
                return original(*args, **kwargs)
            raise cli.Refusal("injected refusal after the path fence")
        cli._assert_salvage_still_matches = refuse
        """,
    )

    assert refused.returncode == 3, refused.stdout + refused.stderr
    assert "injected refusal after the path fence" in refused.stderr
    assert t.checkout(project).is_dir()
    assert len(t.active_slots(project)) == 2
    assert list(sibling.iterdir()) == []


# ------------------------------------------------------------- loop identity (no root)


def _recording(calls: list[tuple[str, ...]]) -> object:
    def run(*args: str, **_kwargs: object) -> str:
        calls.append(args)
        return ""

    return run


def _recording_argv(calls: list[list[str]]) -> object:
    def run(argv: Sequence[str], **_kwargs: object) -> str:
        calls.append(list(argv))
        return ""

    return run


def _noting(order: list[str], label: str) -> object:
    def run(*_args: object, **_kwargs: object) -> list[str]:
        order.append(label)
        return []

    return run


def _noting_first(order: list[str]) -> object:
    def run(*args: str, **_kwargs: object) -> str:
        order.append(args[0])
        return ""

    return run


def _noting_program(order: list[str]) -> object:
    def run(argv: Sequence[str], **_kwargs: object) -> str:
        order.append(argv[0])
        return ""

    return run

UUID_SLOT = "4effbe59-eed3-4925-ac3b-0bc9006edf40"
UUID_STATE = "9a1c2b3d-4e5f-4061-8172-93a4b5c6d7e8"
UUID_OTHER = "11111111-2222-4333-8444-555555555555"


def _fake_image_file(path: Path, uuid_text: str) -> None:
    """A file whose first 4 KiB carry an ext4 superblock magic and UUID, as mkfs writes."""

    block = bytearray(4096)
    struct.pack_into("<H", block, 1024 + 0x38, 0xEF53)
    block[1024 + 0x68 : 1024 + 0x78] = uuid.UUID(uuid_text).bytes
    path.write_bytes(bytes(block))
    path.chmod(0o600)


@dataclasses.dataclass
class _Host:
    """A fake /sys/block and /run/udev/data, and one image-backed slot record."""

    root: Path
    control: Path
    image: slotimage.SlotImage

    @property
    def location(self) -> Path:
        return self.image.location

    def loop(
        self, number: int, backing: str, *, udev_uuid: str | None = None
    ) -> None:
        directory = self.root / "sys" / "block" / f"loop{number}"
        (directory / "loop").mkdir(parents=True)
        (directory / "loop" / "backing_file").write_text(backing + "\n", encoding="utf-8")
        (directory / "dev").write_text(f"7:{number}\n", encoding="ascii")
        if udev_uuid is not None:
            (self.root / "udev" / f"b7:{number}").write_text(
                f"I:1\nE:ID_FS_TYPE=ext4\nE:ID_FS_UUID={udev_uuid}\n", encoding="utf-8"
            )

    def entry(self, number: int, point: Path | None = None, root: str = "/") -> slotimage.MountEntry:
        return slotimage.MountEntry(
            mount_point=self.location if point is None else point,
            fstype="ext4",
            source=f"/dev/loop{number}",
            device=f"7:{number}",
            root=root,
        )


@pytest.fixture
def host(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> _Host:
    root = tmp_path / "host"
    (root / "sys" / "block").mkdir(parents=True)
    (root / "udev").mkdir()
    monkeypatch.setattr(slotimage, "SYS_BLOCK", root / "sys" / "block")
    monkeypatch.setattr(slotimage, "UDEV_DATA", root / "udev")
    for name in ("WRKSLOTS_SANDBOX", "WRKSLOTS_BOX"):
        monkeypatch.delenv(name, raising=False)
    monkeypatch.setattr(slotimage, "_IN_BOX_NAMESPACE", False)
    control = tmp_path / "control"
    (control / "slots").mkdir(parents=True)
    slotimage.register_layout(control, lambda _slot_type, slot: control / "slots" / slot)
    location = control / "slots" / "s1"
    location.mkdir()
    directory = slotimage.image_directory(control, "agent", "s1")
    directory.mkdir(parents=True)
    image = slotimage.SlotImage(
        directory=directory,
        slot="s1",
        slot_type="agent",
        location=location,
        ceiling_bytes=2 * cli.GIB,
        state_ceiling_bytes=cli.GIB,
        backend="kernel",
        created_at="2026-10-18T00:00:00+00:00",
        phase="ready",
    )
    slotimage._write_record(image)
    _fake_image_file(image.slot_image, UUID_SLOT)
    _fake_image_file(image.state_image, UUID_STATE)
    image.state_mount.mkdir()
    return _Host(root=root, control=control, image=image)


def _unresolvable(name: str = "slot.img") -> str:
    # How sysfs renders a backing file once the attaching mount namespace exited:
    # relative to a detached mount's root, resolving nowhere.
    return f"/work/dev-agent-utils/worktrees/slot-images/agent/s1/{name}"


def test_image_uuid_and_fsid_follow_the_superblock(host: _Host) -> None:
    assert slotimage.image_uuid(host.image.slot_image) == UUID_SLOT
    assert slotimage.image_uuid(host.image.directory / "IMAGE.json") is None
    # ext4 reports f_fsid as the XOR of the UUID's two little-endian halves.
    low, high = struct.unpack("<QQ", uuid.UUID(UUID_SLOT).bytes)
    assert slotimage.uuid_fsid(UUID_SLOT) == low ^ high


def test_mount_table_keeps_device_and_mount_root() -> None:
    [entry] = slotimage._parse_mount_table(
        "827 23813 7:40 /sub /p/slots/s\\0401 rw shared:4453 - ext4 /dev/loop40 rw,discard\n"
    )
    assert entry.device == "7:40" and entry.root == "/sub"
    assert entry.mount_point == Path("/p/slots/s 1") and entry.source == "/dev/loop40"


def test_an_unresolvable_rendering_is_identified_by_the_udev_uuid(host: _Host) -> None:
    host.loop(42, _unresolvable(), udev_uuid=UUID_SLOT)
    table = [host.entry(42)]

    assert slotimage.mount_state(host.image.slot_image, host.location, table)[0] == "kernel"
    found = slotimage.image_attachments(host.image.slot_image, host.location, table=table)
    assert [item.name for item in found.devices] == ["/dev/loop42"]
    assert found.host_device is not None and found.stale == ()


def test_the_hosts_own_mount_is_identified_by_statfs_when_nothing_else_can(
    host: _Host, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The host's own mount, attached from a session namespace that has since exited."""

    host.loop(42, _unresolvable())
    table = [host.entry(42)]
    assert slotimage.mount_state(host.image.slot_image, host.location, table)[0] == "unverifiable"

    def statfs(path: Path, device: int) -> int | None:
        if path == host.location and device == os.makedev(7, 42):
            return slotimage.uuid_fsid(UUID_SLOT)
        return None

    monkeypatch.setattr(slotimage, "_mounted_fsid", statfs)
    monkeypatch.setattr(slotimage, "mount_table", lambda mountinfo=None: table)
    assert slotimage.mount_state(host.image.slot_image, host.location, table)[0] == "kernel"
    owned, stale = slotimage.owned_mounts(host.control)
    assert (host.location, "7:42") in owned and stale == frozenset()
    monkeypatch.setattr(
        slotimage, "_mounted_fsid", lambda path, device: slotimage.uuid_fsid(UUID_OTHER)
    )
    assert slotimage.mount_state(host.image.slot_image, host.location, table)[0] == "other"


def test_a_resolvable_rendering_still_matches_by_device_and_inode(host: _Host) -> None:
    host.loop(42, str(host.image.slot_image))
    table = [host.entry(42)]
    assert slotimage.mount_state(host.image.slot_image, host.location, table)[0] == "kernel"
    # A rendering that resolves to another file is not proof by itself (it may
    # be relative to another namespace's root); udev's UUID settles it.
    host.loop(43, str(host.image.state_image))
    assert slotimage.mount_state(host.image.slot_image, host.location, [host.entry(43)])[0] == "unverifiable"
    host.loop(44, str(host.image.state_image), udev_uuid=UUID_STATE)
    assert slotimage.mount_state(host.image.slot_image, host.location, [host.entry(44)])[0] == "other"


def test_a_deleted_backing_file_never_matches_the_image_at_that_path(host: _Host) -> None:
    host.loop(42, f"{host.image.slot_image} (deleted)")
    found = slotimage.image_attachments(host.image.slot_image, host.location, table=[host.entry(42)])
    assert found.devices == () and found.unidentified == ()
    assert slotimage.mount_state(host.image.slot_image, host.location, [host.entry(42)])[0] == "other"


def test_a_bind_of_part_of_the_image_is_not_its_own_mount(host: _Host) -> None:
    host.loop(42, str(host.image.slot_image))
    state, detail = slotimage.mount_state(
        host.image.slot_image, host.location, [host.entry(42, root="/sub")]
    )
    assert state == "other" and "bind of part" in detail


def test_an_image_attached_twice_owns_only_the_hosts_device(host: _Host) -> None:
    host.loop(46, _unresolvable(), udev_uuid=UUID_SLOT)  # held only by a foreign namespace
    host.loop(54, str(host.image.slot_image))  # mounted on the host
    table = [host.entry(54)]
    found = slotimage.image_attachments(host.image.slot_image, host.location, table=table)
    assert sorted(item.name for item in found.devices) == ["/dev/loop46", "/dev/loop54"]
    assert found.host_device is not None and found.host_device.name == "/dev/loop54"
    assert [item.name for item in found.stale] == ["/dev/loop46"]


def test_unmount_refuses_a_loop_mount_it_cannot_identify(
    host: _Host, monkeypatch: pytest.MonkeyPatch
) -> None:
    host.loop(42, _unresolvable())
    monkeypatch.setattr(slotimage, "verification_table", lambda: [host.entry(42)])
    monkeypatch.setattr(slotimage, "mount_table", lambda mountinfo=None: [host.entry(42)])
    called: list[tuple[str, ...]] = []
    monkeypatch.setattr(slotimage, "_privileged", _recording(called))

    with pytest.raises(slotimage.ImageError, match="refusing to treat .* as unmounted"):
        slotimage.unmount(host.image.slot_image, host.location)
    # Converting to a worktree would delete the image of a mount it cannot identify.
    with pytest.raises(slotimage.ImageError, match="refusing to treat"):
        slotimage.discard_files(host.image)
    assert called == []
    assert host.image.slot_image.exists() and host.image.state_image.exists()


def test_kernel_unmount_releases_copies_that_still_hold_the_device(
    host: _Host, monkeypatch: pytest.MonkeyPatch
) -> None:
    states = iter([("kernel", "mounted"), ("absent", "")])
    monkeypatch.setattr(slotimage, "mount_state", lambda *args, **kwargs: next(states))
    monkeypatch.setattr(slotimage, "is_mounted", lambda *args, **kwargs: False)
    commands: list[tuple[str, ...]] = []
    monkeypatch.setattr(
        slotimage, "_privileged", _recording(commands)
    )
    copy = slotimage.LoopEvidence(
        device=os.makedev(7, 42), name="/dev/loop42", backing=_unresolvable(),
        deleted=False, file_identity=None, fsid=None, udev_uuid=UUID_SLOT,
    )
    attached = [(copy,), (copy,), ()]
    monkeypatch.setattr(slotimage, "still_attached", lambda image_file: attached.pop(0) if attached else ())
    released: list[Path] = []
    monkeypatch.setattr(
        slotimage, "release_mount_point_copies", lambda point, anchor=None: released.append(point)
    )

    residual = slotimage.unmount(host.image.slot_image, host.location)

    assert commands == [("umount", str(host.image.slot_image), str(host.location))]
    assert released == [host.location]
    assert residual == []


def test_kernel_unmount_reports_a_copy_that_stays_busy(
    host: _Host, monkeypatch: pytest.MonkeyPatch
) -> None:
    states = iter([("kernel", "mounted"), ("absent", "")])
    monkeypatch.setattr(slotimage, "mount_state", lambda *args, **kwargs: next(states))
    monkeypatch.setattr(slotimage, "is_mounted", lambda *args, **kwargs: False)
    monkeypatch.setattr(slotimage, "_privileged", lambda *args, **kwargs: "")
    copy = slotimage.LoopEvidence(
        device=os.makedev(7, 42), name="/dev/loop42", backing=_unresolvable(),
        deleted=False, file_identity=None, fsid=None, udev_uuid=UUID_SLOT,
    )
    monkeypatch.setattr(slotimage, "still_attached", lambda image_file: (copy,))
    monkeypatch.setattr(slotimage, "release_mount_point_copies", lambda point, anchor=None: None)

    assert slotimage.unmount(host.image.slot_image, host.location) == ["/dev/loop42 (7:42)"]


def test_unmount_passes_the_discard_of_a_stale_copy_only_when_authorized(
    host: _Host, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(slotimage, "is_mounted", lambda *args, **kwargs: False)
    monkeypatch.setattr(slotimage, "still_attached", lambda image_file: ())
    commands: list[tuple[str, ...]] = []
    monkeypatch.setattr(
        slotimage, "_privileged", _recording(commands)
    )
    monkeypatch.setattr(slotimage, "mount_state", lambda *args, **kwargs: ("kernel", "mounted"))
    slotimage.unmount(host.image.slot_image, host.location)
    monkeypatch.setattr(slotimage, "DISCARD_STALE_COPIES", {host.image.slot_image})
    slotimage.unmount(host.image.slot_image, host.location)
    assert commands == [
        ("umount", str(host.image.slot_image), str(host.location)),
        ("umount", str(host.image.slot_image), str(host.location), "--discard-stale-copies"),
    ]


def test_offline_grow_refuses_while_a_loop_device_holds_the_image(
    host: _Host, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A copy in another namespace keeps a superblock open: no e2fsck or resize2fs under it."""

    fuse = dataclasses.replace(host.image, backend="fuse")
    host.loop(40, _unresolvable(), udev_uuid=UUID_SLOT)
    monkeypatch.setattr(slotimage, "verification_table", lambda: [])
    monkeypatch.setattr(slotimage, "mount_table", lambda mountinfo=None: [])
    ran: list[list[str]] = []
    monkeypatch.setattr(slotimage, "_run", _recording_argv(ran))
    before = host.image.slot_image.stat().st_size

    with pytest.raises(slotimage.ImageError, match="still attached to /dev/loop40"):
        slotimage.grow(fuse, 4 * cli.GIB)

    assert ran == [] and host.image.slot_image.stat().st_size == before
    (host.root / "udev" / "b7:40").unlink()
    with pytest.raises(slotimage.ImageError, match="cannot prove that no loop device holds"):
        slotimage.grow(fuse, 4 * cli.GIB)
    assert ran == []


def test_kernel_grow_mounts_first_and_never_resizes_offline(
    host: _Host, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(slotimage, "mounted_backend", lambda *args, **kwargs: None)
    order: list[str] = []
    monkeypatch.setattr(slotimage, "ensure_mounted", _noting(order, "mount"))
    monkeypatch.setattr(
        slotimage, "_privileged", _noting_first(order)
    )
    monkeypatch.setattr(slotimage, "_run", _noting_program(order))
    slotimage.grow(host.image, 4 * cli.GIB)
    assert order[:2] == ["mount", "grow"] and "e2fsck" not in order and "resize2fs" not in order


def test_ensure_all_mounted_continues_past_a_failing_slot(
    host: _Host, monkeypatch: pytest.MonkeyPatch
) -> None:
    second_location = host.control / "slots" / "s2"
    second_location.mkdir()
    directory = slotimage.image_directory(host.control, "agent", "s2")
    directory.mkdir(parents=True)
    second = dataclasses.replace(host.image, directory=directory, slot="s2", location=second_location)
    slotimage._write_record(second)

    def ensure(image: slotimage.SlotImage) -> list[str]:
        if image.slot == "s1":
            raise slotimage.ImageError("image mount point is not empty")
        return [f"mounted {image.slot}"]

    monkeypatch.setattr(slotimage, "ensure_mounted", ensure)
    actions, failures = slotimage.ensure_all_mounted(host.control)
    assert actions == ["mounted s2"]
    assert failures == ["agent/s1: image mount point is not empty"]


# ------------------------------------------------------------------ census (no root)

SLOT = Path("/p/worktrees/slots/s1")


def _line(device: str, point: Path, root: str = "/", source: str = "/dev/loop42") -> str:
    return f"1 2 {device} {root} {point} rw shared:1 - ext4 {source} rw,discard"


def test_a_copy_of_the_slots_own_mount_in_any_table_is_storage(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(cli, "_OWNED_REPRESENTATION_MOUNTS", frozenset({(SLOT, "7:42")}))
    monkeypatch.setattr(cli, "_STALE_REPRESENTATION_MOUNTS", frozenset())
    foreign = _line("7:42", SLOT) + "\n" + _line("8:1", Path("/home"), source="/dev/sda1")
    assert cli._mountinfo_uses_slot(foreign, SLOT, "test") == ()
    # Another device at the slot, or a bind of part of the image, is use.
    assert cli._mountinfo_uses_slot(_line("7:43", SLOT), SLOT, "test") == (f"mount={SLOT}",)
    assert cli._mountinfo_uses_slot(_line("7:42", SLOT, root="/sub"), SLOT, "test") == (
        f"mount={SLOT}",
    )
    assert cli._mountinfo_uses_slot(_line("7:42", SLOT / "x"), SLOT, "test") == (
        f"mount={SLOT / 'x'}",
    )


def test_a_stale_second_attachment_is_reported_as_use_by_name(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(cli, "_OWNED_REPRESENTATION_MOUNTS", frozenset({(SLOT, "7:54")}))
    monkeypatch.setattr(
        cli, "_STALE_REPRESENTATION_MOUNTS", frozenset({(SLOT, "7:46", "/dev/loop46")})
    )
    [use] = cli._mountinfo_uses_slot(_line("7:46", SLOT, source="/dev/loop46"), SLOT, "test")
    assert use.startswith(f"mount={SLOT} [stale-image-copy=/dev/loop46: a second")
    assert cli._mountinfo_uses_slot(_line("7:54", SLOT, source="/dev/loop54"), SLOT, "test") == ()


def _fake_process(proc: Path, pid: int, *, namespace: str, mountinfo: str, root: Path, cwd: Path) -> Path:
    directory = proc / str(pid)
    (directory / "ns").mkdir(parents=True)
    (directory / "ns" / "mnt").symlink_to(namespace)
    (directory / "mountinfo").write_text(mountinfo, encoding="utf-8")
    (directory / "root").symlink_to(root)
    (directory / "cwd").symlink_to(cwd)
    (directory / "exe").symlink_to("/bin/true")
    (directory / "fd").mkdir()
    (directory / "maps").write_text("", encoding="utf-8")
    return directory


def test_a_foreign_process_inside_the_slot_is_still_use(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Negative control: an owned mount line never hides cwd, an open file or a mapping."""

    slot = tmp_path / "slots" / "s1"
    (slot / "sub").mkdir(parents=True)
    (slot / "file").write_text("x", encoding="utf-8")
    monkeypatch.setattr(cli, "_OWNED_REPRESENTATION_MOUNTS", frozenset({(slot, "7:42")}))
    monkeypatch.setattr(cli, "_STALE_REPRESENTATION_MOUNTS", frozenset())
    table = _line("7:42", slot) + "\n"
    proc = tmp_path / "proc"
    idle = _fake_process(proc, 10, namespace="mnt:[1]", mountinfo=table, root=Path("/"), cwd=Path("/"))
    assert cli._process_uses_slot(idle, slot) == []
    inside = _fake_process(
        proc, 11, namespace="mnt:[1]", mountinfo=table, root=Path("/"), cwd=slot / "sub"
    )
    assert cli._process_uses_slot(inside, slot) == [f"cwd={slot / 'sub'}"]
    holding = _fake_process(proc, 12, namespace="mnt:[2]", mountinfo=table, root=Path("/"), cwd=Path("/"))
    (holding / "fd" / "3").symlink_to(slot / "file")
    assert any(use.startswith("fd") and str(slot / "file") in use for use in cli._process_uses_slot(holding, slot))
    mapping = _fake_process(proc, 13, namespace="mnt:[2]", mountinfo=table, root=Path("/"), cwd=Path("/"))
    (mapping / "maps").write_text(
        f"7f00-7f01 r--p 00000000 07:2a 12 {slot / 'file'}\n", encoding="utf-8"
    )
    assert cli._process_uses_slot(mapping, slot) == [f"map={slot / 'file'}"]


def test_stale_copy_holders_lists_every_namespace_and_flags_a_foreign_root(
    tmp_path: Path,
) -> None:
    proc = tmp_path / "proc"
    stale_line = _line("7:46", SLOT, source="/dev/loop46") + "\n"
    other_root = tmp_path / "elsewhere"
    other_root.mkdir()
    _fake_process(proc, 20, namespace="mnt:[100]", mountinfo=stale_line, root=Path("/"), cwd=Path("/"))
    _fake_process(proc, 21, namespace="mnt:[100]", mountinfo=stale_line, root=Path("/"), cwd=Path("/"))
    _fake_process(proc, 30, namespace="mnt:[200]", mountinfo=stale_line, root=Path("/"), cwd=Path("/"))
    _fake_process(proc, 40, namespace="mnt:[300]", mountinfo="", root=Path("/"), cwd=Path("/"))

    [holders] = cli._stale_copy_holders([(SLOT, "7:46", "/dev/loop46")], proc_root=proc)
    assert holders.holder_pids == (20, 21, 30)
    assert holders.mount_namespaces == ("mnt:[100]", "mnt:[200]")
    assert holders.foreign_root_pids == ()
    assert holders.to_obj()["attachment"] == "stale"

    _fake_process(proc, 50, namespace="mnt:[400]", mountinfo=stale_line, root=other_root, cwd=Path("/"))
    [holders] = cli._stale_copy_holders([(SLOT, "7:46", "/dev/loop46")], proc_root=proc)
    assert holders.foreign_root_pids == (50,)


# --------------------------------------------------------- the root helper (sudo -n)


def _sudo_or_skip() -> None:
    if subprocess.run(["sudo", "-n", "true"], capture_output=True, check=False).returncode:
        pytest.skip("needs passwordless sudo")


@pytest.mark.ordinary_environment  # real loop mounts through sudo -n need the initial user namespace
def test_mounting_an_attached_image_again_reuses_its_loop_device(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A second mount of an image goes through the device that already holds it.

    This is what happens when a slot is remounted while a copy of its earlier
    mount (in another mount namespace) keeps the loop device bound: here the
    first mount plays that copy. One device, one file system, two mounts.
    """

    _sudo_or_skip()
    for name in ("WRKSLOTS_SANDBOX", "WRKSLOTS_BOX"):
        monkeypatch.delenv(name, raising=False)
    monkeypatch.setattr(slotimage, "_IN_BOX_NAMESPACE", False)
    image = tmp_path / "slot.img"
    slotimage.make_image_file(image, 512 * 1024 * 1024)
    first = tmp_path / "first"
    second = tmp_path / "second"
    first.mkdir()
    second.mkdir()
    mounted: list[Path] = []
    try:
        attached = slotimage._privileged("mount", str(image), str(first), anchor=tmp_path)
        mounted.append(first)
        assert attached.strip().startswith("attached /dev/loop")
        device = attached.strip().removeprefix("attached ")
        assert slotimage.mount_state(image, first)[0] == "kernel"

        reused = slotimage._privileged("mount", str(image), str(second), anchor=tmp_path)
        mounted.append(second)
        assert reused.strip() == f"reused {device}"
        assert first.stat().st_dev == second.stat().st_dev
        assert [item.name for item in slotimage.image_attachments(image, first).devices] == [device]
        (second / "written-through-second").write_text("x", encoding="utf-8")
        assert (first / "written-through-second").read_text(encoding="utf-8") == "x"

        # The host unmount of one mount leaves the device held by the other:
        # reported, not hidden; releasing the copy cannot touch a mount on
        # another directory.
        residual = slotimage.unmount(image, first)
        mounted.remove(first)
        assert residual == [item.label for item in slotimage.still_attached(image)]
        assert residual and residual[0].startswith(device)
        assert slotimage.unmount(image, second) == []
        mounted.remove(second)
        assert slotimage.still_attached(image) == ()
    finally:
        for point in mounted:
            subprocess.run(
                ["sudo", "-n", "umount", str(point)], capture_output=True, check=False
            )


# ------------------------------------------ unmounted image slots and stray pointers


def _image_record_for(project: Path, slot: str, location: Path, uuid_text: str = UUID_SLOT) -> slotimage.SlotImage:
    """An image record for an existing worktree slot, as if its image were not mounted."""

    control = t.control_directory(project)
    config = cli._load_config(str(project), "testhost")
    del config  # registers the slot layout for record checks
    directory = slotimage.image_directory(control, "agent", slot)
    directory.mkdir(parents=True)
    image = slotimage.SlotImage(
        directory=directory,
        slot=slot,
        slot_type="agent",
        location=Path(os.path.abspath(location)),
        ceiling_bytes=2 * cli.GIB,
        state_ceiling_bytes=cli.GIB,
        backend="kernel",
        created_at="2026-10-18T00:00:00+00:00",
        phase="ready",
    )
    slotimage._write_record(image)
    _fake_image_file(image.slot_image, uuid_text)
    _fake_image_file(image.state_image, UUID_STATE)
    image.state_mount.mkdir()
    return image


@pytest.fixture
def no_real_mounts(monkeypatch: pytest.MonkeyPatch) -> list[Path]:
    """Replace mounting with a fake that refuses a non-empty mount point and records the rest."""

    mounted: list[Path] = []

    def fake_mount(image_file: Path, mount_point: Path, backend_preference: str) -> str:
        if any(mount_point.iterdir()):
            raise slotimage.ImageError(f"image mount point is not empty: {mount_point}")
        mounted.append(mount_point)
        return "kernel"

    monkeypatch.setattr(slotimage, "mount", fake_mount)
    return mounted


def test_an_unmounted_image_slot_gets_no_git_check_and_blocks_only_itself(
    tmp_path: Path, no_real_mounts: list[Path], capsys: pytest.CaptureFixture[str]
) -> None:
    project, _repository, _remote = t.make_project(tmp_path)
    assert t.create(project).returncode == 0
    second = t.create(project, slot="slot02", agent="codex-2", branch="codex/second")
    assert second.returncode == 0, second.stderr
    slot = t.checkout(project).parent
    image = _image_record_for(project, "slot01", slot)

    config = cli._load_config(str(project), "testhost")
    assert f"slot image not mounted: agent/slot01: image mount point is not empty" in (
        capsys.readouterr().err
    )
    states, _archives = cli._validate_global_state(config, require_repository=False)
    record = cli._find_record(cli._load_active(config), "slot01")
    with pytest.raises(cli.Refusal, match="is not mounted at"):
        cli._assert_slot_contents(config, record)
    with pytest.raises(cli.Refusal, match="unmounted image's mount point"):
        cli._repair_registration_at_slot(config, record, record.checkouts[0], slot, cli._GitVcs())
    findings = cli._registry_storage_inconsistencies(config, states)
    kinds = sorted((item.slot, item.kind) for item in findings)
    assert kinds == [("slot01", "image-not-mounted")]
    assert str(image.slot_image) in findings[0].detail
    assert "wrkslots image mount" in findings[0].remedy

    # A command naming another slot proceeds, naming slot01 only in a warning.
    held = t.command(project, "hold", "slot02", "--reason", "keep")
    assert held.returncode == 0, held.stderr
    assert "agent:slot01" in held.stderr
    refused = t.command(project, "hold", "slot01", "--reason", "keep")
    assert refused.returncode == 3 and "is not mounted at" in refused.stderr


def _flat_slot_with_a_stray_pointer(tmp_path: Path) -> tuple[Path, Path, slotimage.SlotImage]:
    project, _repository, _remote = t.make_project(tmp_path, layout="flat")
    assert t.create(project).returncode == 0
    slot = t.checkout(project)
    image = _image_record_for(project, "slot01", slot)
    # What `git worktree repair` leaves in an unmounted image's empty mount point.
    _empty_like_an_unmounted_image(slot)
    git_dir = Path(_git(project / "repo", "rev-parse", "--path-format=absolute", "--git-common-dir").strip())
    admins = [entry for entry in (git_dir / "worktrees").iterdir()]
    [admin] = [entry for entry in admins if (entry / "gitdir").read_text().strip() == f"{slot}/.git"]
    (slot / ".git").write_text(f"gitdir: {admin}\n", encoding="utf-8")
    return project, slot, image


def test_a_stray_git_pointer_is_recognised_only_when_exact(
    tmp_path: Path, no_real_mounts: list[Path]
) -> None:
    project, slot, image = _flat_slot_with_a_stray_pointer(tmp_path)
    config = cli._load_config(str(project), "testhost")
    record = cli._find_record(cli._load_active(config), "slot01")
    found = imagecmd.stray_worktree_pointer(config, record, image)
    assert found is not None and found[0] == slot / ".git"
    pointer = slot / ".git"
    exact = pointer.read_text(encoding="utf-8")
    for content in (exact + "x", exact.rstrip("\n"), exact.replace("worktrees/", "worktrees/../")):
        pointer.write_text(content, encoding="utf-8")
        assert imagecmd.stray_worktree_pointer(config, record, image) is None, content
    pointer.write_text(exact, encoding="utf-8")
    (slot / "notes").write_text("x", encoding="utf-8")
    assert imagecmd.stray_worktree_pointer(config, record, image) is None
    (slot / "notes").unlink()
    assert imagecmd.stray_worktree_pointer(config, record, image) is not None


def test_only_an_authorized_image_mount_removes_a_stray_pointer(
    tmp_path: Path, no_real_mounts: list[Path], capsys: pytest.CaptureFixture[str]
) -> None:
    project, slot, _image = _flat_slot_with_a_stray_pointer(tmp_path)
    pointer = slot / ".git"
    root = ["--project-root", str(project), "--machine", "testhost"]

    assert cli.main([*root, "status"]) == 0
    assert cli.main([*root, "doctor"]) in (0, 1)
    assert pointer.is_file()

    assert cli.main([*root, "image", "mount"]) == 3
    err = capsys.readouterr().err
    assert "holds only Git's own pointer to its worktree registration" in err
    assert "--coordinator-authorized" in err
    assert pointer.is_file() and no_real_mounts == []

    assert cli.main([*root, "image", "mount", "--coordinator-authorized"]) == 0
    assert not pointer.exists()
    assert slot in no_real_mounts
    config = cli._load_config(str(project), "testhost")
    [event] = [
        event
        for event in cli._load_events(config)
        if event["kind"] == "active-state-recorded"
        and cli._as_mapping(event["payload"], "p")["action"] == "stray-git-pointer-removed"
    ]
    evidence = cli._as_mapping(cli._as_mapping(event["payload"], "p")["evidence"], "e")
    assert evidence["path"] == str(pointer) and evidence["coordinator_authorized"] is True


def test_discard_stale_image_copy_needs_authorization_and_an_image_slot(tmp_path: Path) -> None:
    project, _repository, _remote = t.make_project(tmp_path)
    assert t.create(project).returncode == 0
    base = ("remove", "slot01", "--coordinator-pid", str(os.getpid()), "--expected-generation", "1")
    unauthorized = t.raw_command(project, *base, "--discard-stale-image-copy")
    assert unauthorized.returncode == 3
    assert "requires --coordinator-authorized" in unauthorized.stderr
    plain = t.command(project, *base, "--discard-stale-image-copy")
    assert plain.returncode == 3 and "is not image-backed" in plain.stderr
    batch = t.command(
        project, "remove-agent-batch", "--slot", "slot01=1", "--coordinator-pid",
        str(os.getpid()), "--discard-stale-image-copy",
    )
    assert batch.returncode == 2
    assert len(t.active_slots(project)) == 1


# ------------------------------------------------- the root helper's decisions (no root)

_Status = tuple[tuple[int, int], int, int, int, int]


def _status_of(info: os.stat_result, *, offset: int = 0, limit: int = 0, flags: int = 4) -> _Status:
    return (os.major(info.st_dev), os.minor(info.st_dev)), info.st_ino, offset, limit, flags


def test_the_helper_decodes_the_kernels_loop_device_numbers(monkeypatch: pytest.MonkeyPatch) -> None:
    # new_encode_dev(major 259, minor 300): (300 & 0xff) | (259 << 8) | ((300 & ~0xff) << 12)
    encoded = (300 & 0xFF) | (259 << 8) | ((300 & ~0xFF) << 12)
    raw = struct.pack(imagehelper._LOOP_INFO64, encoded, 77, 0, 5, 6, 1, 0, 0, 4, b"", b"", b"", 0, 0)
    monkeypatch.setattr(fcntl, "ioctl", lambda fd, request, buffer: raw)
    assert imagehelper._loop_status(0) == ((259, 300), 77, 5, 6, 4)


def test_the_helper_reuses_one_attachment_and_refuses_two(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    image = tmp_path / "slot.img"
    image.write_bytes(b"x")
    info = os.stat(image)
    stand_in = tmp_path / "loop42"
    stand_in.write_bytes(b"")
    real_open = os.open

    def open_loop(name: str, flags: int, *rest: int, **keywords: int) -> int:
        if str(name).startswith("/dev/loop"):
            return real_open(stand_in, os.O_RDONLY)
        return real_open(name, flags, *rest, **keywords)

    monkeypatch.setattr(os, "open", open_loop)
    statuses: list[_Status] = []
    monkeypatch.setattr(imagehelper, "_loop_status", lambda descriptor: statuses.pop(0))

    monkeypatch.setattr(imagehelper, "_attachments", lambda found: [])
    assert imagehelper._reuse_loop(str(image), info) is None

    one = [("/dev/loop42", _status_of(info))]
    monkeypatch.setattr(imagehelper, "_attachments", lambda found: list(one))
    statuses[:] = [_status_of(info)]
    reused = imagehelper._reuse_loop(str(image), info)
    assert reused is not None and reused[1] == "/dev/loop42"
    os.close(reused[0])

    # Rebound to another file between the scan and the descriptor: refused.
    statuses[:] = [((0, 1), 5, 0, 0, 4)]
    with pytest.raises(imagehelper.HelperError, match="changed while it was checked"):
        imagehelper._reuse_loop(str(image), info)
    # Not a whole-image read-write attachment: refused.
    for odd in (_status_of(info, offset=512), _status_of(info, limit=4096), _status_of(info, flags=5)):
        statuses[:] = [odd]
        with pytest.raises(imagehelper.HelperError, match="offset, a size limit or read-only"):
            imagehelper._reuse_loop(str(image), info)

    two = [("/dev/loop46", _status_of(info)), ("/dev/loop54", _status_of(info))]
    monkeypatch.setattr(imagehelper, "_attachments", lambda found: list(two))
    with pytest.raises(imagehelper.HelperError, match="attached to 2 loop devices .*loop46, /dev/loop54"):
        imagehelper._reuse_loop(str(image), info)


def test_the_helper_never_detaches_a_device_it_reused(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    image = tmp_path / "slot.img"
    image.write_bytes(b"x")
    image.chmod(0o600)
    point = tmp_path / "point"
    point.mkdir()
    stand_in = tmp_path / "loop42"
    stand_in.write_bytes(b"")
    monkeypatch.setattr(imagehelper, "_invoking_uid", lambda: os.getuid())
    monkeypatch.setattr(
        imagehelper, "_reuse_loop", lambda path, info: (os.open(stand_in, os.O_RDONLY), "/dev/loop42")
    )
    configured: list[int] = []
    monkeypatch.setattr(imagehelper, "_configure_loop", lambda descriptor: configured.append(descriptor))
    requests: list[int] = []
    monkeypatch.setattr(fcntl, "ioctl", lambda fd, request, *rest: requests.append(request))

    def failing_mount(*_args: object) -> int:
        return -1

    monkeypatch.setattr(imagehelper, "_libc", lambda: types.SimpleNamespace(mount=failing_mount))
    with pytest.raises(imagehelper.HelperError, match="mount .* on "):
        imagehelper.mount(str(image), str(point))
    assert configured == [] and imagehelper._LOOP_CLR_FD not in requests


def test_the_helper_requires_the_image_for_every_existing_mount_operation() -> None:
    for argv in (["umount", "/x"], ["trim", "/x"], ["grow", "/x"], ["rmdir-lost-found", "/x"]):
        assert imagehelper._main(argv) == 2
    assert imagehelper._main(["umount", "/i", "/x", "--discard-stale-copies", "extra"]) == 2


def test_stale_copy_holders_reads_another_users_table_on_its_own(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """An unreadable namespace link is not a refusal; an unreadable root of a holder is."""

    proc = tmp_path / "proc"
    stale_line = _line("7:46", SLOT, source="/dev/loop46") + "\n"
    _fake_process(proc, 60, namespace="mnt:[1]", mountinfo="", root=Path("/"), cwd=Path("/"))
    other = _fake_process(proc, 61, namespace="mnt:[2]", mountinfo=stale_line, root=Path("/"), cwd=Path("/"))
    (other / "ns" / "mnt").unlink()
    (other / "ns" / "mnt").symlink_to(other / "ns" / "missing-target")
    real_readlink = os.readlink

    def readlink(path: str | os.PathLike[str]) -> str:
        if str(path) == str(other / "ns" / "mnt"):
            raise PermissionError(13, "Permission denied")
        return real_readlink(path)

    monkeypatch.setattr(os, "readlink", readlink)
    [holders] = cli._stale_copy_holders([(SLOT, "7:46", "/dev/loop46")], proc_root=proc)
    assert holders.holder_pids == (61,) and holders.mount_namespaces == ("pid:61",)


#: What test_lifecycle.py collects from this module: its tests and the fixtures they use.
COLLECTED: tuple[str, ...] = tuple(
    name for name in sorted(globals()) if name.startswith("test_")
) + ("host", "no_real_mounts")
