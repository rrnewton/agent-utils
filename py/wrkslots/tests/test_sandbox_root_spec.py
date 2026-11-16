"""Root-launcher spec boundary (#1583, redesign after adversarial review).

Threat model: the attacker is the invoking user, with full control of
the spec and their own files. The helper must refuse anything that
would turn that control into root: unregistered slots, unknown schema
keys, paths outside the allowed roots, symlink-smuggled targets, and
option-string injection through tmp_size.
"""

import json
import os
from pathlib import Path

import pytest

from wrkslots import sandbox


def _control(tmp_path: Path) -> Path:
    control = tmp_path / "worktrees"
    control.mkdir()
    (control / "ACTIVE.test.json").write_text(
        json.dumps({"slots": [{"slot": "s1", "agent": "worker"}]}), encoding="utf-8"
    )
    return control


def _document(tmp_path: Path, control: Path) -> dict[str, object]:
    home = tmp_path / "home"
    home.mkdir(exist_ok=True)
    slot_path = control / "slots" / "s1"
    slot_path.mkdir(parents=True, exist_ok=True)
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
        },
    }


def test_unregistered_slot_is_refused(tmp_path: Path) -> None:
    control = _control(tmp_path)
    document = _document(tmp_path, control)
    registered = document["registered"]
    assert isinstance(registered, dict)
    registered["slot"] = "ghost"
    with pytest.raises(sandbox.SandboxError, match="not registered"):
        sandbox._verify_registered(document, os.getuid())


def test_registered_slot_and_box_are_accepted(tmp_path: Path) -> None:
    control = _control(tmp_path)
    document = _document(tmp_path, control)
    sandbox._verify_registered(document, os.getuid())
    sandbox._verify_registered(
        {"registered": {"kind": "box", "control": str(control)}}, os.getuid()
    )
    with pytest.raises(sandbox.SandboxError, match="no registered slot"):
        sandbox._verify_registered({}, os.getuid())


def test_closed_schema_and_tmp_size_are_enforced(tmp_path: Path) -> None:
    control = _control(tmp_path)
    document = _document(tmp_path, control)
    sandbox._check_root_schema(document)
    with pytest.raises(sandbox.SandboxError, match="unknown keys"):
        sandbox._check_root_schema({**document, "evil": True})
    raw_view = document["view"]
    assert isinstance(raw_view, dict)
    view: dict[str, object] = dict(raw_view)
    with pytest.raises(sandbox.SandboxError, match="unknown keys"):
        sandbox._check_root_schema({**document, "view": {**view, "evil": True}})
    with pytest.raises(sandbox.SandboxError, match="tmp_size"):
        sandbox._check_root_schema({**document, "view": {**view, "tmp_size": "1;evil"}})
    with pytest.raises(sandbox.SandboxError, match="tmp_size"):
        sandbox._check_root_schema({**document, "view": {**view, "tmp_size": "99999G"}})


def test_dumpable_stays_zero_while_capabilities_are_held() -> None:
    # Addition 1: dumpable is 0 from entry and never raised to 1 on the
    # root entry path (a dumpable root helper is ptraceable by the user).
    import inspect

    source = inspect.getsource(sandbox.enter_root)
    assert "_PR_SET_DUMPABLE, 0" in source
    assert "_PR_SET_DUMPABLE, 1" not in source
    assert "_prctl(_PR_SET_NO_NEW_PRIVS" not in source  # setuid-capable by design


def test_paths_outside_allowed_roots_are_refused(tmp_path: Path) -> None:
    control = _control(tmp_path)
    document = _document(tmp_path, control)
    view = document["view"]
    assert isinstance(view, dict)
    sandbox._check_root_paths(document, view)
    home = Path(str(view["home"]))
    for target in ("/etc/sudoers", "/usr/lib/evil.so", "relative/path", "/var/tmp/x"):
        bad = {**view, "binds": [[str(home), target]]}
        with pytest.raises(sandbox.SandboxError, match="refusing"):
            sandbox._check_root_paths(document, bad)
    # A symlink that resolves outside the allowed roots is refused too.
    link = home / "link"
    link.symlink_to("/etc")
    bad = {**view, "binds": [[str(home), str(link / "passwd")]]}
    with pytest.raises(sandbox.SandboxError, match="refusing"):
        sandbox._check_root_paths(document, bad)
