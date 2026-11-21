"""Sanctioned orphan detach checks (#2050).

``imagehelper.detach-orphan`` acts only on the one orphan shape: a
loop device whose backing file shows deleted, whose recorded path
is exactly the expected slot image under a registry's
``slot-images/<type>/<slot>/`` tree, and which is mounted exactly
at the expected mount point. Each single-condition violation
refuses. The privileged unmount/detach itself is exercised only on
the lane's own scratch images, never here.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from wrkslots import imagehelper, slotimage

CONTROL = "/registry/controls/main"
IMAGE = f"{CONTROL}/slot-images/agent/slot07/slot.img"
STATE_IMAGE = f"{CONTROL}/slot-images/agent/slot07/state.img"
STATE_POINT = f"{CONTROL}/slot-images/agent/slot07/state"
LOCATION = "/project/worktrees/slots/slot07"
DEVICE = "/dev/loop7"
DEVICE_ID = "7:7"
MOUNTS = [(DEVICE_ID, LOCATION), (DEVICE_ID, STATE_POINT)]


def _verify(**overrides: object) -> None:
    values: dict[str, object] = {
        "backing_raw": f"{IMAGE} (deleted)",
        "image_path": IMAGE,
        "mount_point": LOCATION,
        "mounts": MOUNTS,
        "record_location": LOCATION,
        "device": DEVICE,
        "device_id": DEVICE_ID,
    }
    values.update(overrides)
    imagehelper._verify_orphan(**values)  # type: ignore[arg-type]


def test_sanctioned_slot_image_orphan_is_accepted() -> None:
    _verify()


def test_sanctioned_state_image_orphan_is_accepted() -> None:
    _verify(
        backing_raw=f"{STATE_IMAGE} (deleted)",
        image_path=STATE_IMAGE,
        mount_point=STATE_POINT,
        record_location=None,
    )


def test_live_backing_file_refuses_toward_normal_umount() -> None:
    with pytest.raises(imagehelper.HelperError, match="not deleted"):
        _verify(backing_raw=IMAGE)


def test_other_deleted_backing_path_refuses() -> None:
    with pytest.raises(imagehelper.HelperError, match="not /registry"):
        _verify(backing_raw="/elsewhere/slot.img (deleted)")


def test_image_outside_registry_tree_refuses() -> None:
    outside = "/tmp/slot-images-lookalike/slot.img"
    with pytest.raises(imagehelper.HelperError, match="slot-images"):
        _verify(
            backing_raw=f"{outside} (deleted)", image_path=outside,
            record_location=None,
        )


def test_wrong_state_mount_point_refuses() -> None:
    with pytest.raises(imagehelper.HelperError, match="expected mount point"):
        _verify(
            backing_raw=f"{STATE_IMAGE} (deleted)",
            image_path=STATE_IMAGE,
            mount_point=LOCATION,
            record_location=None,
        )


def test_recorded_location_mismatch_refuses() -> None:
    with pytest.raises(imagehelper.HelperError, match="expected mount point"):
        _verify(mount_point="/project/worktrees/slots/other")


def test_device_not_mounted_at_point_refuses() -> None:
    with pytest.raises(imagehelper.HelperError, match="not mounted at"):
        _verify(mounts=[(DEVICE_ID, "/elsewhere")])


def test_helper_usage_rejects_wrong_argument_count() -> None:
    assert imagehelper._main(["detach-orphan", DEVICE]) == 2


def test_slotimage_routes_through_privileged_helper(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    seen: dict[str, object] = {}

    def fake_privileged(operation: str, *arguments: str, **kwargs: object) -> str:
        seen["operation"] = operation
        seen["arguments"] = arguments
        return "detached /dev/loop7 (deleted image x) from y\n"

    monkeypatch.setattr(slotimage, "_privileged", fake_privileged)
    receipt = slotimage.detach_orphan(DEVICE, Path(IMAGE), Path(LOCATION))
    assert seen == {
        "operation": "detach-orphan",
        "arguments": (DEVICE, IMAGE, LOCATION),
    }
    assert receipt.startswith("detached /dev/loop7")
