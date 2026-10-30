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

    # The mounted file system's figures come from one fixed observation, not the live
    # host file system, whose free space moves between any two reads. Every field
    # differs from every other, so reading the wrong field cannot give the right number.
    observed: list[str] = []
    fixed = os.statvfs_result((
        4096,     # f_bsize
        1024,     # f_frsize: the unit every byte figure is counted in
        1000003,  # f_blocks
        400009,   # f_bfree
        300007,   # f_bavail
        70001,    # f_files
        20011,    # f_ffree
        20003,    # f_favail
        0,        # f_flag
        255,      # f_namemax
    ))

    def statvfs(path: str | os.PathLike[str]) -> os.statvfs_result:
        observed.append(os.fspath(path))
        return fixed

    monkeypatch.setattr(slotimage, "mount_table", lambda mountinfo=None: [])
    monkeypatch.setattr(slotimage, "mounted_backend", backend)
    monkeypatch.setattr(os, "statvfs", statvfs)  # slotimage reads os.statvfs at call time

    report = slotimage.usage(image)

    # Each image is judged against its own mount point, never another path.
    assert mounted == [(image.slot_image, image.location), (image.state_image, image.state_mount)]
    # Only the mounted image's own mount point is measured, and only once.
    assert observed == [str(image.location)]
    assert report["name"] == "s1" and report["slot_type"] == "agent"
    assert report["location"] == str(image.location) and report["phase"] == "ready"
    slot_stat = os.stat(image.slot_image)
    slot = report["slot"]
    assert isinstance(slot, dict)
    assert slot["image"] == str(image.slot_image)
    assert slot["ceiling_bytes"] == 3 * GIB == slot_stat.st_size  # apparent size C
    assert slot["host_allocated_bytes"] == slot_stat.st_blocks * 512  # B_alloc
    assert int(str(slot["host_allocated_bytes"])) < 3 * GIB
    assert slot["mounted"] is True and slot["backend"] == "kernel"
    assert slot["free_bytes"] == 300007 * 1024 == 307207168  # F: f_bavail * f_frsize
    # U: (f_blocks - f_bfree) * f_frsize
    assert slot["used_bytes"] == (1000003 - 400009) * 1024 == 614393856
    assert slot["used_inodes"] == 70001 - 20011 == 49990  # f_files - f_ffree
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


# ------------------------- --expect-writable: the entered set is the reported set


