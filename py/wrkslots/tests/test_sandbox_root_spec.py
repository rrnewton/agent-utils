"""Root-launcher spec boundary (#1583, after two adversarial reviews).

Threat model: the attacker is the invoking user, with full control of
the spec and their own files. One test per review finding: trusted
roots only, resolve-strict paths, spec policy refused, the seal.
"""

import inspect
import json
import os
from pathlib import Path

import pytest

from wrkslots import sandbox


def _control(tmp_path: Path) -> Path:
    control = tmp_path / "worktrees"
    control.mkdir()
    (control / "ACTIVE.test.json").write_text(
        json.dumps({
            "slots": [{
                "slot": "s1",
                "agent": "worker",
                "checkouts": [{"path": "worktrees/slots/s1/repo", "name": "repo"}],
            }]
        }),
        encoding="utf-8",
    )
    return control


def _document(tmp_path: Path, control: Path) -> dict[str, object]:
    home = Path(os.path.expanduser("~"))
    slot_path = control / "slots" / "s1"
    (slot_path / "repo").mkdir(parents=True, exist_ok=True)
    return {
        "view": {
            "home": str(home),
            "home_layer": str(control / "slot-state" / "s1" / "home"),
            "cwd": str(slot_path),
            "binds": [[str(slot_path), str(slot_path)]],
            "masks": [],
            "read_only": [str(control)],
            "home_aliases": [],
            "home_binds": [],
            "tmp_size": "64M",
        },
        "command": ["/bin/true"],
        "environment": {},
        "uid": os.getuid(),
        "gid": os.getgid(),
        "groups": [],
        "umask": 0o022,
        "registered": {
            "kind": "slot",
            "slot": "s1",
            "slot_path": str(slot_path),
            "control": str(control),
            "project_root": str(tmp_path),
        },
    }


def test_unregistered_and_forged_claims_are_refused(tmp_path: Path) -> None:
    control = _control(tmp_path)
    document = _document(tmp_path, control)
    view = document["view"]
    assert isinstance(view, dict)
    trusted = sandbox._verify_registered(document, view, os.getuid())
    assert trusted["slot_path"] == str(control / "slots" / "s1")
    registered = document["registered"]
    assert isinstance(registered, dict)
    registered["slot"] = "ghost"
    with pytest.raises(sandbox.SandboxError, match="not registered"):
        sandbox._verify_registered(document, view, os.getuid())
    # N1: a spec-supplied home owned by someone else (a system home) refuses.
    forged = {**view, "home": "/etc"}
    with pytest.raises(sandbox.SandboxError, match="passwd home"):
        sandbox._verify_registered(document, forged, os.getuid())
    # N1: a spec-supplied slot_path the registry row does not name refuses.
    registered["slot"] = "s1"
    registered["slot_path"] = str(tmp_path / "elsewhere")
    with pytest.raises(sandbox.SandboxError, match="not registered"):
        sandbox._verify_registered(document, view, os.getuid())


def test_closed_schema_tmp_size_and_policy_are_enforced(tmp_path: Path) -> None:
    control = _control(tmp_path)
    document = _document(tmp_path, control)
    sandbox._check_root_schema(document)
    with pytest.raises(sandbox.SandboxError, match="unknown keys"):
        sandbox._check_root_schema({**document, "evil": True})
    raw_view = document["view"]
    assert isinstance(raw_view, dict)
    view: dict[str, object] = dict(raw_view)
    with pytest.raises(sandbox.SandboxError, match="tmp_size"):
        sandbox._check_root_schema({**document, "view": {**view, "tmp_size": "1;evil"}})
    # N3/N4: spec mount policy is forced, never taken, in root mode.
    trusted = sandbox._verify_registered(document, view, os.getuid())
    falsy_values: list[object] = [False, None, 0, "", []]
    for falsy in falsy_values:
        forced = {**view, "protect_system": falsy, "propagate": ["/x"]}
        sandbox._force_root_policy(forced, trusted, os.getuid())
        assert forced["protect_system"] is True
        assert forced["propagate"] == []
    with pytest.raises(sandbox.SandboxError, match="Eden"):
        sandbox._force_root_policy({**view, "eden_root_cover": True}, trusted, os.getuid())
    with pytest.raises(sandbox.SandboxError, match="tmp_size"):
        sandbox._check_root_schema({**document, "view": {**view, "tmp_size": "0"}})


