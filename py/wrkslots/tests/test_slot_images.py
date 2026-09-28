"""Tests for the disk-image slot representation and the slot sandbox.

Pure tests always run. Tests that need a real mount (``sudo -n`` or FUSE) or a
systemd user manager skip with the reason when the host cannot provide it; the
end-to-end lifecycle exercise is ``wrkslots/tests/e2e_images.sh``.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

from wrkslots import cli, sandbox, slotimage


def _init_args(**overrides: object) -> argparse.Namespace:
    values: dict[str, object] = {
        "slot_representation": None,
        "image_ceiling_gib": None,
        "image_backend": None,
    }
    values.update(overrides)
    return argparse.Namespace(**values)


def test_new_project_defaults_to_images(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("WRKSLOTS_INIT_REPRESENTATION", raising=False)
    representation, image = cli._init_representation(_init_args(), tmp_path / ".wrkslots.yml")
    assert representation == "image"
    assert image is None


def test_existing_project_keeps_worktrees_when_key_absent(tmp_path: Path) -> None:
    config = tmp_path / ".wrkslots.yml"
    config.write_text(json.dumps({"schema": 2}), encoding="utf-8")
    representation, _image = cli._init_representation(_init_args(), config)
    assert representation == "worktree"


def test_explicit_flag_wins_and_image_options_merge(tmp_path: Path) -> None:
    config = tmp_path / ".wrkslots.yml"
    config.write_text(json.dumps({"image": {"backend": "fuse"}}), encoding="utf-8")
    representation, image = cli._init_representation(
        _init_args(slot_representation="image", image_ceiling_gib=7), config
    )
    assert representation == "image"
    assert image == {"backend": "fuse", "ceiling_bytes": 7 * cli.GIB}


def test_worktree_representation_is_omitted_from_payload() -> None:
    payload = cli._config_payload("worktrees/slots", "m", "origin", "refs/remotes/origin/main", 60, "t.py")
    assert "slot_representation" not in payload
    image_payload = cli._config_payload(
        "worktrees/slots", "m", "origin", "refs/remotes/origin/main", 60, "t.py",
        slot_representation="image",
    )
    assert image_payload["slot_representation"] == "image"
    assert cli._canonical_config_payload({**payload, "slot_representation": "worktree"}) == payload


def test_image_settings_validation() -> None:
    assert cli._parse_image_settings({}) == slotimage.ImageSettings()
    parsed = cli._parse_image_settings({"image": {"ceiling_bytes": 2 * cli.GIB, "backend": "fuse"}})
    assert parsed.ceiling_bytes == 2 * cli.GIB and parsed.backend == "fuse"
    with pytest.raises(cli.StateError):
        cli._parse_image_settings({"image": {"backend": "loopback"}})
    with pytest.raises(cli.StateError):
        cli._parse_image_settings({"image": {"surprise": 1}})
    with pytest.raises(cli.StateError):
        cli._parse_slot_representation({"slot_representation": "zfs"})


def test_sandbox_settings_validation() -> None:
    settings = sandbox.settings_from_obj({"home": "select", "home_paths": ["bin"], "memory_max": "32G"})
    assert settings.home == "select" and settings.home_paths == ("bin",)
    for bad in (
        {"home": "rw"},
        {"home_paths": ["../escape"]},
        {"home_paths": ["/abs"]},
        {"memory_max": "lots"},
        {"unknown": 1},
        {"tasks_max": 0},
    ):
        with pytest.raises(sandbox.SandboxError):
            sandbox.settings_from_obj(bad)


def test_slot_slices_nest_under_the_callers_slice(tmp_path: Path) -> None:
    cgroup = tmp_path / "cgroup"
    cgroup.write_text(
        "0::/user.slice/user-1.slice/user@1.service/jail.slice/agent-7.scope\n", encoding="utf-8"
    )
    assert sandbox.parent_slice(cgroup) == "jail.slice"
    assert sandbox.root_slice("jail.slice") == "jail-wrkslots.slice"
    if shutil.which("systemd-escape") is None:
        pytest.skip("systemd-escape is not installed")
    assert sandbox.slice_name("agent", "kvm-foo", "jail.slice") == "jail-wrkslots-kvm\\x2dfoo.slice"
    cgroup.write_text("0::/user.slice/user-1.slice/user@1.service/app.scope\n", encoding="utf-8")
    assert sandbox.parent_slice(cgroup) is None
    assert sandbox.slice_name("validate", "v1", "x.slice").startswith("x-wrkslots-validate")


def test_owned_mount_lines_are_excluded_from_use_evidence(monkeypatch: pytest.MonkeyPatch) -> None:
    line = "1 2 0:1 / /p/worktrees/slots/s1 rw - fuse.ext4 /p/slot.img rw"
    other = "3 2 0:2 / /p/worktrees/slots/s1/bind rw - ext4 /dev/sda1 rw"
    monkeypatch.setattr(
        cli, "_OWNED_REPRESENTATION_MOUNTS", frozenset({(Path("/p/worktrees/slots/s1"), "/p/slot.img")})
    )
    parsed = cli._parse_mountinfo_paths(f"{line}\n{other}\n", "test")
    assert [str(path) for path, _raw in parsed] == ["/", "/p/worktrees/slots/s1/bind", "/dev/sda1"]
    assert cli._is_owned_image_root(Path("/p/worktrees/slots/s1"))
    assert not cli._is_owned_image_root(Path("/p/worktrees/slots/s2"))


def test_build_spec_keeps_the_slot_writable_and_hides_home_aliases(tmp_path: Path) -> None:
    view = sandbox.SlotView(
        slot="s1",
        slot_type="agent",
        slot_path=tmp_path / "slot",
        state_directory=tmp_path / "state",
        git_directories=(tmp_path / "repo.git",),
        control_directory=tmp_path / "control",
        representation="image",
    )
    spec = sandbox.build_spec(view, sandbox.SandboxSettings(home="none"), tmp_path / "home", tmp_path / "slot")
    writable = spec["writable"]
    assert isinstance(writable, list)
    assert str(tmp_path / "slot") in writable and str(tmp_path / "repo.git") in writable
    home = spec["home"]
    assert isinstance(home, dict) and home["expose"] == "none"
    assert home["source"] == str(tmp_path / "state" / "home-none")
    redirects = spec["redirects"]
    assert isinstance(redirects, list) and [str(tmp_path / "state" / "tmp"), "/tmp"] in redirects


def _backend_or_skip() -> str:
    try:
        return slotimage.resolve_backend("auto")
    except slotimage.ImageError as exc:
        pytest.skip(f"no slot image backend on this host: {exc}")


def test_image_lifecycle_relocate_and_destroy(tmp_path: Path) -> None:
    backend = _backend_or_skip()
    control = tmp_path / "control"
    (control / "slots").mkdir(parents=True)
    location = control / "slots" / "s1"
    image = slotimage.provision(
        control, "agent", "s1", location, slotimage.ImageSettings(ceiling_bytes=2 * cli.GIB, backend=backend)
    )
    try:
        assert slotimage.is_mounted(image.slot_image, location)
        assert os.listdir(location) == []
        assert sorted(os.listdir(image.state_mount)) == ["home", "tmp"]
        # Sparse: the image costs far less than its ceiling.
        assert os.stat(image.slot_image).st_blocks * 512 < 256 * 1024 * 1024
        (location / "kept").write_text("x", encoding="utf-8")
        (location / "deleted").write_text("y", encoding="utf-8")
        (location / "deleted").unlink()
        with pytest.raises(slotimage.ImageError):
            slotimage.destroy(image)
        moved = slotimage.relocate(image, control / "slots" / ".s1.fenced")
        assert not location.exists()
        # A file deleted just before the move must not reappear afterwards.
        assert os.listdir(moved.location) == ["kept"]
        assert slotimage.image_for_location(control, moved.location) == moved
        (moved.location / "kept").unlink()
        slotimage.destroy(moved)
        assert not moved.location.exists() and not moved.directory.exists()
    finally:
        for leftover in slotimage.all_images(control):
            slotimage.destroy(leftover, allow_content=True)


def test_sandbox_blocks_writes_outside_the_slot(tmp_path: Path) -> None:
    if shutil.which("systemd-run") is None or shutil.which("busctl") is None:
        pytest.skip("needs a systemd user manager")
    probe = subprocess.run(["unshare", "-rm", "true"], capture_output=True, check=False)
    if probe.returncode != 0:
        pytest.skip("unprivileged user and mount namespaces are unavailable")
    for name in ("slot", "state", "control", "outside"):
        (tmp_path / name).mkdir()
    script = (
        "import sys\n"
        "from pathlib import Path\n"
        "from wrkslots import sandbox\n"
        f"base = Path({str(tmp_path)!r})\n"
        "view = sandbox.SlotView('pytest-probe', 'agent', base / 'slot', base / 'state', (), base / 'control', 'worktree')\n"
        "sandbox.run(view, sandbox.SandboxSettings(home='none', tasks_max=64), sys.argv[1:])\n"
    )
    command = (
        "touch $WRKSLOTS_SLOT_PATH/in && echo slot-ok; "
        f"touch {tmp_path}/outside/x 2>/dev/null && echo ESCAPED || echo outside-readonly; "
        "touch /tmp/t && echo tmp-ok"
    )
    result = subprocess.run(
        [sys.executable, "-c", script, "bash", "-c", command],
        capture_output=True,
        text=True,
        timeout=120,
        env={**os.environ, "PYTHONPATH": str(Path(__file__).resolve().parents[2])},
        check=False,
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout.split() == ["slot-ok", "outside-readonly", "tmp-ok"], result.stdout
    assert (tmp_path / "slot" / "in").exists()
    assert (tmp_path / "state" / "tmp" / "t").exists()
    subprocess.run(
        ["systemctl", "--user", "stop", sandbox.slice_name("agent", "pytest-probe")],
        capture_output=True,
        check=False,
    )