def test_entry_refuses_a_writable_set_other_than_the_reported_one(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """--print alone enforces nothing; the real entry compares its whole set with the report.

    A --read-write input that was missing when the report was made (so the
    report skipped it) and exists at entry would add a writable bind: refused
    before any private state is written, sudo is probed or anything is executed.
    """

    _home, target, cgroup, log = _in_scope_host(tmp_path, monkeypatch)
    view = _slot_view(tmp_path)
    late = tmp_path / "late-input"
    settings = dataclasses.replace(_closed_settings(target), read_write=(str(target), str(late)))
    request = dict(unit="run-1.scope", expected=_APP, slot_writable=False, cgroup_file=cgroup)
    reported = sandbox.in_scope_plan(view, settings, ["true"], **request)["writable"]  # type: ignore[arg-type]
    assert isinstance(reported, dict)
    assert _bind_targets(reported) == [str(target)]

    late.mkdir()
    with pytest.raises(sandbox.SandboxError, match="differs from the expected one"):
        sandbox.enter_in_place(
            view, settings, ["true"], expected_writable=reported, **request  # type: ignore[arg-type]
        )
    assert not (tmp_path / "state").exists()
    assert not any(line.startswith("sudo") for line in _calls(log))

    # Every member is compared, not only the binds.
    late.rmdir()
    for changed in (
        {**reported, "tmp": {"target": "/tmp", "size": "2G"}},
        {**reported, "protect_system": False},
        {**reported, "binds": []},
        {**reported, "extra": 1},
    ):
        with pytest.raises(sandbox.SandboxError, match="differs from the expected one"):
            sandbox.in_scope_plan(
                view, settings, ["true"], expected_writable=changed, **request  # type: ignore[arg-type]
            )

    # The reported set itself is accepted, and the entry proceeds to the helper.
    with pytest.raises(_Execed) as caught:
        sandbox.enter_in_place(
            view, settings, ["true"], expected_writable=reported, **request  # type: ignore[arg-type]
        )
    actual = caught.value.spec["view"]
    assert isinstance(actual, dict)
    assert sandbox.writable_set(actual) == reported


@pytest.mark.parametrize(
    ("extra", "message"),
    [
        (["--expect-writable", "{}", "--", "true"], "--expect-writable applies only with --in-scope"),
        (["--place-only", "--expect-writable", "{}"], "--place-only runs nothing and takes no --expect-writable"),
        (
            ["--in-scope", "run-1.scope", "--expect-slices",
             json.dumps({"enclosing": None, "all_slots": "wrkslots.slice", "slot": "wrkslots-slot\\x2db.slice"}),
             "--expect-writable", "[1]", "--", "true"],
            "--expect-writable must be the JSON object --print reported as writable",
        ),
    ],
)
def test_run_refuses_expect_writable_where_it_cannot_apply(
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


def test_run_in_scope_print_with_a_different_expected_set_refuses(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _registered_slot(tmp_path)
    _home, _target, cgroup, _log = _in_scope_host(tmp_path, monkeypatch)
    target = root / "worktrees" / "slot-b" / "target"
    target.mkdir()
    cgroup.write_text(
        _MANAGER + "/app.slice/app-wrkslots.slice/app-wrkslots-slot\\x2db.slice/run-1.scope\n"
    )
    real = sandbox.in_scope_plan

    def plan(*args: object, **kwargs: object) -> dict[str, object]:
        return real(*args, **kwargs, cgroup_file=cgroup)  # type: ignore[arg-type]

    monkeypatch.setattr(sandbox, "in_scope_plan", plan)
    slices = json.dumps(
        {"enclosing": "app.slice", "all_slots": "app-wrkslots.slice", "slot": "app-wrkslots-slot\\x2db.slice"}
    )
    base = ["--project-root", str(root), "run", "slot-b", "--in-scope", "run-1.scope",
            "--expect-slices", slices, "--isolation", "root", "--closed-set",
            "--home-private", ".cargo", "--read-write", str(target), "--tmp-size", "1G"]
    capsys.readouterr()
    assert cli.main([*base, "--print", "--", "true"]) == 0
    writable = json.loads(capsys.readouterr().out)["writable"]

    assert cli.main([*base, "--expect-writable", json.dumps(writable), "--print", "--", "true"]) == 0
    capsys.readouterr()
    narrowed = {**writable, "binds": []}
    assert cli.main([*base, "--expect-writable", json.dumps(narrowed), "--print", "--", "true"]) != 0
    assert "differs from the expected one" in capsys.readouterr().err


# ------------------- --git-config-policy isolated and --expect-masks (in-scope entry)
#
# Real namespaces and mounts are NOT exercised here: the mount calls, the
# view-building steps around them, sudo and the root helper are stubs or
# recorders, as in the in-scope tests above. These tests bind the spec, the
# report, the comparisons, the per-path decisions and the in-view checks; they
# are not a host acceptance of the mounted view.


def _policy_request(cgroup: Path) -> dict[str, object]:
    return dict(unit="run-1.scope", expected=_APP, slot_writable=False, cgroup_file=cgroup)


def _git_masks(home: Path) -> list[str]:
    return [str(home / ".gitconfig"), str(home / ".config/git"), "/etc/gitconfig"]


def test_isolated_policy_reports_its_masks_and_the_entry_carries_them(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    home, target, cgroup, _log = _in_scope_host(tmp_path, monkeypatch)
    monkeypatch.setattr(sandbox, "path_aliases", lambda path: [path])
    view = _slot_view(tmp_path)
    settings = _closed_settings(target)
    request = _policy_request(cgroup)

    inherited = sandbox.in_scope_plan(view, settings, ["true"], **request)  # type: ignore[arg-type]
    isolated = sandbox.in_scope_plan(
        view, settings, ["true"], git_config_policy="isolated", **request  # type: ignore[arg-type]
    )

    # The default spec is unchanged: no policy keys, the configured masks only.
    assert "git_config_masks" not in inherited["view"] and "git_config_policy" not in inherited["view"]  # type: ignore[operator]
    assert inherited["masks"] == {
        "masks": inherited["view"]["masks"],  # type: ignore[index]
        "git_config_policy": "inherit",
        "git_config_masks": [],
    }
    base = list(inherited["masks"]["masks"])  # type: ignore[index]
    assert str(home / ".ssh") in base and str(home / ".gitconfig") not in base
    # Isolated: the same list, then each fixed Git location once, in order.
    assert isolated["masks"] == {
        "masks": [*base, *_git_masks(home)],
        "git_config_policy": "isolated",
        "git_config_masks": _git_masks(home),
    }
    assert isolated["view"]["git_config_masks"] == _git_masks(home)  # type: ignore[index]
    # The writable set is a separate member and is not changed by the policy.
    assert isolated["writable"] == inherited["writable"]

    with pytest.raises(_Execed) as caught:
        sandbox.enter_in_place(
            view, settings, ["true"], git_config_policy="isolated",
            expected_masks=isolated["masks"], expected_writable=isolated["writable"],  # type: ignore[arg-type]
            **request,  # type: ignore[arg-type]
        )
    actual = caught.value.spec["view"]
    assert isinstance(actual, dict)
    assert sandbox.mask_report(actual) == isolated["masks"]
    assert actual["git_config_policy"] == "isolated"


def test_isolated_policy_masks_every_home_alias_and_a_path_already_hidden_once(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    home, target, _cgroup, _log = _in_scope_host(tmp_path, monkeypatch)
    alias = tmp_path / "alias-home"
    monkeypatch.setattr(sandbox, "path_aliases", lambda path: [path, alias])
    settings = dataclasses.replace(
        _closed_settings(target), home_hidden=(*sandbox.DEFAULT_HOME_HIDDEN, ".gitconfig")
    )

    spec = sandbox.build_spec(_slot_view(tmp_path), settings, home, tmp_path, git_config_policy="isolated")

    git_masks = [str(home / ".gitconfig"), str(home / ".config/git"),
                 str(alias / ".gitconfig"), str(alias / ".config/git"), "/etc/gitconfig"]
    assert spec["git_config_masks"] == git_masks
    masks = spec["masks"]
    assert isinstance(masks, list)
    assert len(masks) == len(set(masks))  # each path once, even when also in home_hidden
    assert all(path in masks for path in git_masks)
    with pytest.raises(sandbox.SandboxError, match="--git-config-policy must be one of"):
        sandbox.build_spec(_slot_view(tmp_path), settings, home, tmp_path, git_config_policy="arbitrary")


def test_entry_refuses_masks_other_than_the_reported_ones(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Predicted masks must equal actual masks, before any state write and again at the spec."""

    home, target, cgroup, log = _in_scope_host(tmp_path, monkeypatch)
    monkeypatch.setattr(sandbox, "path_aliases", lambda path: [path])
    view = _slot_view(tmp_path)
    settings = _closed_settings(target)
    request = _policy_request(cgroup)
    reported = sandbox.in_scope_plan(
        view, settings, ["true"], git_config_policy="isolated", **request  # type: ignore[arg-type]
    )["masks"]
    assert isinstance(reported, dict)

    # The policy dropped between --print and the entry: refused before anything is written.
    with pytest.raises(sandbox.SandboxError, match="masks this entry would apply differ"):
        sandbox.enter_in_place(
            view, settings, ["true"], expected_masks=reported, **request  # type: ignore[arg-type]
        )
    assert not (tmp_path / "state").exists()
    assert not any(line.startswith("sudo") for line in _calls(log))

    # Every member is compared: each policy path omitted, the list reordered,
    # an extra mask, another policy name, an unknown member.
    masks = list(reported["masks"])
    variants = [
        {**reported, "masks": [path for path in masks if path != omitted]}
        for omitted in _git_masks(home)
    ]
    variants += [
        {**reported, "git_config_masks": _git_masks(home)[:-1]},
        {**reported, "masks": list(reversed(masks))},
        {**reported, "masks": [*masks, "/etc/other"]},
        {**reported, "git_config_policy": "inherit"},
        {**reported, "extra": 1},
    ]
    for changed in variants:
        with pytest.raises(sandbox.SandboxError, match="masks this entry would apply differ"):
            sandbox.in_scope_plan(
                view, settings, ["true"], git_config_policy="isolated", expected_masks=changed,
                **request,  # type: ignore[arg-type]
            )


def test_entry_compares_the_masks_again_from_the_spec_it_writes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A mask list that changes after the first comparison is refused before the spec is written."""

    home, target, cgroup, _log = _in_scope_host(tmp_path, monkeypatch)
    alias = tmp_path / "late-alias"
    calls: list[int] = []

    def aliases(path: Path) -> list[Path]:
        calls.append(1)
        return [path] if len(calls) <= 2 else [path, alias]  # print, first check; then drift

    monkeypatch.setattr(sandbox, "path_aliases", aliases)
    view = _slot_view(tmp_path)
    settings = _closed_settings(target)
    request = _policy_request(cgroup)
    reported = sandbox.in_scope_plan(
        view, settings, ["true"], git_config_policy="isolated", **request  # type: ignore[arg-type]
    )["masks"]
    written: list[object] = []

    def write_root_spec(*args: object) -> str:
        written.append(args)
        return "/nonexistent"

    monkeypatch.setattr(sandbox, "write_root_spec", write_root_spec)

    with pytest.raises(sandbox.SandboxError, match="masks this entry would apply differ"):
        sandbox.enter_in_place(
            view, settings, ["true"], git_config_policy="isolated",
            expected_masks=reported, **request,  # type: ignore[arg-type]
        )
    assert written == []
    assert len(calls) == 3


@pytest.mark.parametrize(
    ("extra", "message"),
    [
        (["--git-config-policy", "isolated", "--", "true"], "--git-config-policy applies only with --in-scope"),
        (["--expect-masks", "{}", "--", "true"], "--expect-masks applies only with --in-scope"),
        (["--place-only", "--git-config-policy", "isolated"],
         "--place-only runs nothing and takes no --git-config-policy"),
        (["--place-only", "--expect-masks", "{}"], "--place-only runs nothing and takes no --expect-masks"),
        (
            ["--in-scope", "run-1.scope", "--expect-slices",
             json.dumps({"enclosing": None, "all_slots": "wrkslots.slice", "slot": "wrkslots-slot\\x2db.slice"}),
             "--expect-masks", "[1]", "--", "true"],
            "--expect-masks must be the JSON object --print reported as masks",
        ),
    ],
)
def test_run_refuses_policy_options_where_they_cannot_apply(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    extra: list[str],
    message: str,
) -> None:
    root = _registered_slot(tmp_path)
    log = _fake_systemd_tools(tmp_path, monkeypatch)

    def must_not_run(*args: object, **kwargs: object) -> None:
        # A request that is not refused must not reach a real scope, namespace or exec.
        raise AssertionError("a request that must be refused reached the box")

    monkeypatch.setattr(sandbox, "run", must_not_run)
    monkeypatch.setattr(sandbox, "enter_in_place", must_not_run)
    monkeypatch.setattr(os, "execvpe", must_not_run)
    capsys.readouterr()

    assert cli.main(["--project-root", str(root), "run", "slot-b", *extra]) != 0

    assert message in capsys.readouterr().err
    assert _calls(log) == []


def test_run_in_scope_print_reports_masks_and_expect_masks_binds_them(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _registered_slot(tmp_path)
    home, _target, cgroup, _log = _in_scope_host(tmp_path, monkeypatch)
    target = root / "worktrees" / "slot-b" / "target"
    target.mkdir()
    cgroup.write_text(
        _MANAGER + "/app.slice/app-wrkslots.slice/app-wrkslots-slot\\x2db.slice/run-1.scope\n"
    )
    real = sandbox.in_scope_plan

    def plan(*args: object, **kwargs: object) -> dict[str, object]:
        return real(*args, **kwargs, cgroup_file=cgroup)  # type: ignore[arg-type]

    monkeypatch.setattr(sandbox, "in_scope_plan", plan)
    slices = json.dumps(
        {"enclosing": "app.slice", "all_slots": "app-wrkslots.slice", "slot": "app-wrkslots-slot\\x2db.slice"}
    )
    base = ["--project-root", str(root), "run", "slot-b", "--in-scope", "run-1.scope",
            "--expect-slices", slices, "--isolation", "root", "--closed-set",
            "--home-private", ".cargo", "--read-write", str(target), "--tmp-size", "1G",
            "--git-config-policy", "isolated"]
    capsys.readouterr()
    assert cli.main([*base, "--print", "--", "true"]) == 0
    report = json.loads(capsys.readouterr().out)
    masks = report["masks"]
    assert masks["git_config_policy"] == "isolated"
    assert masks["git_config_masks"][:2] == [str(home / ".gitconfig"), str(home / ".config/git")]
    assert masks["git_config_masks"][-1] == "/etc/gitconfig"
    assert masks["masks"] == report["view"]["masks"]

    assert cli.main([*base, "--expect-masks", json.dumps(masks), "--print", "--", "true"]) == 0
    capsys.readouterr()
    narrowed = {**masks, "masks": [path for path in masks["masks"] if path != "/etc/gitconfig"]}
    assert cli.main([*base, "--expect-masks", json.dumps(narrowed), "--print", "--", "true"]) != 0
    assert "masks this entry would apply differ" in capsys.readouterr().err


# ---- per-path decisions inside the view (STUB: mount calls are recorded, not made)


def _record_mounts(monkeypatch: pytest.MonkeyPatch) -> list[tuple[object, ...]]:
    mounts: list[tuple[object, ...]] = []
    monkeypatch.setattr(sandbox, "_mount", lambda *args: mounts.append(("mount", *args)))
    monkeypatch.setattr(sandbox, "_remount_read_only", lambda *args: mounts.append(("remount-ro", *args)))
    return mounts


def test_mask_git_config_masks_the_path_itself(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    mounts = _record_mounts(monkeypatch)
    file, directory, absent = tmp_path / "gitconfig", tmp_path / "git", tmp_path / "missing" / "gitconfig"
    file.write_text("[core]\n\tfsmonitor = sentinel\n")
    directory.mkdir()
    (directory / "config").write_text("[include]\n\tpath = sentinel\n")

    assert sandbox._mask_git_config(str(file)) == "file"
    assert sandbox._mask_git_config(str(directory)) == "directory"
    assert sandbox._mask_git_config(str(absent)) == "absent"

    assert mounts[0][:3] == ("mount", "/dev/null", str(file))
    assert mounts[1] == ("remount-ro", str(file))
    assert mounts[2][1:4] == ("tmpfs", str(directory), "tmpfs")
    assert int(mounts[2][4]) & sandbox._MS_RDONLY  # type: ignore[call-overload]
    assert len(mounts) == 3  # nothing is mounted for the absent path


@pytest.mark.parametrize("kind", ["symlink", "dangling", "parent-symlink", "fifo"])
def test_mask_git_config_refuses_links_and_specials_without_following(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, kind: str
) -> None:
    mounts = _record_mounts(monkeypatch)
    elsewhere = tmp_path / "elsewhere"
    elsewhere.mkdir()
    (elsewhere / "gitconfig").write_text("[user]\n\tname = not git config at all\n")
    home = tmp_path / "home"
    home.mkdir()
    if kind == "symlink":
        path = home / ".gitconfig"
        path.symlink_to(elsewhere / "gitconfig")
    elif kind == "dangling":
        path = home / ".gitconfig"
        path.symlink_to(tmp_path / "nowhere")
    elif kind == "parent-symlink":
        (home / ".config").symlink_to(elsewhere)
        (elsewhere / "git").mkdir()
        path = home / ".config" / "git"
    else:
        path = home / ".gitconfig"
        os.mkfifo(path)

    with pytest.raises(sandbox.SandboxError, match="git-config-policy isolated"):
        sandbox._mask_git_config(str(path))
    assert mounts == []  # in particular, nothing was mounted on the link's target

    # The ordinary mask, by contrast, follows the link (unchanged behaviour).
    if kind == "symlink":
        sandbox._mask(str(path))
        assert mounts[0][:3] == ("mount", "/dev/null", str(elsewhere / "gitconfig"))


def test_mask_git_config_refuses_every_error(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    file = tmp_path / "gitconfig"
    file.write_text("x")

    def failing(*args: object) -> None:
        raise sandbox.MountError("mount /dev/null on gitconfig: Operation not permitted", 1)

    monkeypatch.setattr(sandbox, "_mount", failing)
    with pytest.raises(sandbox.SandboxError, match="cannot mask"):
        sandbox._mask_git_config(str(file))

    real_lstat = os.lstat

    def denied(path: object, *args: object, **kwargs: object) -> os.stat_result:
        if str(path) == str(file):
            raise PermissionError(13, "Permission denied")
        return real_lstat(path, *args, **kwargs)  # type: ignore[arg-type]

    monkeypatch.setattr(os, "lstat", denied)
    with pytest.raises(sandbox.SandboxError, match="cannot inspect"):
        sandbox._mask_git_config(str(file))
    with pytest.raises(sandbox.SandboxError, match="normalized absolute path"):
        sandbox._mask_git_config("relative/gitconfig")


def _Status(mode: int, rdev: int) -> os.stat_result:
    return os.stat_result((mode, 0, 0, 0, 0, 0, 0, 0, 0, 0), {"st_rdev": rdev})


def test_confirm_git_config_masks_checks_each_outcome_inside_the_view() -> None:
    """STUB observations: what the finished view must show for each outcome."""

    null_rdev = os.stat("/dev/null").st_rdev
    seen = {
        "/v/file": _Status(stat.S_IFCHR | 0o666, null_rdev),
        "/v/dir": _Status(stat.S_IFDIR | 0o700, 0),
    }

    def lstat(path: str) -> os.stat_result:
        if path in seen:
            return seen[path]
        raise FileNotFoundError(2, "No such file or directory", path)

    def confirm(outcomes: list[tuple[str, str]], *, empty: bool = True, read_only: bool = True) -> None:
        sandbox._confirm_git_config_masks(
            outcomes, lstat=lstat, listdir=lambda path: [] if empty else ["config"],
            read_only=lambda path: read_only,
        )

    confirm([("/v/file", "file"), ("/v/dir", "directory"), ("/v/absent", "absent")])
    for outcomes, keywords in (
        ([("/v/absent", "file")], {}),                 # the file mask is not there
        ([("/v/dir", "file")], {}),
        ([("/v/file", "absent")], {}),                 # an absent path appeared before COMMAND
        ([("/v/dir", "directory")], {"empty": False}),  # the directory still shows its config
        ([("/v/dir", "directory")], {"read_only": False}),
        ([("/v/file", "file")], {"read_only": False}),
        ([("/v/file", "unknown")], {}),
    ):
        with pytest.raises(sandbox.SandboxError, match="is not .* inside the view"):
            confirm(outcomes, **keywords)
    seen["/v/file"] = _Status(stat.S_IFREG | 0o644, 0)  # an unmasked regular file
    with pytest.raises(sandbox.SandboxError, match="is not file inside the view"):
        confirm([("/v/file", "file")])


def test_build_view_masks_policy_paths_strictly_and_confirms_them(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """STUB view: every mount step is recorded; the order and the dispatch are what is checked."""

    events: list[tuple[object, ...]] = []
    for name in ("_mount", "_bind_fd", "_remount_read_only", "_limit_propagation"):
        monkeypatch.setattr(sandbox, name, lambda *args, name=name: events.append((name, *args)))
    monkeypatch.setattr(sandbox, "_remount_tree_read_only", lambda *args, **kwargs: [])
    monkeypatch.setattr(sandbox, "_mask", lambda path: events.append(("mask", path)))

    def git_mask(path: str) -> str:
        events.append(("git-mask", path))
        return "file"

    monkeypatch.setattr(sandbox, "_mask_git_config", git_mask)
    monkeypatch.setattr(
        sandbox, "_confirm_git_config_masks", lambda outcomes: events.append(("confirm", list(outcomes)))
    )
    monkeypatch.setattr(os, "chdir", lambda path: None)
    home, layer = tmp_path / "home", tmp_path / "layer"
    home.mkdir()
    layer.mkdir()
    spec: dict[str, object] = {
        "home": str(home), "home_layer": str(layer), "home_mode": "ro", "home_aliases": [],
        "home_binds": [], "read_only": [], "binds": [], "tmp_size": "1G", "protect_system": True,
        "propagate": [], "cwd": str(tmp_path),
        "masks": [str(home / ".ssh"), "/etc/gitconfig", str(home / ".gitconfig")],
        "git_config_policy": "isolated",
        "git_config_masks": [str(home / ".gitconfig"), "/etc/gitconfig"],
    }

    sandbox.build_view(spec)

    masking = [event for event in events if event[0] in ("mask", "git-mask", "confirm")]
    assert masking == [
        ("mask", str(home / ".ssh")),
        ("git-mask", "/etc/gitconfig"),
        ("git-mask", str(home / ".gitconfig")),
        ("confirm", [("/etc/gitconfig", "file"), (str(home / ".gitconfig"), "file")]),
    ]

    # A policy path the masks do not carry is refused, not skipped.
    events.clear()
    with pytest.raises(sandbox.SandboxError, match="missing from the view's masks"):
        sandbox.build_view({**spec, "masks": [str(home / ".ssh"), "/etc/gitconfig"]})
    assert not any(event[0] == "confirm" for event in events)

    # Without the policy the view is built as before: no strict mask, no confirmation.
    events.clear()
    plain = {key: value for key, value in spec.items() if not key.startswith("git_config")}
    sandbox.build_view(plain)
    assert [event for event in events if event[0] in ("mask", "git-mask", "confirm")] == [
        ("mask", str(home / ".ssh")), ("mask", "/etc/gitconfig"), ("mask", str(home / ".gitconfig")),
    ]


# ---- the Git lookup before the view (real git; hostile caller environment)


def _hostile_git_environment(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    """A `git` first on PATH that records each call, plus hostile GIT_* and loader variables."""

    tools = tmp_path / "hostile-bin"
    tools.mkdir()
    log = tmp_path / "hostile-git.log"
    (tools / "git").write_text(f'#!/bin/sh\nprintf "%s\\n" "$*" >> {log}\nexec /usr/bin/git "$@"\n')
    (tools / "git").chmod(0o755)
    monkeypatch.setenv("PATH", f"{tools}{os.pathsep}{os.environ.get('PATH', '')}")
    monkeypatch.setenv("GIT_DIR", str(tmp_path / "not-a-repository"))
    monkeypatch.setenv("GIT_CONFIG_PARAMETERS", "'core.fsmonitor=/bin/false'")
    monkeypatch.setenv("GIT_EXEC_PATH", str(tools))
    monkeypatch.setenv("LD_LIBRARY_PATH", str(tmp_path / "hostile-lib"))
    monkeypatch.setenv("XDG_CONFIG_HOME", str(tmp_path / "hostile-xdg"))
    return log


def test_isolated_lookup_before_the_view_inherits_nothing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _registered_slot(tmp_path)
    config = cli._load_config(str(root), "node-a")
    ordinary = imagecmd.slot_view(config, "slot-b", "agent")
    log = _hostile_git_environment(tmp_path, monkeypatch)
    calls: list[tuple[list[str], dict[str, str]]] = []
    real_run = subprocess.run

    def recording(command: list[str], *args: object, **kwargs: object) -> object:
        calls.append((list(command), dict(kwargs.get("env") or {})))  # type: ignore[call-overload]
        return real_run(command, *args, **kwargs)  # type: ignore[call-overload]

    monkeypatch.setattr(subprocess, "run", recording)

    isolated = imagecmd.slot_view(config, "slot-b", "agent", isolated_git=True)

    assert isolated.git_directories == ordinary.git_directories != ()
    assert not log.exists()  # the `git` on PATH was never run
    assert calls and all(command[0] == "/usr/bin/git" for command, _env in calls)
    assert all(env == imagecmd.ISOLATED_GIT_ENVIRONMENT for _command, env in calls)

    # The ordinary lookup is unchanged: it still runs `git` from PATH.
    monkeypatch.setattr(subprocess, "run", real_run)
    imagecmd.slot_view(config, "slot-b", "agent")
    assert log.exists() and "rev-parse" in log.read_text()


def test_isolated_lookup_refuses_an_unverified_git_executable(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _registered_slot(tmp_path)
    config = cli._load_config(str(root), "node-a")
    mine = tmp_path / "git"
    mine.write_text("#!/bin/sh\nexit 0\n")
    mine.chmod(0o755)
    for candidate in (str(mine), str(tmp_path / "absent-git")):
        monkeypatch.setattr(imagecmd, "ISOLATED_GIT_EXECUTABLE", candidate)
        with pytest.raises(cli.Refusal, match="git-config-policy isolated"):
            imagecmd.slot_view(config, "slot-b", "agent", isolated_git=True)
