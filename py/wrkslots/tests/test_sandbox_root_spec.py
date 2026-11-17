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


def test_tmp_percent_form_is_capped_like_the_absolute_form(tmp_path: Path) -> None:
    # I5: "100%" of a large host is far over the 64 GiB absolute cap.
    control = _control(tmp_path)
    document = _document(tmp_path, control)
    view = document["view"]
    assert isinstance(view, dict)
    host = 128 * (1 << 30)
    with pytest.raises(sandbox.SandboxError, match="tmp_size"):
        sandbox._check_root_schema(
            {**document, "view": {**view, "tmp_size": "100%"}}, host_memory_bytes=host
        )
    sandbox._check_root_schema(
        {**document, "view": {**view, "tmp_size": "50%"}}, host_memory_bytes=host
    )


def test_root_schema_type_checks_every_view_value(tmp_path: Path) -> None:
    # I1b: a non-list masks (or any wrongly typed value) refuses at the
    # schema, not as a late assert inside build_view.
    control = _control(tmp_path)
    document = _document(tmp_path, control)
    view = document["view"]
    assert isinstance(view, dict)
    for key, bad in (
        ("home", 123),
        ("home_layer", None),
        ("masks", "not-a-list"),
        ("home_binds", ".config"),
        ("binds", [["/a", "/b"], ["/a", 7]]),
        ("binds", "/a:/b"),
        ("protect_system", "yes"),
        ("eden_backing", 42),
    ):
        with pytest.raises(sandbox.SandboxError, match=key):
            sandbox._check_root_schema({**document, "view": {**view, key: bad}})


