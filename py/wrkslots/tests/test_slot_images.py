"""Tests for the disk-image slot representation and the slot sandbox.

Pure tests always run. Tests that need a real mount (``sudo -n`` or FUSE) or a
systemd user manager skip with the reason when the host cannot provide it; the
end-to-end lifecycle exercise is ``wrkslots/tests/e2e_images.sh``.
"""

from __future__ import annotations

import argparse
import dataclasses
import json
import os
import re
import shutil
import subprocess
import sys
from collections.abc import Iterator
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
    settings = sandbox.settings_from_obj(
        {
            "isolation": "root",
            "home": "hidden",
            "home_expose": ["bin"],
            "read_write": ["/var/x/$USER", "~/scratch"],
            "env": {"A_B": "~/x"},
            "limits": {"memory_max": "32G"},
        }
    )
    assert settings.isolation == "root" and settings.home == "hidden"
    assert settings.home_expose == ("bin",) and settings.limits.memory_max == "32G"
    assert settings.limits.tasks_max == sandbox.DEFAULT_TASKS_MAX
    for bad, fragment in (
        ({"home": "rw"}, "home must be"),
        ({"home": "none"}, "hidden with home_expose"),
        ({"isolation": "namespace"}, "renamed userns"),
        ({"home_expose": ["../escape"]}, "without '..'"),
        ({"home_shared": ["/abs"]}, "relative"),
        ({"home_private_files": [".config/x"]}, "top-level"),
        ({"outputs": ["."]}, "relative"),
        ({"read_write": ["relative/path"]}, "absolute"),
        ({"env": {"1BAD": "x"}}, "invalid variable name"),
        ({"env": ["A=1"]}, "object"),
        ({"tmp_size": "lots"}, "tmp_size"),
        ({"limits": {"memory_max": "lots"}}, "32G"),
        ({"limits": {"tasks_max": 0}}, "positive"),
        ({"limits": {"surprise": 1}}, "unknown keys: surprise"),
        ({"memory_max": "32G"}, "now limits.memory_max"),
        ({"home_writable": [".cache"]}, "now home_private"),
        ({"unknown": 1}, "unknown keys: unknown"),
    ):
        with pytest.raises(sandbox.SandboxError, match=re.escape(fragment)):
            sandbox.settings_from_obj(bad)


def test_default_sandbox_section_spells_out_every_key() -> None:
    section = sandbox.default_config_obj()
    assert list(section) == list(sandbox.SETTING_KEYS)
    assert section["limits"] == {key: getattr(sandbox.SandboxLimits(), key) for key in sandbox.LIMIT_KEYS}
    assert sandbox.settings_from_obj(section) == sandbox.SandboxSettings()
    assert sandbox.settings_from_obj({}) == sandbox.SandboxSettings()
    assert section["isolation"] == "userns" and section["home"] == "ro"
    shared = section["home_shared"]
    assert isinstance(shared, list) and ".local/share/muse" in shared
    assert section["home_private_files"] == [".claude.json"]
    assert section["outputs"] == ["ai_docs", "experiments"]
    assert section["env"] == {} and section["tmp_size"] == "16G"
    json.dumps(section)


def test_merge_defaults_adds_only_missing_keys() -> None:
    existing: dict[str, object] = {"home": "hidden", "env": {"A": "1"}, "limits": {"memory_max": "8G"}}
    merged, added = sandbox.merge_defaults(existing)
    assert merged["home"] == "hidden" and merged["env"] == {"A": "1"}
    default_limits = sandbox.default_config_obj()["limits"]
    assert isinstance(default_limits, dict)
    assert merged["limits"] == {**default_limits, "memory_max": "8G"}
    assert "home" not in added and "env" not in added and "limits.memory_max" not in added
    assert "isolation" in added and "limits.cpu_quota" in added
    again, added_again = sandbox.merge_defaults(merged)
    assert again == merged and added_again == []
    with pytest.raises(sandbox.SandboxError):
        sandbox.merge_defaults({"legacy": True})


