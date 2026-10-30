"""Installed command surface tests for wrkslots."""

from __future__ import annotations

import argparse
import dataclasses
import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path

import pytest
from wrkslots import cli, sandbox, slotimage


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
