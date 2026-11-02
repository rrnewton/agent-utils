"""Per-host retire freeze (#780): frozen hosts refuse every removal."""

from __future__ import annotations

import argparse
import contextlib
import io
from pathlib import Path

import pytest
from wrkslots import cli as wrkslots_cli

from wrkslots.tests.test_create_text_rust import _fixture


def _freeze(root: Path, action: str, *reason: str) -> tuple[int, str]:
    args = argparse.Namespace(
        project_root=str(root), machine="node-a",
        freeze_action=action, reason=list(reason),
    )
    buffer = io.StringIO()
    with contextlib.redirect_stdout(buffer):
        rc = wrkslots_cli._cmd_freeze(args)
    return rc, buffer.getvalue()


def _refusal(root: Path, command: str) -> str:
    config = wrkslots_cli._load_config(str(root), "node-a")
    with pytest.raises(wrkslots_cli.Refusal) as caught:
        wrkslots_cli._assert_not_retire_frozen(config, command)
    return str(caught.value)


def test_freeze_marker_refuses_with_stable_code(tmp_path: Path) -> None:
    root = _fixture(tmp_path / "case")
    rc, out = _freeze(root, "status")
    assert (rc, out) == (0, "frozen=false\n")
    rc, out = _freeze(root, "set", "isolation", "migration")
    assert rc == 0 and "frozen=true reason=isolation migration source=marker" in out
    for command in ("remove", "retire-pending", "clean-caches"):
        message = _refusal(root, command)
        assert message.startswith("retire-freeze:")
        assert "isolation migration" in message
    rc, out = _freeze(root, "status")
    assert "frozen=true" in out
    rc, out = _freeze(root, "clear")
    assert out == "frozen=false\n"
    config = wrkslots_cli._load_config(str(root), "node-a")
    wrkslots_cli._assert_not_retire_frozen(config, "remove")


def test_freeze_config_key_refuses(tmp_path: Path) -> None:
    root = _fixture(tmp_path / "case")
    config_path = root / ".wrkslots.yml"
    config_path.write_text(config_path.read_text() + "retire_freeze: planned migration\n")
    message = _refusal(root, "remove")
    assert "retire-freeze:" in message and "planned migration" in message
    assert "source: config" in message
    rc, out = _freeze(root, "status")
    assert rc == 0 and "source=config" in out
