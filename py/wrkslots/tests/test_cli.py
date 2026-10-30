"""Installed command surface tests for wrkslots."""

from __future__ import annotations

import argparse
import dataclasses
import hashlib
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import time
from pathlib import Path

import pytest
from wrkslots import cli, imagecmd, sandbox, slotimage


PACKAGE_ROOT = Path(__file__).resolve().parents[1]
COMPATIBILITY_COMMAND = PACKAGE_ROOT.parent / "wrkslots.py"
GIB = 1024**3


def _run(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, "-m", "wrkslots", *args],
        cwd=PACKAGE_ROOT.parent,
        text=True,
        capture_output=True,
        check=False,
    )


def test_quickstart_teaches_the_normal_lifecycle() -> None:
    completed = _run("quickstart")

    assert completed.returncode == 0, completed.stderr
    for command in ("init", "create", "heartbeat", "finish", "remove", "recover"):
        assert f"wrkslots {command}" in completed.stdout
    assert "--remote product=upstream" in completed.stdout
    assert "--remote-url product=URL" in completed.stdout


def test_pre_packaging_repository_command_still_runs() -> None:
    completed = subprocess.run(
        [sys.executable, str(COMPATIBILITY_COMMAND), "--version"],
        cwd=PACKAGE_ROOT.parent,
        text=True,
        capture_output=True,
        check=False,
    )

    assert completed.returncode == 0, completed.stderr
    assert completed.stdout.strip() == "wrkslots 0.6.0"


def test_every_command_help_explains_effect_and_inputs() -> None:
    commands = (
        "init",
        "status",
        "create",
        "register",
        "import-existing",
        "adopt",
        "transfer-owner",
        "recover-unbound-owner",
        "relocate-repository",
        "heartbeat",
        "finish",
        "remove",
        "recover",
    )
    for command in commands:
        completed = _run(command, "--help")
        assert completed.returncode == 0, completed.stderr
        assert "options:" in completed.stdout
        assert len(completed.stdout.splitlines()) >= 8

    create = _run("create", "--help").stdout
    assert "linked Git worktree" in create
    assert "configured origin" in create
    assert "--remote NAME=REMOTE" in create
    assert "expected fetch URL" in create
    assert "--remote-url NAME=URL" in create
    normalized_create = " ".join(create.split())
    assert "another child of the invoking coordinator" in normalized_create
    assert "even when its assigned coordinator runs separately" in normalized_create
    assert "unless the owner is in this command's ancestry" in normalized_create
    assert "whose lifetime represents this one agent" in normalized_create
    assert "do not use a shared multiplexing supervisor" in normalized_create
    assert "or ../NAME[/PATH] path components inside one direct sibling" in normalized_create
    assert (
        "absolute paths, other parent traversal, and symlink components are refused"
        in normalized_create
    )

    imported = _run("import-existing", "--help").stdout
    assert "--purpose 'continue task-123'" in imported
    assert "--coordinator-pid PID" in imported
    assert "--slot-type agent --coordinator-authorized" in imported

    init = _run("init", "--help").stdout
    assert "called as PATH AGENT" in init
    assert "0 dead, 1 alive, or 2 unverifiable" in init


def test_userguide_is_the_packaged_document() -> None:
    completed = _run("--userguide")

    assert completed.returncode == 0, completed.stderr
    assert completed.stdout == (PACKAGE_ROOT / "USER_GUIDE.md").read_text(encoding="utf-8")


# ------------------------------------------------- per-create image ceilings


def _git(cwd: Path, *args: str) -> str:
    env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    env.update(
        {
            "LC_ALL": "C",
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": "/dev/null",
            "GIT_CONFIG_SYSTEM": "/dev/null",
        }
    )
    result = subprocess.run(
        [
            "git", "-c", "user.email=test@example.invalid", "-c", "user.name=Test",
            "-c", "commit.gpgsign=false", "-C", str(cwd), *args,
        ],
        capture_output=True, text=True, check=True, env=env, timeout=120,
    )
    return result.stdout.strip()


def _create_fixture(base: Path) -> Path:
    """A project with one upstream repository and no slots."""

    root = base / "project"
    (root / "worktrees").mkdir(parents=True)
    (root / "live.py").write_text("# probe\n")
    (root / ".wrkslots.yml").write_text(
        "schema: 2\n"
        "machine: node-a\n"
        "worktrees_dir: worktrees\n"
        "default_remote: origin\n"
        "default_landed_ref: refs/remotes/origin/main\n"
        "heartbeat_ttl_seconds: 60\n"
        "liveness_command: live.py\n"
    )
    upstream = base / "upstream-repo"
    upstream.mkdir()
    _git(upstream, "init", "-q", "-b", "main")
    (upstream / "file.txt").write_text("one\n")
    _git(upstream, "add", "file.txt")
    _git(upstream, "commit", "-q", "-m", "one")
    _git(upstream, "remote", "add", "origin", str(upstream))
    _git(upstream, "fetch", "-q", "origin", "+refs/heads/*:refs/remotes/origin/*")
    return root


def _create_args(root: Path, **overrides: object) -> argparse.Namespace:
    """The namespace `create` parses, without the per-create ceiling options."""

    values: dict[str, object] = dict(
        project_root=str(root), machine="node-a", slot="slot-b", slot_type="agent",
        coordinator_authorized=True, agent="agent-new", task="task-1",
        purpose="testing", owner_pid=os.getpid(), coordinator_pid=os.getpid(),
        task_scope_unit=None, task_scope_invocation_id=None,
        repo=["repo=../upstream-repo"], remote=[], remote_url=[],
        branch=["repo=suite/new"], start=[], override_disk_floor=False,
        representation="config", format="human", wait_lock=0.0,
    )
    values.update(overrides)
    return argparse.Namespace(**values)