def test_init_writes_the_full_sandbox_section_only_for_new_projects(tmp_path: Path) -> None:
    config = tmp_path / ".wrkslots.yml"
    assert cli._init_sandbox_section(config) == sandbox.default_config_obj()
    config.write_text(json.dumps({"schema": 2}), encoding="utf-8")
    assert cli._init_sandbox_section(config) is None
    config.write_text(json.dumps({"sandbox": {"home": "hidden"}}), encoding="utf-8")
    assert cli._init_sandbox_section(config) == {"home": "hidden"}
    payload = cli._config_payload(
        "worktrees/slots", "m", "origin", "refs/remotes/origin/main", 60, "t.py",
        sandbox_section=sandbox.default_config_obj(),
    )
    assert payload["sandbox"] == sandbox.default_config_obj()


def test_expand_path_knows_only_home_and_user() -> None:
    home, user = "/h/u", "u"
    assert sandbox.expand_path("~", home, user) == "/h/u"
    assert sandbox.expand_path("~/x", home, user) == "/h/u/x"
    assert sandbox.expand_path("/var/c/$USER/x", home, user) == "/var/c/u/x"
    assert sandbox.expand_path("/var/c/${USER}/x", home, user) == "/var/c/u/x"
    assert sandbox.expand_path("$HOME/y", home, user) == "/h/u/y"
    assert sandbox.expand_path("/var/$USERNAME", home, user) == "/var/$USERNAME"
    assert sandbox.expand_path("/a/~b", home, user) == "/a/~b"


def _fake_home(base: Path) -> Path:
    home = base / "home"
    for directory in (".config/gh", ".config/muse", ".local/bin", ".ssh", ".cache", "bin", "work"):
        (home / directory).mkdir(parents=True)
    (home / ".config/gh/hosts.yml").write_text("token: secret\n", encoding="utf-8")
    (home / ".config/gh/config.yml").write_text("editor: vi\n", encoding="utf-8")
    (home / ".local/bin/tool").write_text("#!/bin/sh\necho tool\n", encoding="utf-8")
    (home / ".local/bin/tool").chmod(0o755)
    (home / ".ssh/id_x").write_text("private\n", encoding="utf-8")
    (home / ".netrc").write_text("machine x password y\n", encoding="utf-8")
    (home / ".claude.json").write_text('{"seeded": true}\n', encoding="utf-8")
    (home / ".profile").write_text("# profile\n", encoding="utf-8")
    (home / "link").symlink_to("work")
    return home


def _view(base: Path) -> sandbox.SlotView:
    return sandbox.SlotView(
        slot="s1",
        slot_type="agent",
        slot_path=base / "slot",
        state_directory=base / "state",
        git_directories=(base / "repo.git",),
        control_directory=base / "control",
        representation="worktree",
        project_root=base / "project",
    )


def test_build_spec_layers_home_and_binds_blessed_paths(tmp_path: Path) -> None:
    home = _fake_home(tmp_path)
    for directory in ("slot", "repo.git", "control", "project/ai_docs"):
        (tmp_path / directory).mkdir(parents=True)
    view = _view(tmp_path)
    spec = sandbox.build_spec(view, sandbox.SandboxSettings(), home, tmp_path / "slot")
    binds = spec["binds"]
    assert isinstance(binds, list)
    targets = [target for _source, target in binds]
    # Shared paths are bound onto themselves, nested ones included; missing ones are skipped.
    assert [str(home / ".config/muse")] * 2 in binds
    assert str(home / ".claude") not in targets
    for path in ("slot", "repo.git", "control", "project/ai_docs"):
        assert str(tmp_path / path) in targets
    assert str(tmp_path / "project/experiments") not in targets  # missing output: skipped
    # Every real top-level entry is bound read-only over the layer, .config and
    # .local included; private directories and private files are not.
    home_binds = spec["home_binds"]
    assert isinstance(home_binds, list)
    assert {".config", ".local", ".ssh", "bin", "work", ".netrc", ".profile"} <= set(home_binds)
    assert ".cache" not in home_binds and ".claude.json" not in home_binds and "link" not in home_binds
    masks = spec["masks"]
    assert isinstance(masks, list)
    assert str(home / ".ssh") in masks and str(home / ".config/gh/hosts.yml") in masks
    assert spec["home_layer"] == str(tmp_path / "state" / "home")
    hidden = sandbox.build_spec(view, sandbox.SandboxSettings(home="hidden"), home, tmp_path / "slot")
    assert hidden["home_binds"] == ["bin", ".local/bin"]


