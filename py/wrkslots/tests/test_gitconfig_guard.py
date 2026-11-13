"""The wrkslots suite runs under the global git config guard (#1474)."""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

from gitconfig_guard import fingerprint, real_gitconfig_path


def test_guard_redirect_is_active_for_this_session() -> None:
    sentinel = os.environ.get("GIT_CONFIG_GLOBAL")
    assert sentinel, "the conftest guard did not set GIT_CONFIG_GLOBAL"
    assert Path(sentinel).is_file()
    assert Path(sentinel).resolve() != real_gitconfig_path().resolve()
    assert os.environ.get("GIT_CONFIG_NOSYSTEM") == "1"


def test_global_write_lands_in_sentinel_not_real_file() -> None:
    real = real_gitconfig_path()
    before = fingerprint(real)
    subprocess.run(
        ["git", "config", "--global", "user.email", "guard-probe@example.invalid"],
        check=True,
        timeout=30,
    )
    assert fingerprint(real) == before
    sentinel = Path(os.environ["GIT_CONFIG_GLOBAL"])
    assert "guard-probe@example.invalid" in sentinel.read_text(encoding="utf-8")
