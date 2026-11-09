"""Differential: wrkslotsd validate-config against `_load_config`.

#562 rust-primary-wrkslots slice 2. Fixture roots are built on disk; the
Python loader's Config is serialized in the same shape the Rust command
prints, and both must agree, including refusal messages on bad fixtures.
"""

from __future__ import annotations

import dataclasses
import json
import subprocess
from pathlib import Path

import pytest
from wrkslots import __version__ as wrkslots_version
from wrkslots import cli as wrkslots_cli
from wrkslots import sandbox as sandbox_module

from wrkslots.tests.rust_binary import rust_wrkslotsd


AU_ROOT = Path(__file__).resolve().parents[3]


def _root(tmp_path: Path, extra: str = "") -> Path:
    root = tmp_path / "project"
    (root / "worktrees").mkdir(parents=True)
    (root / "live.py").write_text("# probe\n")
    (root / ".wrkslots.yml").write_text(
        "schema: 2\n"
        "machine: node-a\n"
        "worktrees_dir: worktrees\n"
        "default_remote: origin\n"
        "default_landed_ref: refs/heads/main\n"
        "heartbeat_ttl_seconds: 60\n"
        "liveness_command: live.py\n"
        + extra
    )
    return root


def _expected(root: Path) -> dict[str, object]:
    config = wrkslots_cli._load_config(str(root), None)
    return {
        "cache_globs": list(config.cache_globs),
        "config_path": str(config.config_path),
        "control": str(config.control),
        "default_landed_ref": config.default_landed_ref,
        "default_remote": config.default_remote,
        "disk_advisory_bytes": config.disk_advisory_bytes,
        "disk_emergency_bytes": config.disk_emergency_bytes,
        "disk_provisioning_floor_bytes": config.disk_provisioning_floor_bytes,
        "heartbeat_ttl_seconds": config.heartbeat_ttl_seconds,
        "identity": (
            None if config.identity is None else dataclasses.asdict(config.identity)
        ),
        "image": dataclasses.asdict(config.image_settings),
        "layout": config.layout,
        "liveness_batch_command": (
            None if config.liveness_batch_command is None else str(config.liveness_batch_command)
        ),
        "liveness_command": str(config.liveness_command),
        "machine": config.machine,
        "max_active_slots": config.max_active_slots,
        "post_provision_hooks": list(config.post_provision_hooks),
        "post_provision_hook_slot_types": [
            None if types is None else list(types)
            for types in config.post_provision_hook_slot_types
        ],
        "reclaimable_ignored_paths": list(config.reclaimable_ignored_paths),
        "repo_cache_globs": [[name, list(globs)] for name, globs in config.repo_cache_globs],
        "retire_freeze": config.retire_freeze,
        "root": str(config.root),
        "salvage_push_remotes": list(config.salvage_push_remotes),
        "sandbox": sandbox_module.settings_to_obj(config.sandbox_settings),
        "slot_representation": config.slot_representation,
        "worktrees": str(config.worktrees),
        "worktrees_image": (
            None
            if config.worktrees_image is None
            else {
                "fs": config.worktrees_image.filesystem,
                "mount": config.worktrees_image.mount,
                "mount_at": config.worktrees_image.mount_at,
                "path": config.worktrees_image.relative_path,
                "quota_mode": config.worktrees_image.quota_mode,
                "size_bytes": config.worktrees_image.size_bytes,
                "slot_quota_bytes": config.worktrees_image.slot_quota_bytes,
            }
        ),
    }


def _rust(root: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(rust_wrkslotsd()), "validate-config", "--root", str(root)],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
    )


def test_validate_config_matches_python(tmp_path: Path) -> None:
    root = _root(
        tmp_path,
        "max_active_slots: 3\ncache_globs: [target/**, 'node_modules/**']\n"
        "reclaimable_ignored_paths: [ignored/split/target, 'ignored/**/out']\n",
    )
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == _expected(root)


def test_validate_config_with_sections_matches_python(tmp_path: Path) -> None:
    root = _root(
        tmp_path,
        "identity: {name: Slot Bot, email: slot-bot@example.invalid}\n"
        "image: {backend: kernel, ceiling_bytes: 2147483648}\n"
        "sandbox: {isolation: cgroup, limits: {memory_max: 32G, tasks_max: 10}}\n"
        "post_provision_hooks: ['true', {command: 'true', slot_types: [validate, agent]}, {command: 'true'}]\n",
    )
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == _expected(root)