def test_prepare_state_builds_the_layer_without_touching_home(tmp_path: Path) -> None:
    home = _fake_home(tmp_path)
    before = sorted(str(path.relative_to(home)) for path in home.rglob("*"))
    view = _view(tmp_path)
    sandbox.prepare_state(view, sandbox.SandboxSettings(), home)
    layer = tmp_path / "state" / "home"
    assert (layer / ".config").is_dir() and not any((layer / ".config").iterdir())
    assert (layer / ".profile").is_file() and (layer / ".profile").stat().st_size == 0
    assert os.readlink(layer / "link") == "work"
    assert json.loads((layer / ".claude.json").read_text(encoding="utf-8")) == {"seeded": True}
    assert (layer / ".cache").is_dir() and (layer / ".buck").is_dir()
    (layer / ".claude.json").write_text("{}", encoding="utf-8")
    sandbox.prepare_state(view, sandbox.SandboxSettings(), home)
    assert (layer / ".claude.json").read_text(encoding="utf-8") == "{}"  # seeded once
    # Switching to hidden removes the ro placeholders and links it created,
    # but never content the agent wrote.
    (layer / ".profile").write_text("agent content", encoding="utf-8")
    sandbox.prepare_state(view, sandbox.SandboxSettings(home="hidden"), home)
    assert not (layer / ".config" / "gh").exists() and (layer / ".config" / "muse").is_dir()
    assert not os.path.lexists(layer / "link") and not (layer / "work").exists()
    assert (layer / ".profile").read_text(encoding="utf-8") == "agent content"
    assert (layer / "bin").is_dir() and (layer / ".local" / "bin").is_dir()
    assert sorted(str(path.relative_to(home)) for path in home.rglob("*")) == before


def test_child_environment(tmp_path: Path) -> None:
    view = _view(tmp_path)
    environ = {"HOME": "/h/u", "PATH": "/bin", "TMPDIR": "/var/tmp/x", "INVOCATION_ID": "1", "TMP": "/t"}
    settings = sandbox.SandboxSettings(env=(("CONF", "~/.conf"),))
    boxed = sandbox.child_environment(view, settings, environ)
    assert boxed["TMPDIR"] == "/tmp" and "TMP" not in boxed and "INVOCATION_ID" not in boxed
    assert boxed["CONF"] == "/h/u/.conf" and boxed["WRKSLOTS_SANDBOX"] == "userns"
    assert boxed["WRKSLOTS_SLOT_REPRESENTATION"] == "worktree"
    limited = sandbox.child_environment(view, dataclasses.replace(settings, isolation="cgroup"), environ)
    assert limited["TMPDIR"] == "/var/tmp/x" and limited["TMP"] == "/t"


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
        assert sorted(os.listdir(image.state_mount)) == ["home"]
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


def _sandbox_host_or_skip(isolation: str) -> None:
    if shutil.which("systemd-run") is None or shutil.which("busctl") is None:
        pytest.skip("needs a systemd user manager")
    if subprocess.run(["systemctl", "--user", "show-environment"], capture_output=True, check=False).returncode:
        pytest.skip("the systemd user manager is not reachable")
    if isolation == "userns":
        if subprocess.run(["unshare", "-rm", "true"], capture_output=True, check=False).returncode:
            pytest.skip("unprivileged user and mount namespaces are unavailable")
    elif isolation == "root":
        if os.getuid() == 0 or shutil.which("sudo") is None:
            pytest.skip("isolation root needs a non-root user with sudo")
        if subprocess.run(["sudo", "-n", "true"], capture_output=True, check=False).returncode:
            pytest.skip("isolation root needs passwordless sudo")


@pytest.fixture
def box_base() -> Iterator[Path]:
    """A scratch directory OUTSIDE /tmp: the box replaces /tmp with a fresh tmpfs."""

    import tempfile

    root = Path("/var/tmp")
    if not root.is_dir() or not os.access(root, os.W_OK):
        pytest.skip("/var/tmp is not writable")
    base = Path(tempfile.mkdtemp(prefix="wrkslots-box-", dir=root))
    try:
        yield base
    finally:
        subprocess.run(["chmod", "-R", "u+w", str(base)], capture_output=True, check=False)
        shutil.rmtree(base, ignore_errors=True)


