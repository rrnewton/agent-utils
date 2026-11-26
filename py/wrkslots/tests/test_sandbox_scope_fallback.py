"""`wrkslots run`'s systemd-run fallback hands the boxed command over unexpanded (#2316).

When the user manager's D-Bus API cannot place the process in a scope, `run` execs
``systemd-run --scope -- COMMAND``. Newer systemd-run expands ``${NAME}``, a whole-word ``$NAME``
and ``$$`` in that command from the caller's environment, so the fallback passes
``--expand-environment=no`` where systemd-run accepts it, and keeps its old argv, with one note,
where it does not (systemd before 254, whose scopes never expanded).
"""

from __future__ import annotations

import os
import shutil
import subprocess
from pathlib import Path

import pytest

from wrkslots import sandbox

#: Arguments systemd-run would rewrite if it expanded the environment.
PAYLOAD = ["A:${PROBE}", "B:$$", "C:${x%% *}", "$PROBE", "D:$PROBE"]


class _Exec(Exception):
    """Raised in place of an exec so the test can read what would have run."""

    def __init__(self, argv: list[str], env: dict[str, str]) -> None:
        super().__init__(argv)
        self.argv = argv
        self.env = env


def _fallback_argv(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, supported: bool | None
) -> tuple[list[str], dict[str, str]]:
    """The argv and environment `run` would exec on the fallback path, for a cgroup box."""

    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(sandbox, "place_in_scope", lambda unit, slice_unit: False)
    monkeypatch.setattr(sandbox, "apply_slice_limits", lambda view, limits, **kwargs: [])
    monkeypatch.setattr(sandbox, "_NO_EXPAND_SUPPORTED", supported)

    def fake_exec(file: str, argv: list[str], env: dict[str, str]) -> None:
        raise _Exec(list(argv), dict(env))

    monkeypatch.setattr(os, "execvpe", fake_exec)
    view = sandbox.SlotView(
        slot="slot-2316",
        slot_type="agent",
        slot_path=tmp_path,
        state_directory=tmp_path / "state",
        git_directories=(),
        control_directory=tmp_path / "control",
        representation="worktrees",
    )
    settings = sandbox.settings_from_obj({"isolation": "cgroup"})
    with pytest.raises(_Exec) as raised:
        sandbox.run(view, settings, ["printf", "%s\\n", *PAYLOAD])
    return raised.value.argv, raised.value.env


def test_fallback_turns_expansion_off_before_the_separator(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    argv, _env = _fallback_argv(monkeypatch, tmp_path, True)
    assert argv[:3] == ["systemd-run", "--user", "--scope"]
    separator = argv.index("--")
    assert argv[separator - 1] == "--expand-environment=no", argv
    assert argv[separator + 1 :] == ["printf", "%s\\n", *PAYLOAD]


def test_fallback_keeps_the_old_argv_when_systemd_run_refuses_the_option(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    argv, _env = _fallback_argv(monkeypatch, tmp_path, False)
    assert not any(arg.startswith("--expand-environment") for arg in argv), argv
    assert argv[argv.index("--") + 1 :] == ["printf", "%s\\n", *PAYLOAD]


@pytest.mark.parametrize(
    ("script", "supported", "noted"),
    [
        pytest.param(
            '[ "$1" = --expand-environment=no ] && [ "$2" = --version ] && exit 0\nexit 9\n',
            True,
            False,
            id="accepts",
        ),
        pytest.param(
            "echo \"systemd-run: unrecognized option '$1'\" >&2\nexit 1\n",
            False,
            True,
            id="refuses",
        ),
    ],
)
def test_probe_asks_systemd_run_once_and_notes_a_refusal(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
    script: str,
    supported: bool,
    noted: bool,
) -> None:
    calls = tmp_path / "calls"
    fake = tmp_path / "systemd-run"
    fake.write_text(f"#!/bin/sh\necho call >> '{calls}'\n{script}")
    fake.chmod(0o755)
    monkeypatch.setenv("PATH", f"{tmp_path}{os.pathsep}{os.environ['PATH']}")
    monkeypatch.setattr(sandbox, "_NO_EXPAND_SUPPORTED", None)
    expected = ["--expand-environment=no"] if supported else []
    assert sandbox.systemd_run_no_expand_args() == expected
    assert sandbox.systemd_run_no_expand_args() == expected
    assert calls.read_text().splitlines() == ["call"]
    err = capsys.readouterr().err
    assert ("refuses --expand-environment=no" in err and "#2316" in err) is noted, err


def test_live_fallback_delivers_dollar_text_byte_for_byte(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    """The exact argv the fallback execs, run through the real systemd-run with PROBE set in its
    environment, prints the payload unchanged. Skipped without a usable user scope, and below an
    outer validation run, whose cgroup a fresh top-level scope would leave."""

    outer = ("DAGRUN_DELEGATED_CGROUP", "DAGRUN_DELEGATED_UNBOXED", "DAGRUN_IN_SCOPE",
             "DAGRUN_OUTER_RUN")
    if any(key in os.environ for key in outer):
        pytest.skip("below an outer validation run (#2316 live probe)")
    if shutil.which("systemd-run") is None:
        pytest.skip("no systemd-run here")
    probe = subprocess.run(
        ["systemd-run", "--user", "--scope", "--quiet", "--collect", "true"],
        capture_output=True,
        check=False,
        timeout=60,
    )
    if probe.returncode != 0:
        pytest.skip(f"no usable systemd user scope: {probe.stderr!r}")
    argv, env = _fallback_argv(monkeypatch, tmp_path, None)
    env = {key: value for key, value in env.items() if key != "x"}
    env["PROBE"] = "expanded"
    proc = subprocess.run(
        argv, env=env, capture_output=True, text=True, check=False, timeout=60,
        stdin=subprocess.DEVNULL,
    )
    assert proc.returncode == 0, proc.stderr
    assert proc.stdout.splitlines() == PAYLOAD, proc.stderr