def test_validate_config_worktrees_image_matches_python(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # The first exercise of the non-null worktrees_image shape: the Rust
    # reader must accept mount_at and quota_mode (#1162) and print the
    # same resolved settings Python does. Python's loader would try to
    # mount the (missing) image on load; neutralise that side effect the
    # way the worktrees-image tests do.
    from wrkslots import worktreesimage

    monkeypatch.setattr(worktreesimage, "ensure_image_mounted", lambda *a: None)
    monkeypatch.setattr(worktreesimage, "ensure_binds", lambda *a: [])
    monkeypatch.setattr(
        worktreesimage, "current_state", lambda *a: worktreesimage.NOT_MOUNTED
    )
    root = _root(
        tmp_path,
        "worktrees_image: {path: wt.img, size_bytes: 2147483648, "
        "slot_quota_bytes: 67108864, mount_at: worktrees_dir, quota_mode: full}\n",
    )
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == _expected(root)


@pytest.mark.parametrize(
    ("extra", "fragment"),
    [
        ("unknown_key: 1\n", "invalid fields"),
        ("machine: 'bad name'\n", "invalid machine"),
        ("default_landed_ref: main\n", "invalid full default landed ref"),
        ("disk_advisory_bytes: 5\n", "must supply advisory"),
        ("sandbox: {isolation: namespace}\n", "namespace was renamed userns"),
        ("reclaimable_ignored_paths: ['../out']\n", "configuration.reclaimable_ignored_paths: cache glob must stay inside"),
        ("identity: Slot Bot\n", "configuration.identity must be a JSON object"),
        ("identity: {name: Slot Bot}\n", "configuration.identity has invalid fields: missing email"),
        ("identity: {name: Slot Bot, email: x@y, key: 1}\n", "configuration.identity has invalid fields: unknown key"),
        ("identity: {name: 'Bot <b>', email: x@y}\n", "configuration.identity.name must be"),
        ("identity: {name: ' Bot', email: x@y}\n", "configuration.identity.name must be"),
        ("identity: {name: Bot, email: 'a@b@c'}\n", "configuration.identity.email must be"),
        ("identity: {name: Bot, email: 'a b@c'}\n", "configuration.identity.email must be"),
        ("identity: {name: Bot, email: 7}\n", "configuration.identity.email must be a string"),
        (
            "post_provision_hooks: [{command: 'true', slot_types: [agnet]}]\n",
            "unknown slot type 'agnet'",
        ),
        (
            "post_provision_hooks: [{command: 'true', slot_types: []}]\n",
            "must name at least one slot type",
        ),
        (
            "post_provision_hooks: [{command: 'true', slot_types: [agent, agent]}]\n",
            "repeats 'agent'",
        ),
        (
            "post_provision_hooks: [{command: 'true', when: [agent]}]\n",
            "configuration.post_provision_hooks[0] has invalid fields: unknown when",
        ),
        (
            "post_provision_hooks: [{slot_types: [agent]}]\n",
            "configuration.post_provision_hooks[0] has invalid fields: missing command",
        ),
        (
            "post_provision_hooks: [7]\n",
            "configuration.post_provision_hooks[0] must be a JSON object",
        ),
        (
            "post_provision_hooks: [{command: 'true', slot_types: agent}]\n",
            "configuration.post_provision_hooks[0].slot_types must be a JSON array",
        ),
        (
            "post_provision_hooks: [{command: 'true', slot_types: [7]}]\n",
            "configuration.post_provision_hooks[0].slot_types[0] must be a string",
        ),
        (
            "post_provision_hooks: [{command: 'true', slot_types: null}]\n",
            "configuration.post_provision_hooks[0].slot_types must be a JSON array",
        ),
        ("post_provision_hooks: 'true'\n", "configuration.post_provision_hooks must be a JSON array"),
    ],
)
def test_validate_config_refusals_name_the_same_problem(tmp_path: Path, extra: str, fragment: str) -> None:
    root = _root(tmp_path)
    text = (root / ".wrkslots.yml").read_text()
    # Replace or append the offending key.
    lines = [line for line in text.splitlines() if not line.startswith(extra.split(":")[0] + ":")]
    (root / ".wrkslots.yml").write_text("\n".join(lines) + "\n" + extra)
    with pytest.raises(Exception) as caught:
        wrkslots_cli._load_config(str(root), None)
    assert fragment in str(caught.value)
    result = _rust(root)
    assert result.returncode == 1
    assert fragment in result.stderr


# #1109 wrkslots-config-forward-compat: ignorable_keys and min_wrkslots_version.


@pytest.fixture
def fresh_ignore_warnings(monkeypatch: pytest.MonkeyPatch) -> None:
    """Each test sees the one-per-process warning for every key it ignores."""

    monkeypatch.setattr(wrkslots_cli, "_WARNED_IGNORED_CONFIG_KEYS", set())


def _ignore_warning(root: Path, key: str) -> str:
    return (
        f"WARNING: wrkslots {wrkslots_version} ignores configuration key {key} in "
        f"{root / '.wrkslots.yml'}: it does not know the key, and ignorable_keys lists it"
    )


@pytest.mark.usefixtures("fresh_ignore_warnings")
def test_a_listed_unknown_key_is_ignored_with_a_warning_by_both(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _root(
        tmp_path,
        "ignorable_keys: [future_key, max_active_slots]\n"
        "future_key: {anything: [1, 2]}\n"
        "max_active_slots: 4\n"
        "min_wrkslots_version: 0.7.0\n",
    )
    expected = _expected(root)
    assert expected["max_active_slots"] == 4
    assert capsys.readouterr().err.splitlines() == [_ignore_warning(root, "future_key")]
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == expected
    assert result.stderr.splitlines() == [_ignore_warning(root, "future_key")]


@pytest.mark.usefixtures("fresh_ignore_warnings")
def test_an_older_reader_ignores_only_the_listed_key_it_does_not_know(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    """A reader from before ``identity`` (#1096), modelled by removing the key it knows."""

    monkeypatch.setattr(
        wrkslots_cli, "OPTIONAL_CONFIG_KEYS", wrkslots_cli.OPTIONAL_CONFIG_KEYS - {"identity"}
    )
    identity = "identity: {name: Slot Bot, email: slot-bot@example.invalid}\n"
    listed = _root(tmp_path / "listed", "ignorable_keys: [identity]\n" + identity)
    config = wrkslots_cli._load_config(str(listed), None)
    assert config.identity is None
    assert capsys.readouterr().err.splitlines() == [_ignore_warning(listed, "identity")]
    # The same file read by this build, which knows the key, uses it silently.
    monkeypatch.undo()
    monkeypatch.setattr(wrkslots_cli, "_WARNED_IGNORED_CONFIG_KEYS", set())
    assert wrkslots_cli._load_config(str(listed), None).identity is not None
    assert capsys.readouterr().err == ""

    monkeypatch.setattr(
        wrkslots_cli, "OPTIONAL_CONFIG_KEYS", wrkslots_cli.OPTIONAL_CONFIG_KEYS - {"identity"}
    )
    unlisted = _root(tmp_path / "unlisted", identity)
    with pytest.raises(wrkslots_cli.StateError, match="invalid fields: unknown identity"):
        wrkslots_cli._load_config(str(unlisted), None)


@pytest.mark.usefixtures("fresh_ignore_warnings")
@pytest.mark.parametrize(
    ("extra", "fragment"),
    [
        # A typo is never ignored, whatever ignorable_keys lists.
        (
            "ignorable_keys: [future_key]\nmax_active_slot: 3\n",
            "configuration has invalid fields: unknown max_active_slot",
        ),
        ("future_key: 1\n", "configuration has invalid fields: unknown future_key"),
        ("ignorable_keys: future_key\n", "configuration.ignorable_keys must be a list of key names"),
        ("ignorable_keys: [7]\n", "configuration.ignorable_keys must be a list of key names"),
        (
            "ignorable_keys: [Future-Key]\n",
            "configuration.ignorable_keys: invalid key name 'Future-Key'",
        ),
        (
            "ignorable_keys: [future_key, future_key]\n",
            "configuration.ignorable_keys must not contain duplicates",
        ),
        (
            "min_wrkslots_version: '1.0'\n",
            "configuration.min_wrkslots_version must be a version MAJOR.MINOR.PATCH",
        ),
        (
            "min_wrkslots_version: 01.0.0\n",
            "configuration.min_wrkslots_version must be a version MAJOR.MINOR.PATCH",
        ),
        ("min_wrkslots_version: 1\n", "configuration.min_wrkslots_version must be a string"),
    ],
)
def test_forward_compat_refusals_name_the_same_problem(
    tmp_path: Path, extra: str, fragment: str
) -> None:
    root = _root(tmp_path, extra)
    with pytest.raises(wrkslots_cli.StateError) as caught:
        wrkslots_cli._load_config(str(root), None)
    assert fragment in str(caught.value)
    result = _rust(root)
    assert result.returncode == 1
    assert fragment in result.stderr


@pytest.mark.usefixtures("fresh_ignore_warnings")
def test_a_newer_min_wrkslots_version_refuses_naming_both_versions(tmp_path: Path) -> None:
    """Checked before the keys, so an unknown key does not hide the version."""

    root = _root(tmp_path, "min_wrkslots_version: 99.0.0\nfuture_key: 1\n")
    message = (
        f"configuration {root / '.wrkslots.yml'} requires wrkslots 99.0.0 or later "
        f"(min_wrkslots_version); this is wrkslots {wrkslots_version}. Run a newer "
        "wrkslots for this project"
    )
    with pytest.raises(wrkslots_cli.StateError) as caught:
        wrkslots_cli._load_config(str(root), None)
    assert str(caught.value) == message
    result = _rust(root)
    assert result.returncode == 1
    # The Rust edition names the same version as the Python package.
    assert message in result.stderr, result.stderr