_PROBE = r"""
report() { if eval "$2" >/dev/null 2>&1; then echo "$1=yes"; else echo "$1=no"; fi; }
report tmpdir '[ "$TMPDIR" = /tmp ]'
report tmp_fresh '[ -z "$(ls -A /tmp)" ]'
report tmp_write 'echo x > /tmp/t'
report slot_write 'echo x > "$WRKSLOTS_SLOT_PATH/in"'
report output_write 'echo x > "$BASE/project/ai_docs/note"'
report project_write 'echo x > "$BASE/project/src/no"'
report outside_write 'echo x > "$BASE/outside/no"'
report home_new_file 'echo x > ~/new-top-level'
report home_work_write 'echo x > ~/work/no'
report config_read 'grep -q editor ~/.config/gh/config.yml'
report config_write 'echo x > ~/.config/no'
report shared_write 'echo x > ~/.config/muse/shared'
report local_bin_runs '[ "$(~/.local/bin/tool)" = tool ]'
report ssh_empty '[ -z "$(ls -A ~/.ssh)" ]'
report netrc_empty '[ ! -s ~/.netrc ]'
report gh_hosts_empty '[ ! -s ~/.config/gh/hosts.yml ]'
report cache_write 'echo x > ~/.cache/private'
report rename_top_level 'echo "{}" > ~/.claude.json.tmp.1 && mv ~/.claude.json.tmp.1 ~/.claude.json'
"""


def _probe_results(stdout: str) -> dict[str, bool]:
    results: dict[str, bool] = {}
    for line in stdout.splitlines():
        key, separator, value = line.partition("=")
        if separator and value in ("yes", "no"):
            results[key] = value == "yes"
    return results


_EXPECTED = {
    "tmpdir": True,
    "tmp_fresh": True,
    "tmp_write": True,
    "slot_write": True,
    "output_write": True,
    "project_write": False,
    "outside_write": False,
    "home_new_file": True,
    "home_work_write": False,
    "config_read": True,
    "config_write": False,
    "shared_write": True,
    "local_bin_runs": True,
    "ssh_empty": True,
    "netrc_empty": True,
    "gh_hosts_empty": True,
    "cache_write": True,
    "rename_top_level": True,
}


def _assert_home_untouched_and_layered(base: Path, home: Path, state: Path) -> None:
    assert (home / ".config/muse/shared").exists()  # shared: the real file
    assert not (home / "new-top-level").exists() and (state / "home/new-top-level").exists()
    assert not (home / ".cache/private").exists() and (state / "home/.cache/private").exists()
    assert json.loads((home / ".claude.json").read_text(encoding="utf-8")) == {"seeded": True}
    assert (state / "home/.claude.json").read_text(encoding="utf-8").strip() == "{}"
    assert not (state / "tmp").exists()  # /tmp is per launch
    assert not (base / "outside/no").exists() and not (base / "project/src/no").exists()


@pytest.mark.parametrize("isolation", ["userns", "root"])
def test_sandbox_view_confines_writes(box_base: Path, isolation: str) -> None:
    _sandbox_host_or_skip(isolation)
    home = _fake_home(box_base)
    for directory in ("slot", "state", "control", "outside", "project/ai_docs", "project/src"):
        (box_base / directory).mkdir(parents=True)
    slot = f"pytest-{isolation}-{os.getpid()}"
    script = (
        "import sys\n"
        "from pathlib import Path\n"
        "from wrkslots import sandbox\n"
        "base = Path(sys.argv[1])\n"
        f"view = sandbox.SlotView({slot!r}, 'agent', base / 'slot', base / 'state', (), base / 'control', 'worktree', base / 'project')\n"
        f"settings = sandbox.SandboxSettings(isolation={isolation!r}, tmp_size='64M', limits=sandbox.SandboxLimits(tasks_max=256))\n"
        "sandbox.run(view, settings, sys.argv[2:])\n"
    )
    environment = {
        **os.environ,
        "HOME": str(home),
        "BASE": str(box_base),
        "PYTHONPATH": str(Path(__file__).resolve().parents[2]),
    }
    try:
        result = subprocess.run(
            [sys.executable, "-c", script, str(box_base), "bash", "-c", _PROBE],
            capture_output=True,
            text=True,
            timeout=120,
            env=environment,
            check=False,
        )
    finally:
        subprocess.run(["systemctl", "--user", "stop", sandbox.slice_name("agent", slot)], capture_output=True, check=False)
    assert result.returncode == 0, result.stderr
    assert _probe_results(result.stdout) == _EXPECTED, result.stdout + result.stderr
    _assert_home_untouched_and_layered(box_base, home, box_base / "state")