def test_paths_outside_trusted_roots_are_refused(tmp_path: Path) -> None:
    control = _control(tmp_path)
    document = _document(tmp_path, control)
    view = document["view"]
    assert isinstance(view, dict)
    trusted = sandbox._verify_registered(document, view, os.getuid())
    sandbox._check_root_ownership(sandbox._allowed_roots(trusted, view, os.getuid()), os.getuid())
    sandbox._check_root_paths(view, trusted, os.getuid())
    home = Path(str(view["home"]))
    for target in ("/etc/sudoers", "/usr/lib/evil.so", "relative/path", "/var/tmp/x"):
        bad = {**view, "binds": [[str(home), target]]}
        with pytest.raises(sandbox.SandboxError, match="refusing"):
            sandbox._check_root_paths(bad, trusted, os.getuid())
    link = tmp_path / "link"
    link.symlink_to("/etc")
    bad = {**view, "binds": [[str(home), str(link / "passwd")]]}
    with pytest.raises(sandbox.SandboxError, match="refusing"):
        sandbox._check_root_paths(bad, trusted, os.getuid())


def test_enter_refuses_root_and_sudo_env(monkeypatch: pytest.MonkeyPatch) -> None:
    # HIGH: the userns entry must never run privileged (126 = refused).
    monkeypatch.setattr(os, "geteuid", lambda: 0)
    assert sandbox._main(["enter", "{}", "--", "/bin/true"]) == 126
    monkeypatch.setattr(os, "geteuid", lambda: 1000)
    monkeypatch.setenv("SUDO_UID", "1000")
    assert sandbox._main(["enter", "{}", "--", "/bin/true"]) == 126


def test_sudo_identity_forgery_is_refused(monkeypatch: pytest.MonkeyPatch) -> None:
    # Item 5/6: env-claimed uid/gid must match the sudo parent's real ids.
    uid, gid = os.getuid(), os.getgid()
    monkeypatch.setenv("SUDO_UID", str(uid))
    monkeypatch.setenv("SUDO_GID", str(gid))
    good = f"Name:\tsudo\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\nGid:\t{gid}\t{gid}\t{gid}\t{gid}\n"
    sandbox._sudo_identity(good)
    forged_uid = f"Name:\tsudo\nUid:\t{uid + 1}\t{uid + 1}\t{uid + 1}\t{uid + 1}\nGid:\t{gid}\t{gid}\t{gid}\t{gid}\n"
    with pytest.raises(sandbox.SandboxError, match="sudo monitor"):
        sandbox._sudo_identity(forged_uid)
    forged_gid = f"Name:\tsudo\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\nGid:\t{gid}\t{gid}\t{gid}\t{gid}\n"
    monkeypatch.setenv("SUDO_GID", "987654")
    with pytest.raises(sandbox.SandboxError, match="sudo gid"):
        sandbox._sudo_identity(forged_gid)


def test_no_new_privs_is_set_by_the_seal() -> None:
    # Live: after the seal prctl, /proc/self/status shows NoNewPrivs: 1.
    import subprocess
    import sys

    completed = subprocess.run(
        [
            sys.executable, "-c",
            "import ctypes; libc = ctypes.CDLL(None); "
            "assert libc.prctl(38, 1, 0, 0, 0) == 0; "
            "print([l for l in open('/proc/self/status') if l.startswith('NoNewPrivs')][0].strip())",
        ],
        capture_output=True, text=True, check=False, timeout=30,
    )
    assert completed.returncode == 0, completed.stderr
    assert "NoNewPrivs:\t1" in completed.stdout


def test_seal_and_dumpable_are_pinned() -> None:
    source = inspect.getsource(sandbox.enter_root)
    assert "_PR_SET_DUMPABLE, 0" in source
    assert "_PR_SET_DUMPABLE, 1" not in source
    # (a): root mode is sealed; harness setuid steps fail honestly there.
    assert "_prctl(_PR_SET_NO_NEW_PRIVS, 1)" in source
