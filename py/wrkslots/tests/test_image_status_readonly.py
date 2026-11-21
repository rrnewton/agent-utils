"""`image status` is read-only (#2061).

Status must report mount state without mounting: on an unmounted
image slot it leaves the slot unmounted and never calls the
ensure-mounted path. Other commands keep the ensure-mount on
config load.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import test_slot_images as images  # type: ignore[import-not-found]  # noqa: E402

from wrkslots import cli, imagecmd, slotimage  # noqa: E402


def _project_with_unmounted_image(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> Path:
    home = images._fake_home(tmp_path)
    project: Path
    project, _environment = images._small_project(tmp_path, home)
    monkeypatch.setenv("HOME", str(home))
    config = cli._load_config(str(project), "testhost")
    directory = slotimage.image_directory(config.control, "agent", "slot07")
    directory.mkdir(parents=True)
    location = project / "worktrees" / "slot07"
    image = slotimage.SlotImage(
        directory=directory, slot="slot07", slot_type="agent",
        location=location, ceiling_bytes=1024, state_ceiling_bytes=1024,
        backend="kernel", created_at="2026-01-01T00:00:00+00:00",
        phase="ready",
    )
    slotimage._write_record(image)
    return project


def test_status_never_mounts(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project = _project_with_unmounted_image(tmp_path, monkeypatch)
    calls: list[Path] = []

    def forbidden(control: Path) -> tuple[list[str], list[str]]:
        calls.append(control)
        raise AssertionError("image status mounted an image")

    monkeypatch.setattr(slotimage, "ensure_all_mounted", forbidden)
    args = argparse.Namespace(
        project_root=str(project), machine="testhost", format="human"
    )
    assert imagecmd._cmd_image_status(args) == 0
    assert calls == []
    out = capsys.readouterr().out
    assert "slot07" in out and "mounted=False" in out


def test_other_loads_still_ensure_mounted(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project = _project_with_unmounted_image(tmp_path, monkeypatch)
    calls: list[Path] = []

    def counting(control: Path) -> tuple[list[str], list[str]]:
        calls.append(control)
        return ([], [])

    monkeypatch.setattr(slotimage, "ensure_all_mounted", counting)
    cli._load_config(str(project), "testhost")
    assert len(calls) == 1