def test_wrkslots_run_boxes_a_plain_worktree_slot(box_base: Path) -> None:
    """The box does not depend on the representation: a plain-worktree slot gets the same view."""

    _sandbox_host_or_skip("userns")
    home = _fake_home(box_base)
    (box_base / "outside").mkdir()
    remote, project = box_base / "remote.git", box_base / "project"
    repository = project / "src"
    git_env = {**os.environ, "GIT_CONFIG_NOSYSTEM": "1", "HOME": str(home)}

    def git(*arguments: str, cwd: Path | None = None) -> None:
        subprocess.run(["git", *arguments], cwd=cwd, check=True, capture_output=True, text=True, env=git_env)

    git("init", "--bare", "--initial-branch=main", str(remote))
    project.mkdir()
    git("clone", str(remote), str(repository))
    for key, value in (("user.name", "t"), ("user.email", "t@example.invalid")):
        git("config", key, value, cwd=repository)
    (repository / "seed").write_text("seed\n", encoding="utf-8")
    git("add", "seed", cwd=repository)
    git("commit", "-m", "seed", cwd=repository)
    git("push", "-u", "origin", "main", cwd=repository)
    (project / "ai_docs").mkdir()
    liveness = project / "liveness.py"
    liveness.write_text("#!/usr/bin/env python3\nraise SystemExit(1)\n", encoding="utf-8")
    liveness.chmod(0o755)
    environment = {
        **git_env,
        "BASE": str(box_base),
        "PYTHONPATH": str(Path(__file__).resolve().parents[2]),
        "WRKSLOTS_INIT_REPRESENTATION": "worktree",
    }

    def wrkslots(*arguments: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, "-m", "wrkslots", "--machine", "testhost", *arguments],
            cwd=project,
            capture_output=True,
            text=True,
            timeout=300,
            env=environment,
            check=False,
        )

    initialized = wrkslots("init", str(project), "--worktrees-dir", "worktrees", "--liveness-command", "liveness.py")
    assert initialized.returncode == 0, initialized.stderr
    configuration = json.loads((project / ".wrkslots.yml").read_text(encoding="utf-8"))
    assert configuration["sandbox"] == sandbox.default_config_obj()
    slot = f"box{os.getpid()}"
    created = wrkslots(
        "create", slot, "--slot-type", "agent", "--coordinator-authorized", "--agent", "a1", "--task", "t1", "--purpose", "box test",
        "--owner-pid", str(os.getpid()), "--coordinator-pid", str(os.getpid()),
        "--repo", "src=src", "--branch", "src=agent/box",
    )
    assert created.returncode == 0, created.stderr
    probe = _PROBE + r"""
report representation '[ "$WRKSLOTS_SLOT_REPRESENTATION" = worktree ]'
report checkout_commit 'cd "$WRKSLOTS_SLOT_PATH"/src && echo w > w && git add w && git -c user.name=t -c user.email=t@e commit -qm w'
"""
    try:
        boxed = wrkslots("run", slot, "--", "bash", "-c", probe.replace("$BASE/project/src/no", "$BASE/project/src/seed"))
    finally:
        subprocess.run(["systemctl", "--user", "stop", sandbox.slice_name("agent", slot)], capture_output=True, check=False)
    assert boxed.returncode == 0, boxed.stderr
    expected = {**_EXPECTED, "representation": True, "checkout_commit": True}
    assert _probe_results(boxed.stdout) == expected, boxed.stdout + boxed.stderr
    assert (repository / "seed").read_text(encoding="utf-8") == "seed\n"  # primary checkout unchanged
    state = project / "worktrees" / ".wrkslots" / "slot-state" / "agent" / slot
    if not state.exists():
        candidates = list(project.rglob(f"slot-state/agent/{slot}"))
        assert candidates, "worktree slot state directory not found"
        state = candidates[0]
    _assert_home_untouched_and_layered(box_base, home, state)