def _record_provisioning(monkeypatch: pytest.MonkeyPatch) -> list[slotimage.ImageSettings]:
    """Replace image provisioning (sudo or FUSE) by a recorder that only makes the mount point."""

    seen: list[slotimage.ImageSettings] = []

    def provision(
        control: Path, slot_type: str, slot: str, location: Path, settings: slotimage.ImageSettings
    ) -> slotimage.SlotImage:
        seen.append(settings)
        location.mkdir(mode=0o755)
        return slotimage.SlotImage(
            directory=slotimage.image_directory(control, slot_type, slot),
            slot=slot,
            slot_type=slot_type,
            location=location,
            ceiling_bytes=settings.ceiling_bytes,
            state_ceiling_bytes=settings.state_ceiling_bytes,
            backend="kernel",
            created_at="2026-10-30T00:00:00Z",
            phase="ready",
        )

    monkeypatch.setattr(slotimage, "provision", provision)
    return seen


def test_create_help_documents_both_per_create_ceilings() -> None:
    create = " ".join(_run("create", "--help").stdout.split())

    assert "--image-ceiling-gib GIB" in create
    assert "--image-state-ceiling-gib GIB" in create
    assert "configuration.image.state_ceiling_bytes" in create


def test_create_threads_per_create_ceilings_into_provisioning(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    seen = _record_provisioning(monkeypatch)
    root = _create_fixture(tmp_path)
    configured = cli._load_config(str(root), "node-a")

    assert cli._cmd_create(
        _create_args(
            root, representation="image", image_ceiling_gib=24, image_state_ceiling_gib=3
        )
    ) == 0

    assert [(item.ceiling_bytes, item.state_ceiling_bytes) for item in seen] == [
        (24 * GIB, 3 * GIB)
    ]
    # Only this slot's copy changed; the configured defaults are untouched.
    assert seen[0].backend == configured.image_settings.backend
    after = cli._load_config(str(root), "node-a")
    assert after.image_settings == configured.image_settings == slotimage.ImageSettings()
    assert "image" not in (root / ".wrkslots.yml").read_text()


def test_create_keeps_the_configured_ceilings_without_the_options(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    seen = _record_provisioning(monkeypatch)
    root = _create_fixture(tmp_path)

    # A namespace with no ceiling attributes at all: callers that predate the options.
    assert cli._cmd_create(_create_args(root, representation="image")) == 0

    assert seen == [slotimage.ImageSettings()]


def test_create_sets_only_the_ceiling_it_is_given(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    seen = _record_provisioning(monkeypatch)
    root = _create_fixture(tmp_path)

    assert cli._cmd_create(
        _create_args(
            root, representation="image", image_ceiling_gib=None, image_state_ceiling_gib=2
        )
    ) == 0

    assert [(item.ceiling_bytes, item.state_ceiling_bytes) for item in seen] == [
        (slotimage.DEFAULT_CEILING_BYTES, 2 * GIB)
    ]


@pytest.mark.parametrize(
    ("overrides", "message"),
    [
        (
            {"representation": "worktree", "image_ceiling_gib": 20},
            "--image-ceiling-gib applies only to image-backed slots",
        ),
        (
            {"representation": "config", "image_state_ceiling_gib": 2},
            "--image-state-ceiling-gib applies only to image-backed slots",
        ),
        ({"representation": "image", "image_ceiling_gib": 0}, "--image-ceiling-gib must be at least 1"),
        (
            {"representation": "image", "image_state_ceiling_gib": -1},
            "--image-state-ceiling-gib must be at least 1",
        ),
    ],
)
def test_create_refuses_a_ceiling_it_cannot_apply_before_writing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, overrides: dict[str, object], message: str
) -> None:
    seen = _record_provisioning(monkeypatch)
    root = _create_fixture(tmp_path)
    before = sorted(path.relative_to(root) for path in root.rglob("*"))

    with pytest.raises(cli.Refusal, match=re.escape(message)):
        cli._cmd_create(_create_args(root, **overrides))

    assert seen == []
    assert sorted(path.relative_to(root) for path in root.rglob("*")) == before


def test_provision_slot_image_uses_the_settings_it_is_given(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    seen = _record_provisioning(monkeypatch)
    monkeypatch.setattr(cli, "_refresh_owned_mounts", lambda config: None)
    root = _create_fixture(tmp_path)
    config = cli._load_config(str(root), "node-a")
    custom = dataclasses.replace(config.image_settings, state_ceiling_bytes=5 * GIB)

    cli._provision_slot_image(config, "a", "agent", tmp_path / "a", custom)
    cli._provision_slot_image(config, "b", "agent", tmp_path / "b")

    assert seen == [custom, config.image_settings]


# ------------------------------------------- root launcher bytecode suppression


def test_root_helper_adds_dont_write_bytecode_only_when_this_interpreter_has_it(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    helper = str(Path(sandbox.__file__).resolve())
    interpreter = os.path.realpath(sys.executable)

    monkeypatch.setattr(sys, "dont_write_bytecode", False)
    assert sandbox.root_helper("SPEC") == [
        "sudo", "-n", "--", interpreter, "-I", helper, "enter-root", "SPEC"
    ]

    monkeypatch.setattr(sys, "dont_write_bytecode", True)
    assert sandbox.root_helper("SPEC") == [
        "sudo", "-n", "--", interpreter, "-I", "-B", helper, "enter-root", "SPEC"
    ]


@pytest.mark.parametrize(("flags", "expected"), [((), ["-I"]), (("-B",), ["-I", "-B"])])
def test_root_helper_carries_the_callers_dash_b_across_sudo_by_argv(
    tmp_path: Path, flags: tuple[str, ...], expected: list[str]
) -> None:
    """A `python3 -B` caller yields a root helper argv with `-I -B`; sudo scrubs the environment."""

    environment = {
        key: value
        for key, value in os.environ.items()
        if key != "PYTHONDONTWRITEBYTECODE"
    }
    environment["PYTHONPATH"] = str(PACKAGE_ROOT.parent)
    completed = subprocess.run(
        [
            sys.executable, *flags, "-c",
            "import json; from wrkslots import sandbox; print(json.dumps(sandbox.root_helper('SPEC')))",
        ],
        cwd=tmp_path, env=environment, text=True, capture_output=True, check=False, timeout=60,
    )

    assert completed.returncode == 0, completed.stderr
    argv = json.loads(completed.stdout)
    assert argv[4 : 4 + len(expected)] == expected
    assert argv[4 + len(expected)].endswith("sandbox.py")


# ----------------------------------------------- run --place-only (slices once)


def _fake_systemd_tools(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    """Put recording systemctl, systemd-escape and sudo first on PATH; return the call log.

    FAKE_SLEEP makes systemctl and sudo hang, for the deadline cases. Nothing
    here reaches a real user manager or sudo.
    """

    tools = tmp_path / "fake-tools"
    tools.mkdir()
    log = tmp_path / "calls.log"
    record = '#!/bin/sh\nprintf \'%s %s\\n\' "$(basename "$0")" "$*" >> "$FAKE_LOG"\n'
    hang = 'if [ -n "$FAKE_SLEEP" ]; then sleep "$FAKE_SLEEP"; fi\nexit 0\n'
    (tools / "systemctl").write_text(record + hang)
    (tools / "sudo").write_text(record + hang)
    (tools / "systemd-escape").write_text(
        record + 'printf \'%s\\n\' "$2" | sed \'s/-/\\\\x2d/g\'\n'
    )
    for tool in tools.iterdir():
        tool.chmod(0o755)
    monkeypatch.setenv("PATH", f"{tools}{os.pathsep}{os.environ.get('PATH', '')}")
    monkeypatch.setenv("FAKE_LOG", str(log))
    monkeypatch.delenv("FAKE_SLEEP", raising=False)
    return log


def _calls(log: Path) -> list[str]:
    return log.read_text().splitlines() if log.exists() else []


def _slot_view(tmp_path: Path, slot: str = "s1") -> sandbox.SlotView:
    return sandbox.SlotView(
        slot=slot,
        slot_type="agent",
        slot_path=tmp_path / "slot",
        state_directory=tmp_path / "state",
        git_directories=(),
        control_directory=tmp_path / "control",
        representation="image",
        project_root=tmp_path / "project",
    )


_LIMITS = sandbox.SandboxLimits(memory_max="8G", tasks_max=512, all_slots_memory_max="64G")


def _count_parent_slice_calls(monkeypatch: pytest.MonkeyPatch, cgroup: Path) -> list[int]:
    """Read the caller's slice from ``cgroup`` once; any second reading fails the test."""

    calls: list[int] = []
    real = sandbox.parent_slice

    def once(cgroup_file: Path = cgroup) -> str | None:
        calls.append(1)
        assert len(calls) == 1, "the caller's slice was read again after placement"
        return real(cgroup)

    monkeypatch.setattr(sandbox, "parent_slice", once)
    return calls


@pytest.mark.parametrize(
    ("cgroup_line", "triple"),
    [
        # The caller is already inside a named slice (T3b).
        (
            "0::/user.slice/user-1.slice/user@1.service/app.slice/agent-7.scope",
            ("app.slice", "app-wrkslots.slice", "app-wrkslots-s1.slice"),
        ),
        # A nested enclosing slice: the innermost one is the parent.
        (
            "0::/user.slice/user-1.slice/user@1.service/app.slice/app-jail.slice/x.scope",
            ("app-jail.slice", "app-jail-wrkslots.slice", "app-jail-wrkslots-s1.slice"),
        ),
        # No enclosing slice, represented explicitly rather than rediscovered.
        (
            "0::/user.slice/user-1.slice/user@1.service/app.scope",
            (None, "wrkslots.slice", "wrkslots-s1.slice"),
        ),
    ],
)
def test_place_only_reads_the_slice_once_and_limits_exactly_those_names(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    cgroup_line: str,
    triple: tuple[str | None, str, str],
) -> None:
    log = _fake_systemd_tools(tmp_path, monkeypatch)
    cgroup = tmp_path / "cgroup"
    cgroup.write_text(cgroup_line + "\n")
    calls = _count_parent_slice_calls(monkeypatch, cgroup)

    placement, applied = sandbox.place_only(_slot_view(tmp_path), _LIMITS, cgroup_file=cgroup)

    assert (placement.enclosing, placement.all_slots, placement.slot) == triple
    assert placement.to_obj() == dict(zip(("enclosing", "all_slots", "slot"), triple))
    assert calls == [1]  # one reading; a second one would have failed inside the wrapper
    _enclosing, all_slots, slot = triple
    systemctl = [line for line in _calls(log) if line.startswith("systemctl ")]
    assert systemctl == [
        f"systemctl --user start {all_slots}",
        f"systemctl --user set-property --runtime {all_slots} MemoryMax=64G MemorySwapMax=0",
        f"systemctl --user start {slot}",
        f"systemctl --user set-property --runtime {slot} MemoryMax=8G MemorySwapMax=0 TasksMax=512",
    ]
    assert applied == [
        f"{all_slots}: MemoryMax=64G MemorySwapMax=0",
        f"{slot}: MemoryMax=8G MemorySwapMax=0 TasksMax=512",
    ]
    assert not any("-wrkslots-s1-wrkslots" in line for line in _calls(log))


def test_explicit_absent_parent_is_not_rediscovered(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(sandbox, "parent_slice", lambda cgroup_file=None: "moved.slice")

    # None still means "rediscover", exactly as before.
    assert sandbox.root_slice() == "moved-wrkslots.slice"
    assert sandbox.root_slice(None) == "moved-wrkslots.slice"
    # The explicit absent value and an explicit name are taken as given.
    assert sandbox.root_slice(sandbox.NO_ENCLOSING_SLICE) == "wrkslots.slice"
    assert sandbox.root_slice("app.slice") == "app-wrkslots.slice"


def test_ordinary_slice_limits_still_rediscover_the_callers_slice(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    log = _fake_systemd_tools(tmp_path, monkeypatch)
    monkeypatch.setattr(sandbox, "parent_slice", lambda cgroup_file=None: "jail.slice")

    sandbox.apply_slice_limits(_slot_view(tmp_path), _LIMITS)

    assert "systemctl --user start jail-wrkslots.slice" in _calls(log)
    assert "systemctl --user start jail-wrkslots-s1.slice" in _calls(log)


def test_place_only_deadline_kills_a_hung_systemctl(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _fake_systemd_tools(tmp_path, monkeypatch)
    monkeypatch.setenv("FAKE_SLEEP", "30")
    cgroup = tmp_path / "cgroup"
    cgroup.write_text("0::/user.slice/user-1.slice/user@1.service/app.scope\n")
    started = time.monotonic()

    with pytest.raises(sandbox.SandboxError, match="the deadline passed during setting limits on wrkslots.slice"):
        sandbox.place_only(
            _slot_view(tmp_path),
            _LIMITS,
            cgroup_file=cgroup,
            deadline_ns=time.monotonic_ns() + 500_000_000,
        )

    assert time.monotonic() - started < 10


def test_a_passed_deadline_starts_nothing(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    log = _fake_systemd_tools(tmp_path, monkeypatch)
    cgroup = tmp_path / "cgroup"
    cgroup.write_text("0::/user.slice/user-1.slice/user@1.service/app.scope\n")

    with pytest.raises(sandbox.SandboxError, match="the deadline passed before systemd-escape"):
        sandbox.place_only(
            _slot_view(tmp_path), _LIMITS, cgroup_file=cgroup, deadline_ns=time.monotonic_ns() - 1
        )
    with pytest.raises(sandbox.SandboxError, match=r"the deadline passed before `sudo -n true`"):
        sandbox.check_sudo(deadline_ns=time.monotonic_ns() - 1)

    assert _calls(log) == []


def test_check_sudo_deadline_kills_a_hung_probe(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    log = _fake_systemd_tools(tmp_path, monkeypatch)
    monkeypatch.setenv("FAKE_SLEEP", "30")
    started = time.monotonic()

    with pytest.raises(sandbox.SandboxError, match=r"the deadline passed during `sudo -n true`"):
        sandbox.check_sudo(deadline_ns=time.monotonic_ns() + 300_000_000)

    assert time.monotonic() - started < 10
    assert _calls(log) == ["sudo -n true"]
    # Without a deadline the probe is unchanged: it simply runs.
    monkeypatch.delenv("FAKE_SLEEP")
    sandbox.check_sudo()
    assert _calls(log) == ["sudo -n true", "sudo -n true"]


def _registered_slot(tmp_path: Path) -> Path:
    root = _create_fixture(tmp_path)
    assert cli._cmd_create(_create_args(root, representation="worktree")) == 0
    return root


def test_run_place_only_prints_the_captured_slices(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _registered_slot(tmp_path)
    log = _fake_systemd_tools(tmp_path, monkeypatch)
    cgroup = tmp_path / "cgroup"
    cgroup.write_text("0::/user.slice/user-1.slice/user@1.service/app.slice/agent.scope\n")
    real = sandbox.place_only
    monkeypatch.setattr(
        sandbox,
        "place_only",
        lambda view, limits, *, deadline_ns=None: real(
            view, limits, deadline_ns=deadline_ns, cgroup_file=cgroup
        ),
    )
    capsys.readouterr()

    assert cli.main(
        ["--project-root", str(root), "run", "slot-b", "--place-only", "--format", "json",
         "--memory-max", "8G", "--deadline-monotonic-ns", str(time.monotonic_ns() + 60 * 10**9)]
    ) == 0

    captured = capsys.readouterr()
    assert json.loads(captured.out) == {
        "enclosing": "app.slice",
        "all_slots": "app-wrkslots.slice",
        "slot": "app-wrkslots-slot\\x2db.slice",
    }
    assert "wrkslots run: limits app-wrkslots-slot\\x2db.slice: MemoryMax=8G" in captured.err
    assert not any(line.startswith(("busctl", "systemd-run")) for line in _calls(log))

    assert cli.main(["--project-root", str(root), "run", "slot-b", "--place-only"]) == 0
    assert capsys.readouterr().out.splitlines() == [
        "enclosing: app.slice",
        "all_slots: app-wrkslots.slice",
        "slot: app-wrkslots-slot\\x2db.slice",
    ]


@pytest.mark.parametrize(
    ("extra", "message"),
    [
        (["--place-only", "--", "true"], "--place-only runs nothing and takes no COMMAND"),
        (["--place-only", "--print"], "--place-only runs nothing and takes no --print"),
        (["--place-only", "--cwd", "/"], "--place-only runs nothing and takes no --cwd"),
        (["--format", "json", "--", "true"], "--format applies only with --place-only"),
        (["--deadline-monotonic-ns", "1", "--", "true"], "--deadline-monotonic-ns applies only with --place-only"),
    ],
)
def test_run_refuses_place_only_options_it_cannot_honour(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    extra: list[str],
    message: str,
) -> None:
    root = _registered_slot(tmp_path)
    log = _fake_systemd_tools(tmp_path, monkeypatch)
    capsys.readouterr()

    assert cli.main(["--project-root", str(root), "run", "slot-b", *extra]) != 0

    assert message in capsys.readouterr().err
    assert _calls(log) == []


# ------------------------------------- run --in-scope (no second scope), closed set

_MANAGER = "0::/user.slice/user-1.slice/user@1.service"
_APP = sandbox.SlicePlacement("app.slice", "app-wrkslots.slice", "app-wrkslots-s1.slice")


def _no_rediscovery(monkeypatch: pytest.MonkeyPatch) -> None:
    def refuse(cgroup_file: Path = Path("/proc/self/cgroup")) -> str | None:
        raise AssertionError("the caller's slice was rediscovered inside the scope")

    monkeypatch.setattr(sandbox, "parent_slice", refuse)


@pytest.mark.parametrize(
    ("placement", "path"),
    [
        (_APP, "/app.slice/app-wrkslots.slice/app-wrkslots-s1.slice/run-1.scope"),
        (
            sandbox.SlicePlacement("app-jail.slice", "app-jail-wrkslots.slice", "app-jail-wrkslots-s1.slice"),
            "/app.slice/app-jail.slice/app-jail-wrkslots.slice/app-jail-wrkslots-s1.slice/run-1.scope",
        ),
        (
            sandbox.SlicePlacement(None, "wrkslots.slice", "wrkslots-s1.slice"),
            "/wrkslots.slice/wrkslots-s1.slice/run-1.scope",
        ),
    ],
)
def test_in_scope_accepts_exactly_the_placed_scope_without_rediscovery(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    placement: sandbox.SlicePlacement,
    path: str,
) -> None:
    _fake_systemd_tools(tmp_path, monkeypatch)
    _no_rediscovery(monkeypatch)
    cgroup = tmp_path / "cgroup"
    cgroup.write_text(_MANAGER + path + "\n")

    sandbox.verify_in_scope(_slot_view(tmp_path), "run-1.scope", placement, cgroup_file=cgroup)


@pytest.mark.parametrize(
    ("unit", "placement", "path", "message"),
    [
        # A second, nested slot tree: what rediscovering the slice inside the scope makes.
        (
            "run-1.scope",
            _APP,
            "/app.slice/app-wrkslots.slice/app-wrkslots-s1.slice/app-wrkslots-s1-wrkslots.slice/"
            "app-wrkslots-s1-wrkslots-s1.slice/run-1.scope",
            "this process is in",
        ),
        # Still in the caller's own cgroup: the scope was never joined.
        ("run-1.scope", _APP, "/app.slice/agent-7.scope", "this process is in"),
        # Another scope under the right slices.
        ("run-1.scope", _APP, "/app.slice/app-wrkslots.slice/app-wrkslots-s1.slice/run-2.scope", "this process is in"),
        # A different enclosing slice than the one captured.
        ("run-1.scope", _APP, "/wrkslots.slice/wrkslots-s1.slice/run-1.scope", "this process is in"),
        # Not below a user manager at all.
        ("run-1.scope", _APP, None, "this process is in"),
        # The triple names another slot's slices.
        (
            "run-1.scope",
            sandbox.SlicePlacement("app.slice", "app-wrkslots.slice", "app-wrkslots-s2.slice"),
            "/app.slice/app-wrkslots.slice/app-wrkslots-s2.slice/run-1.scope",
            "are not slot s1's",
        ),
        # The aggregate slice is not the one the enclosing slice implies.
        (
            "run-1.scope",
            sandbox.SlicePlacement("app.slice", "wrkslots.slice", "app-wrkslots-s1.slice"),
            "/app.slice/app-wrkslots.slice/app-wrkslots-s1.slice/run-1.scope",
            "are not slot s1's",
        ),
        ("run-1.service", _APP, "/app.slice/app-wrkslots.slice/app-wrkslots-s1.slice/run-1.service", "needs a scope unit"),
    ],
)
def test_in_scope_refuses_any_other_placement(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    unit: str,
    placement: sandbox.SlicePlacement,
    path: str | None,
    message: str,
) -> None:
    _fake_systemd_tools(tmp_path, monkeypatch)
    _no_rediscovery(monkeypatch)
    cgroup = tmp_path / "cgroup"
    cgroup.write_text(("0::/system.slice/x.scope" if path is None else _MANAGER + path) + "\n")

    with pytest.raises(sandbox.SandboxError, match=re.escape(message)):
        sandbox.verify_in_scope(_slot_view(tmp_path), unit, placement, cgroup_file=cgroup)


class _Execed(Exception):
    def __init__(self, argv: list[str], spec: dict[str, object]) -> None:
        super().__init__(argv[0])
        self.argv = argv
        self.spec = spec


def _in_scope_host(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> tuple[Path, Path, Path, Path]:
    """A fake HOME with a host ~/.cargo, a runtime directory, the fake tools, and an exec catcher.

    Returns (home, target, cgroup file, call log).
    """

    log = _fake_systemd_tools(tmp_path, monkeypatch)
    home = tmp_path / "home"
    for directory in (".cargo/registry", ".cargo/git", ".cache", ".claude", "work"):
        (home / directory).mkdir(parents=True)
    (home / ".cargo/config.toml").write_text("[net]\noffline = true\n")
    runtime = tmp_path / "runtime"
    runtime.mkdir(mode=0o700)
    target = tmp_path / "slot" / "target"
    target.mkdir(parents=True)
    (tmp_path / "control").mkdir()
    monkeypatch.setenv("HOME", str(home))
    monkeypatch.setenv("XDG_RUNTIME_DIR", str(runtime))
    monkeypatch.delenv("WRKSLOTS_BOX", raising=False)
    cgroup = tmp_path / "cgroup"
    cgroup.write_text(_MANAGER + "/app.slice/app-wrkslots.slice/app-wrkslots-s1.slice/run-1.scope\n")

    def execvpe(file: str, argv: list[str], env: dict[str, str]) -> None:
        spec = json.loads(Path(argv[-1]).read_text())
        assert stat.S_IMODE(os.stat(argv[-1]).st_mode) == 0o600
        raise _Execed(list(argv), spec)

    monkeypatch.setattr(os, "execvpe", execvpe)
    return home, target, cgroup, log


def _closed_settings(target: Path) -> sandbox.SandboxSettings:
    args = argparse.Namespace(
        closed_set=True, isolation="root", home=None, home_shared=None,
        home_private=[".cargo"], home_private_file=None, home_hidden=None, home_expose=None,
        output=None, read_write=[str(target)], env=None, tmp_size="1G", no_protect_system=False,
        memory_max=None, memory_high=None, cpu_quota=None, tasks_max=None,
    )
    return imagecmd._override_settings(sandbox.SandboxSettings(), args)


def test_closed_set_replaces_the_lists_and_open_runs_still_add(tmp_path: Path) -> None:
    target = tmp_path / "target"
    closed = _closed_settings(target)
    assert closed.home_shared == ()
    assert closed.home_private == (".cargo",)
    assert closed.outputs == ()
    assert closed.read_write == (str(target),)
    assert closed.tmp_size == "1G"
    # Masks and private files are configuration, not writable roots: kept.
    assert closed.home_hidden == sandbox.DEFAULT_HOME_HIDDEN

    opened = imagecmd._override_settings(
        sandbox.SandboxSettings(),
        argparse.Namespace(
            isolation=None, home=None, home_shared=None, home_private=[".cargo"],
            home_private_file=None, home_hidden=None, home_expose=None, output=None,
            read_write=None, env=None, tmp_size=None, no_protect_system=False,
            memory_max=None, memory_high=None, cpu_quota=None, tasks_max=None,
        ),
    )
    assert opened.home_shared == sandbox.DEFAULT_HOME_SHARED
    assert opened.home_private == (*sandbox.DEFAULT_HOME_PRIVATE, ".cargo")


def test_enter_in_place_runs_the_root_steps_in_the_callers_scope(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    home, target, cgroup, log = _in_scope_host(tmp_path, monkeypatch)
    _no_rediscovery(monkeypatch)
    view = _slot_view(tmp_path)
    settings = _closed_settings(target)
    monkeypatch.setattr(sys, "dont_write_bytecode", True)

    with pytest.raises(_Execed) as caught:
        sandbox.enter_in_place(
            view, settings, ["cargo", "build"], unit="run-1.scope", expected=_APP,
            slot_writable=False, cgroup_file=cgroup,
            deadline_ns=time.monotonic_ns() + 60 * 10**9,
        )

    argv, spec = caught.value.argv, caught.value.spec
    assert argv[:6] == ["sudo", "-n", "--", os.path.realpath(sys.executable), "-I", "-B"]
    assert argv[6:8] == [str(Path(sandbox.__file__).resolve()), "enter-root"]
    assert not Path(argv[-1]).exists()  # the spec is removed when the exec does not happen
    # The slice check (one escape), then the sudo probe; no scope was created or
    # joined (no busctl, no systemd-run) and no limit was applied (no systemctl).
    assert _calls(log) == ["systemd-escape -- s1", "sudo -n true"]
    view_spec = spec["view"]
    assert isinstance(view_spec, dict)
    assert view_spec["binds"] == [[str(target), str(target)]]
    assert view_spec["tmp_size"] == "1G" and view_spec["protect_system"] is True
    # The payload's ~/.cargo is the slot's private directory, not the host's.
    assert ".cargo" not in view_spec["home_binds"]
    assert (tmp_path / "state" / "home" / ".cargo").is_dir()
    assert spec["command"] == ["cargo", "build"]
    assert spec["environment"]["WRKSLOTS_SLOT"] == "s1"  # type: ignore[index]
    writable = sandbox.writable_set(view_spec)
    assert writable == {
        "home_layer": {"source": str(tmp_path / "state" / "home"), "target": str(home)},
        "binds": [{"source": str(target), "target": str(target)}],
        "tmp": {"target": "/tmp", "size": "1G"},
        "protect_system": True,
        "always_writable": ["/proc", "/sys", "/dev", "/run/user"],
    }


def _bind_targets(report: dict[str, object]) -> list[str]:
    binds = report["binds"]
    assert isinstance(binds, list)
    return [str(item["target"]) for item in binds if isinstance(item, dict)]


def test_an_open_spec_reports_every_writable_bind(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    home, _target, _cgroup, _log = _in_scope_host(tmp_path, monkeypatch)
    for directory in ("slot", "project/ai_docs"):
        (tmp_path / directory).mkdir(parents=True, exist_ok=True)
    view = _slot_view(tmp_path)

    open_spec = sandbox.build_spec(view, sandbox.SandboxSettings(), home, tmp_path / "slot")
    closed_spec = sandbox.build_spec(
        view, sandbox.SandboxSettings(), home, tmp_path / "slot", slot_writable=False
    )

    targets = _bind_targets(sandbox.writable_set(open_spec))
    assert str(home / ".cargo/registry") in targets and str(home / ".cargo/git") in targets
    assert str(tmp_path / "slot") in targets and str(tmp_path / "project/ai_docs") in targets
    closed_targets = _bind_targets(sandbox.writable_set(closed_spec))
    assert str(tmp_path / "slot") not in closed_targets
    assert str(home / ".cargo/registry") in closed_targets  # the lists are the caller's to close
    unprotected = sandbox.writable_set({**open_spec, "protect_system": False})
    assert unprotected["always_writable"] == ["/"]


@pytest.mark.parametrize(
    ("change", "message"),
    [
        ("userns", "needs isolation root, not userns"),
        ("box", "from inside a coordinator box"),
        ("cgroup", "this process is in"),
    ],
)
def test_enter_in_place_refuses_before_touching_state(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, change: str, message: str
) -> None:
    _home, target, cgroup, log = _in_scope_host(tmp_path, monkeypatch)
    settings = _closed_settings(target)
    if change == "userns":
        settings = dataclasses.replace(settings, isolation="userns")
    elif change == "box":
        monkeypatch.setenv("WRKSLOTS_BOX", "coordinator")
    else:
        cgroup.write_text(_MANAGER + "/app.slice/agent-7.scope\n")

    with pytest.raises(sandbox.SandboxError, match=re.escape(message)):
        sandbox.enter_in_place(
            _slot_view(tmp_path), settings, ["true"], unit="run-1.scope", expected=_APP,
            slot_writable=False, cgroup_file=cgroup,
        )

    assert not (tmp_path / "state").exists()
    assert not any(line.startswith("sudo") for line in _calls(log))
    assert list((tmp_path / "runtime").iterdir()) in ([], [tmp_path / "runtime" / "wrkslots"])


def test_run_in_scope_print_reports_the_closed_writable_set(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _registered_slot(tmp_path)
    home, _target, cgroup, log = _in_scope_host(tmp_path, monkeypatch)
    slot_path = root / "worktrees" / "slot-b"
    target = slot_path / "target"
    target.mkdir()
    cgroup.write_text(
        _MANAGER + "/app.slice/app-wrkslots.slice/app-wrkslots-slot\\x2db.slice/run-1.scope\n"
    )
    real = sandbox.in_scope_plan

    def plan(*args: object, **kwargs: object) -> dict[str, object]:
        return real(*args, **kwargs, cgroup_file=cgroup)  # type: ignore[arg-type]

    monkeypatch.setattr(sandbox, "in_scope_plan", plan)
    expected = json.dumps(
        {"enclosing": "app.slice", "all_slots": "app-wrkslots.slice", "slot": "app-wrkslots-slot\\x2db.slice"}
    )
    capsys.readouterr()

    assert cli.main(
        ["--project-root", str(root), "run", "slot-b", "--in-scope", "run-1.scope",
         "--expect-slices", expected, "--isolation", "root", "--closed-set",
         "--home-private", ".cargo", "--read-write", str(target), "--tmp-size", "1G",
         "--print", "--", "cargo", "build"]
    ) == 0

    report = json.loads(capsys.readouterr().out)
    assert report["unit"] == "run-1.scope"
    assert report["writable"]["binds"] == [{"source": str(target), "target": str(target)}]
    assert report["writable"]["tmp"] == {"target": "/tmp", "size": "1G"}
    assert report["helper"][4] == "-I"
    assert ".cargo" not in report["view"]["home_binds"]
    assert _calls(log) == [line for line in _calls(log) if line.startswith("systemd-escape ")]
    # --print wrote nothing into the slot's private state.
    control = cli._load_config(str(root), "node-a").control
    assert not (control / "slot-state").exists()


@pytest.mark.parametrize(
    ("extra", "message"),
    [
        (["--in-scope", "run-1.scope", "--", "true"], "--in-scope and --expect-slices are given together"),
        (["--expect-slices", "{}", "--", "true"], "--in-scope and --expect-slices are given together"),
        (["--in-scope", "run-1.scope", "--expect-slices", "[1]", "--", "true"], "--expect-slices must be an object"),
        (["--in-scope", "run-1.scope", "--expect-slices", "{", "--", "true"], "--expect-slices must be the JSON object"),
        (["--closed-set", "--", "true"], "--closed-set applies only with --place-only or --in-scope"),
        (
            ["--in-scope", "run-1.scope", "--expect-slices", "{}", "--closed-set", "--no-protect-system", "--", "true"],
            "--closed-set refuses --no-protect-system",
        ),
        (["--place-only", "--in-scope", "run-1.scope"], "--place-only runs nothing and takes no --in-scope"),
    ],
)
def test_run_refuses_in_scope_options_it_cannot_honour(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    extra: list[str],
    message: str,
) -> None:
    root = _registered_slot(tmp_path)
    log = _fake_systemd_tools(tmp_path, monkeypatch)
    capsys.readouterr()

    assert cli.main(["--project-root", str(root), "run", "slot-b", *extra]) != 0

    assert message in capsys.readouterr().err
    assert _calls(log) == []


# ------------------------------------------- no bytecode writes from a -B chain


def _tree_digest(root: Path) -> dict[str, str]:
    return {
        str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in sorted(root.rglob("*"))
        if path.is_file()
    }


def _package_copy(base: Path, state: str) -> Path:
    """A copy of the package as PYTHONPATH: cold (no caches) or stale (caches older than sources)."""

    parent = base / "pythonpath"
    shutil.copytree(
        PACKAGE_ROOT, parent / "wrkslots", ignore=shutil.ignore_patterns("__pycache__", "tests")
    )
    if state == "stale":
        compiled = subprocess.run(
            [sys.executable, "-m", "compileall", "-q", str(parent / "wrkslots")],
            capture_output=True, text=True, check=False, timeout=300,
        )
        assert compiled.returncode == 0, compiled.stderr
        later = time.time() + 60
        for source in (parent / "wrkslots").rglob("*.py"):
            os.utime(source, (later, later))
        assert list((parent / "wrkslots").rglob("*.pyc"))
    return parent


@pytest.mark.parametrize("state", ["cold", "stale"])
def test_a_dash_b_placement_and_root_helper_write_no_bytecode(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, state: str
) -> None:
    root = _registered_slot(tmp_path)
    _fake_systemd_tools(tmp_path, monkeypatch)
    parent = _package_copy(tmp_path, state)
    before = _tree_digest(parent)
    environment = {key: value for key, value in os.environ.items() if key != "PYTHONDONTWRITEBYTECODE"}
    environment["PYTHONPATH"] = str(parent)

    placed = subprocess.run(
        [sys.executable, "-B", "-m", "wrkslots", "--project-root", str(root), "run", "slot-b",
         "--place-only", "--format", "json"],
        cwd=tmp_path, env=environment, text=True, capture_output=True, check=False, timeout=300,
    )
    helper = subprocess.run(
        [sys.executable, "-B", "-c",
         "import json; from wrkslots import sandbox; print(json.dumps(sandbox.root_helper('SPEC')))"],
        cwd=tmp_path, env=environment, text=True, capture_output=True, check=False, timeout=300,
    )

    assert placed.returncode == 0, placed.stderr
    assert set(json.loads(placed.stdout)) == {"enclosing", "all_slots", "slot"}
    assert helper.returncode == 0, helper.stderr
    assert json.loads(helper.stdout)[4:6] == ["-I", "-B"]
    assert json.loads(helper.stdout)[6] == str(parent / "wrkslots" / "sandbox.py")
    assert _tree_digest(parent) == before

    # The probe can see a write: the same import without -B fills the cache.
    unsuppressed = subprocess.run(
        [sys.executable, "-c", "from wrkslots import sandbox"],
        cwd=tmp_path, env=environment, text=True, capture_output=True, check=False, timeout=300,
    )
    assert unsuppressed.returncode == 0, unsuppressed.stderr
    assert _tree_digest(parent) != before


# ------------------------------------ image status: the fields a caller binds to


def _sparse_image(tmp_path: Path) -> slotimage.SlotImage:
    """A slot image record whose files are sparse: 3 GiB apparent, 1 MiB written; 1 GiB state."""

    directory = tmp_path / "control" / "slot-images" / "agent" / "s1"
    (directory / "state").mkdir(parents=True)
    location = tmp_path / "slot"
    location.mkdir()
    image = slotimage.SlotImage(
        directory=directory, slot="s1", slot_type="agent", location=location,
        ceiling_bytes=3 * GIB, state_ceiling_bytes=GIB, backend="kernel",
        created_at="2026-10-30T00:00:00Z", phase="ready",
    )
    with open(image.slot_image, "wb") as handle:
        handle.truncate(3 * GIB)
        handle.write(b"x" * 1024 * 1024)
    with open(image.state_image, "wb") as handle:
        handle.truncate(GIB)
    return image


def test_image_status_reports_apparent_size_allocation_and_the_actual_mount_separately(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    image = _sparse_image(tmp_path)
    mounted: list[tuple[Path, Path]] = []

    def backend(image_file: Path, mount_point: Path, table: object = None) -> str | None:
        mounted.append((image_file, mount_point))
        return "kernel" if image_file == image.slot_image else None

    monkeypatch.setattr(slotimage, "mount_table", lambda mountinfo=None: [])
    monkeypatch.setattr(slotimage, "mounted_backend", backend)

    report = slotimage.usage(image)

    # Each image is judged against its own mount point, never another path.
    assert mounted == [(image.slot_image, image.location), (image.state_image, image.state_mount)]
    assert report["name"] == "s1" and report["slot_type"] == "agent"
    assert report["location"] == str(image.location) and report["phase"] == "ready"
    slot_stat = os.stat(image.slot_image)
    vfs = os.statvfs(image.location)
    slot = report["slot"]
    assert isinstance(slot, dict)
    assert slot["image"] == str(image.slot_image)
    assert slot["ceiling_bytes"] == 3 * GIB == slot_stat.st_size  # apparent size C
    assert slot["host_allocated_bytes"] == slot_stat.st_blocks * 512  # B_alloc
    assert int(str(slot["host_allocated_bytes"])) < 3 * GIB
    assert slot["mounted"] is True and slot["backend"] == "kernel"
    assert slot["free_bytes"] == vfs.f_bavail * vfs.f_frsize  # F
    assert slot["used_bytes"] == (vfs.f_blocks - vfs.f_bfree) * vfs.f_frsize  # U
    assert set(slot) == {
        "image", "ceiling_bytes", "host_allocated_bytes", "mounted", "backend",
        "used_bytes", "free_bytes", "used_inodes",
    }
    # Not this image's own mount: no used or free figure is invented for it.
    state = report["state"]
    assert state == {
        "image": str(image.state_image),
        "ceiling_bytes": GIB,
        "host_allocated_bytes": os.stat(image.state_image).st_blocks * 512,
        "mounted": False,
    }


def test_image_status_marks_an_unreadable_backing_file(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    image = _sparse_image(tmp_path)
    image.state_image.unlink()
    monkeypatch.setattr(slotimage, "mount_table", lambda mountinfo=None: [])
    monkeypatch.setattr(slotimage, "mounted_backend", lambda *args, **kwargs: None)

    state = slotimage.usage(image)["state"]

    assert isinstance(state, dict)
    assert "error" in state and "ceiling_bytes" not in state and "host_allocated_bytes" not in state
    assert state["mounted"] is False


def test_image_status_json_carries_every_bound_field(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _create_fixture(tmp_path)
    image = _sparse_image(tmp_path)
    monkeypatch.setattr(slotimage, "all_images", lambda control: [image])
    monkeypatch.setattr(slotimage, "mount_table", lambda mountinfo=None: [])
    monkeypatch.setattr(
        slotimage,
        "mounted_backend",
        lambda image_file, mount_point, table=None: "fuse" if image_file == image.slot_image else None,
    )
    capsys.readouterr()

    assert imagecmd._cmd_image_status(
        argparse.Namespace(project_root=str(root), machine="node-a", format="json")
    ) == 0

    document = json.loads(capsys.readouterr().out)
    assert set(document) == {"new_slot_representation", "images"}
    [entry] = document["images"]
    assert {key: entry[key] for key in ("name", "slot_type", "location", "phase")} == {
        "name": "s1", "slot_type": "agent", "location": str(image.location), "phase": "ready",
    }
    assert entry["slot"]["mounted"] is True and entry["slot"]["backend"] == "fuse"
    assert entry["slot"]["ceiling_bytes"] == 3 * GIB
    assert {"host_allocated_bytes", "free_bytes", "used_bytes"} <= set(entry["slot"])
    assert entry["state"]["mounted"] is False and "free_bytes" not in entry["state"]


def test_in_scope_print_reports_the_private_binds_the_entry_will_create(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The printed writable set equals the one the real entry builds after preparing the state.

    A nested home_private directory is bound from the slot's private layer;
    on a fresh state it does not exist until prepare_state creates it. A
    missing --read-write input is still skipped by both, as the helper does.
    """

    _home, target, cgroup, _log = _in_scope_host(tmp_path, monkeypatch)
    view = _slot_view(tmp_path)
    settings = dataclasses.replace(
        _closed_settings(target),
        home="hidden",
        home_private=(".cargo", "nested/cache"),
        read_write=(str(target), str(tmp_path / "missing-input")),
    )
    request = dict(unit="run-1.scope", expected=_APP, slot_writable=False, cgroup_file=cgroup)

    planned = sandbox.in_scope_plan(view, settings, ["true"], **request)  # type: ignore[arg-type]
    assert not (tmp_path / "state").exists()  # the plan prepared nothing
    with pytest.raises(_Execed) as caught:
        sandbox.enter_in_place(view, settings, ["true"], **request)  # type: ignore[arg-type]
    actual_view = caught.value.spec["view"]
    assert isinstance(actual_view, dict)

    assert planned["writable"] == sandbox.writable_set(actual_view)
    nested = str(tmp_path / "state" / "home" / "nested" / "cache")
    assert _bind_targets(sandbox.writable_set(actual_view)) == [
        str(_home / "nested" / "cache"), str(target)
    ]
    assert nested in [
        item["source"] for item in planned["writable"]["binds"]  # type: ignore[index]
    ]