def test_verify_mount_points_checks_the_source_device(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # I2: a bind whose point shows some other inode refuses, even
    # though something is mounted at its point.
    source = tmp_path / "source"
    other = tmp_path / "other"
    source.mkdir()
    other.mkdir()
    view: dict[str, object] = {
        "home": str(tmp_path),
        "binds": [[str(source), str(other)]],
        "masks": [],
    }
    rows = [("1:1", "/", tmp_path), ("2:2", "/", other), ("3:3", "/", Path("/tmp"))]
    monkeypatch.setattr(sandbox, "_mount_rows", lambda: rows)
    with pytest.raises(sandbox.SandboxError, match="do not match"):
        sandbox._verify_mount_points(view)
    view["binds"] = [[str(source), str(source)]]  # its own point shows it
    rows.append(("2:2", "/", source))
    sandbox._verify_mount_points(view)


def test_resolve_fd_stays_beneath_its_root(tmp_path: Path) -> None:
    # I1: resolution is openat2 BENEATH|NO_SYMLINKS from the root fd.
    (tmp_path / "real").mkdir()
    descriptor = sandbox._resolve_fd(str(tmp_path / "real"), [str(tmp_path)])
    try:
        assert os.fstat(descriptor).st_ino == os.stat(tmp_path / "real").st_ino
    finally:
        os.close(descriptor)
    (tmp_path / "link").symlink_to("/etc")
    with pytest.raises(sandbox.SandboxError):
        sandbox._resolve_fd(str(tmp_path / "link" / "passwd"), [str(tmp_path)])
    created = sandbox._resolve_fd(str(tmp_path / "made"), [str(tmp_path)], create="dir")
    os.close(created)
    assert (tmp_path / "made").is_dir()


def test_fd_mounts_survive_source_and_target_swaps(tmp_path: Path) -> None:
    # I1: inside a throwaway user namespace, bind by resolved fd after
    # both names are swapped: the fd, not the name, is mounted.
    import subprocess
    import sys

    py = str(Path(__file__).resolve().parents[2])
    script = r'''
import os, sys
sys.path.insert(0, sys.argv[1])
from wrkslots import sandbox
base = sys.argv[2]
uid, gid = os.getuid(), os.getgid()
sandbox._check(sandbox._libc().unshare(sandbox._CLONE_NEWUSER | sandbox._CLONE_NEWNS), "unshare")
sandbox._write("/proc/self/setgroups", "deny")
sandbox._write("/proc/self/uid_map", f"{uid} {uid} 1\n")
sandbox._write("/proc/self/gid_map", f"{gid} {gid} 1\n")

def put(path, text):
    os.makedirs(path, exist_ok=True)
    with open(os.path.join(path, "marker"), "w", encoding="utf-8") as handle:
        handle.write(text)

src_a, src_b = os.path.join(base, "srcA"), os.path.join(base, "srcB")
dst, dst2 = os.path.join(base, "dst"), os.path.join(base, "dst2")
victim = os.path.join(base, "victim")
put(src_a, "A"); put(src_b, "B"); put(victim, "V")
os.makedirs(dst); os.makedirs(dst2)
roots = [base]
# Source swap: resolve A, then put B at A's name; the bind mounts A.
fd_a = sandbox._resolve_fd(src_a, roots)
os.rename(src_a, os.path.join(base, "oldA"))
os.rename(src_b, src_a)
sandbox._bind_fd_strict(fd_a, dst, roots)
os.close(fd_a)
assert open(os.path.join(dst, "marker"), encoding="utf-8").read() == "A"
# Target swap: resolve dst2, replace its name with a symlink to the
# victim; the mount lands on the resolved (renamed) directory only.
fd_t = sandbox._resolve_fd(dst2, roots)
os.rename(dst2, os.path.join(base, "dst2old"))
os.symlink(victim, dst2)
fd_b = sandbox._resolve_fd(os.path.join(base, "oldA"), roots)
sandbox._bind_resolved_fd(fd_b, fd_t)
os.close(fd_b); os.close(fd_t)
assert open(os.path.join(base, "dst2old", "marker"), encoding="utf-8").read() == "A"
assert not os.path.ismount(victim)
assert open(os.path.join(victim, "marker"), encoding="utf-8").read() == "V"
print("FD-MOUNTS-OK")
'''
    completed = subprocess.run(
        [sys.executable, "-c", script, py, str(tmp_path)],
        capture_output=True, text=True, check=False, timeout=60,
    )
    if completed.returncode != 0 and "unshare" in completed.stderr:
        pytest.skip("user namespaces unavailable on this host")
    assert completed.returncode == 0, completed.stderr
    assert "FD-MOUNTS-OK" in completed.stdout


def test_strict_fd_build_view_in_a_user_namespace() -> None:
    # I1/I2 end-to-end for the strict builder without sudo: build a
    # real view with fd mounts inside a throwaway user namespace and
    # check the visible result (binds show, masks hide, /tmp is fresh).
    import subprocess
    import sys

    py = str(Path(__file__).resolve().parents[2])
    script = r'''
import os, sys
sys.path.insert(0, sys.argv[1])
from wrkslots import sandbox
base = sys.argv[2]
uid, gid = os.getuid(), os.getgid()
sandbox._check(sandbox._libc().unshare(sandbox._CLONE_NEWUSER | sandbox._CLONE_NEWNS), "unshare")
sandbox._write("/proc/self/setgroups", "deny")
sandbox._write("/proc/self/uid_map", f"{uid} {uid} 1\n")
sandbox._write("/proc/self/gid_map", f"{gid} {gid} 1\n")
home = os.path.join(base, "home")
spec = {
    "home": home,
    "home_layer": os.path.join(base, "layer"),
    "home_mode": "ro",
    "home_aliases": [],
    "home_binds": [],
    "read_only": [],
    "binds": [[os.path.join(base, "slot"), os.path.join(base, "slot")]],
    "masks": [os.path.join(home, ".ssh")],
    "tmp_size": "64M",
    "protect_system": False,
    "propagate": [],
    "cwd": base,
}
sandbox.build_view(spec, strict_fd=True, strict_roots=[base])
assert open(os.path.join(base, "slot", "file"), encoding="utf-8").read() == "slot"
ssh = os.path.join(home, ".ssh")
assert not os.path.exists(ssh) or os.listdir(ssh) == []
assert os.listdir("/tmp") == []
with open("/tmp/probe", "w", encoding="utf-8") as handle:
    handle.write("x")
sandbox._verify_mount_points({**spec, "home": home})
print("STRICT-VIEW-OK")
'''
    # Scratch under /var/tmp, not /tmp: the built view covers /tmp.
    import shutil
    import tempfile

    base = Path(tempfile.mkdtemp(prefix="wrkslots-strict-", dir="/var/tmp"))
    try:
        (base / "home" / ".ssh").mkdir(parents=True)
        (base / "home" / ".ssh" / "secret").write_text("s", encoding="utf-8")
        (base / "layer").mkdir()
        (base / "slot").mkdir()
        (base / "slot" / "file").write_text("slot", encoding="utf-8")
        completed = subprocess.run(
            [sys.executable, "-c", script, py, str(base)],
            capture_output=True, text=True, check=False, timeout=60,
        )
    finally:
        shutil.rmtree(base, ignore_errors=True)
    if completed.returncode != 0 and "unshare" in completed.stderr:
        pytest.skip("user namespaces unavailable on this host")
    assert completed.returncode == 0, completed.stderr
    assert "STRICT-VIEW-OK" in completed.stdout


def test_strict_mount_syscalls_fail_closed(monkeypatch: pytest.MonkeyPatch) -> None:
    # I1: no fd-mount syscall, no root launch; never a path fallback.
    def missing(number: int, *args: object) -> int:
        raise sandbox.MountError("syscall unavailable", 38)  # ENOSYS

    monkeypatch.setattr(sandbox, "_syscall", missing)
    with pytest.raises(sandbox.SandboxError, match="refusing"):
        sandbox._require_fd_mount_syscalls()


def test_claim_prefixes_and_layer_parent_are_refused(tmp_path: Path) -> None:
    # I4: a slot claimed under a claim prefix is not registered, and a
    # state directory outside the control directory refuses.
    control = _control(tmp_path)
    document = _document(tmp_path, control)
    view = document["view"]
    registered = document["registered"]
    assert isinstance(view, dict) and isinstance(registered, dict)
    trusted = sandbox._verify_registered(document, view, os.getuid())
    escape = {**view, "home_layer": str(tmp_path / "outside" / "home")}
    with pytest.raises(sandbox.SandboxError, match="state directory"):
        sandbox._check_root_paths(escape, trusted, os.getuid())
    forged = json.loads(json.dumps(document))
    forged["registered"]["slot_path"] = "/var/tmp/evil-slot"
    rows_path = control / "ACTIVE.test.json"
    rows_path.write_text(
        json.dumps({"slots": [{"slot": "s1", "agent": "worker",
                                "checkouts": [{"path": "../..//var/tmp/evil-slot"}]}]}),
        encoding="utf-8",
    )
    with pytest.raises(sandbox.SandboxError):
        sandbox._verify_registered(forged, view, os.getuid())


def test_no_new_privs_is_set_by_the_seal() -> None:
    # Kernel sanity only: the seal prctl works. The seal itself is
    # exercised through enter_root by the live root test in
    # test_slot_images (installed helper required); see I4.
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
