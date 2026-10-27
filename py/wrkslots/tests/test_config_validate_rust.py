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
from wrkslots import cli as wrkslots_cli
from wrkslots import sandbox as sandbox_module


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
        "image": dataclasses.asdict(config.image_settings),
        "layout": config.layout,
        "liveness_batch_command": (
            None if config.liveness_batch_command is None else str(config.liveness_batch_command)
        ),
        "liveness_command": str(config.liveness_command),
        "machine": config.machine,
        "max_active_slots": config.max_active_slots,
        "post_provision_hooks": list(config.post_provision_hooks),
        "repo_cache_globs": [[name, list(globs)] for name, globs in config.repo_cache_globs],
        "root": str(config.root),
        "salvage_push_remotes": list(config.salvage_push_remotes),
        "sandbox": sandbox_module.settings_to_obj(config.sandbox_settings),
        "slot_representation": config.slot_representation,
        "worktrees": str(config.worktrees),
    }


def _rust(root: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["cargo", "run", "--quiet", "--release", "--manifest-path", "rs/Cargo.toml",
         "-p", "wrkslotsd", "--", "validate-config", "--root", str(root)],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
    )


def test_validate_config_matches_python(tmp_path: Path) -> None:
    root = _root(tmp_path, "max_active_slots: 3\ncache_globs: [target/**, 'node_modules/**']\n")
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == _expected(root)


def test_validate_config_with_sections_matches_python(tmp_path: Path) -> None:
    root = _root(
        tmp_path,
        "image: {backend: kernel, ceiling_bytes: 2147483648}\n"
        "sandbox: {isolation: cgroup, limits: {memory_max: 32G, tasks_max: 10}}\n",
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
