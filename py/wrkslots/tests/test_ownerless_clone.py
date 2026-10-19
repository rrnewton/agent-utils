"""Retire unregistered standalone clones and detached ownerless worktrees.

recover-ownerless-agent-clone removes a leaked directory that holds its own Git
directories. These tests check each refusal (dirty, untracked, staged, nested
repository dirty, unversioned content, live process, lsof diagnostic, unread
handoff, changed inventory, missing or unrelated remote, outside worktree,
failed push), that every unreachable ref, tag and stash entry gets its own
rescue ref read back from the remote, that a dry run changes nothing, that an
interrupted removal resumes, and that recover-ownerless-agent-worktree accepts
a detached HEAD only while a remote branch contains it.
"""

from __future__ import annotations

import datetime as dt
import hashlib
import os
import shlex
import subprocess
import sys
import time
from pathlib import Path

import pytest

from wrkslots import cli as wrkslots
from wrkslots.tests import test_lifecycle as lifecycle

git = lifecycle.git


class Crash(BaseException):
    """Stands in for the process dying at an interrupt point."""


def allow_test_host(monkeypatch: pytest.MonkeyPatch, *, processes: bool = False) -> None:
    monkeypatch.setattr(wrkslots, "_short_hostname", lambda: "testhost")
    monkeypatch.setattr(wrkslots, "_user_systemd_snapshot", lambda: ())
    if not processes:
        monkeypatch.setattr(
            wrkslots, "_assert_slot_unused", lambda *_args, **_kwargs: None
        )


def make_clone(
    tmp_path: Path, slot: str = "leaked-clone"
) -> tuple[Path, Path, Path]:
    """Return (project, remote, clone): a fully pushed standalone clone in the slot root."""

    project, _repository, remote = lifecycle.make_project(tmp_path)
    clone = lifecycle.slots_directory(project) / slot
    subprocess.run(
        ["git", "clone", "-q", str(remote), str(clone)],
        check=True,
        capture_output=True,
        text=True,
    )
    git(clone, "config", "user.name", "Wrkslots Test")
    git(clone, "config", "user.email", "wrkslots@example.invalid")
    git(clone, "config", "gc.autoDetach", "false")
    return project, remote, clone


def commit(checkout: Path, name: str, text: str = "content\n") -> str:
    (checkout / name).write_text(text, encoding="utf-8")
    git(checkout, "add", name)
    git(checkout, "commit", "-q", "-m", f"add {name}")
    return git(checkout, "rev-parse", "HEAD").stdout.strip()


def run_clone(
    project: Path,
    clone: Path,
    remote: Path,
    *extra: str,
    apply: bool = False,
    digest: str | None = None,
    remote_url: str | None = None,
) -> int:
    arguments = [
        "--project-root",
        str(project),
        "recover-ownerless-agent-clone",
        clone.relative_to(project).as_posix(),
        "--remote-url",
        f".git={remote_url or remote}",
        *extra,
    ]
    if apply:
        assert digest is not None
        arguments.extend(
            (
                "--inventory-sha256",
                digest,
                "--apply",
                "--coordinator-authorized",
                "--coordinator-pid",
                str(os.getpid()),
            )
        )
    return wrkslots.main(arguments)


def dry_run_digest(
    project: Path,
    clone: Path,
    remote: Path,
    capsys: pytest.CaptureFixture[str],
    *extra: str,
) -> tuple[str, str]:
    assert run_clone(project, clone, remote, *extra) == 0
    output = combined(capsys)
    first = output.splitlines()[0]
    digest = first.split("inventory_sha256=")[1].split()[0]
    return digest, output


def rescue_refs(remote: Path) -> dict[str, str]:
    listing = git(remote, "for-each-ref", "--format=%(refname) %(objectname)", "refs/rescue/")
    return dict(line.split(" ", 1) for line in listing.stdout.splitlines())


def combined(capsys: pytest.CaptureFixture[str]) -> str:
    captured = capsys.readouterr()
    return captured.out + captured.err


def removal_event(project: Path) -> dict[str, object]:
    config = wrkslots._load_config(str(project), "testhost")
    events = wrkslots._load_events(config)
    event = next(
        value
        for value in reversed(events)
        if value["kind"] == "ownerless-agent-clone-removed"
    )
    return dict(wrkslots._as_mapping(event["payload"], "payload"))


def assert_refused_and_preserved(
    project: Path,
    clone: Path,
    remote: Path,
    capsys: pytest.CaptureFixture[str],
    expected: str,
    *extra: str,
) -> None:
    assert run_clone(project, clone, remote, *extra) == 3
    captured = capsys.readouterr()
    assert expected in captured.out + captured.err
    assert clone.is_dir()
    assert rescue_refs(remote) == {}
    config = wrkslots._load_config(str(project), "testhost")
    assert wrkslots._outstanding_journals(config) == []


# --- dry run and apply ------------------------------------------------------


def test_dry_run_changes_nothing_and_apply_rescues_then_removes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, remote, clone = make_clone(tmp_path)
    git(clone, "switch", "-q", "-c", "agent/unpushed")
    unpushed = commit(clone, "work.txt")
    allow_test_host(monkeypatch)
    before = sorted(path.relative_to(clone) for path in clone.rglob("*"))

    digest, output = dry_run_digest(project, clone, remote, capsys)

    assert "outcome=planned detail=ownerless-clone" in output
    assert f"TIP refs/heads/agent/unpushed {unpushed} commit rescue->refs/rescue/leaked-clone-" in output
    assert "TIP refs/heads/main" in output and " reachable" in output
    assert f"--inventory-sha256 {digest} --apply" in output
    assert clone.is_dir()
    assert sorted(path.relative_to(clone) for path in clone.rglob("*")) == before
    assert rescue_refs(remote) == {}

    assert run_clone(project, clone, remote, apply=True, digest=digest) == 0
    output = combined(capsys)
    assert not clone.exists()
    refs = rescue_refs(remote)
    assert list(refs.values()) == [unpushed]
    [name] = refs
    assert f"RESCUED repository=.git ref={name} object={unpushed}" in output
    payload = removal_event(project)
    receipts = wrkslots._as_list(payload["rescue"], "rescue")
    assert [dict(wrkslots._as_mapping(r, "r"))["ref"] for r in receipts] == [name]
    record = wrkslots._as_mapping(payload["record"], "record")
    [repository] = wrkslots._as_list(record["repositories"], "repositories")
    tips = {
        tip["label"]: tip["survives_as"]
        for tip in (
            wrkslots._as_mapping(value, "tip")
            for value in wrkslots._as_list(
                wrkslots._as_mapping(repository, "repository")["tips"], "tips"
            )
        )
    }
    assert tips["refs/heads/agent/unpushed"] == name
    assert tips["refs/heads/main"] == "reachable-from-remote"
    config = wrkslots._load_config(str(project), "testhost")
    assert wrkslots._outstanding_journals(config) == []


def test_fully_pushed_clone_is_removed_without_any_rescue_ref(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, remote, clone = make_clone(tmp_path)
    allow_test_host(monkeypatch)
    digest, output = dry_run_digest(project, clone, remote, capsys)
    assert "rescue_objects=0" in output
    assert run_clone(project, clone, remote, apply=True, digest=digest) == 0
    assert not clone.exists()
    assert rescue_refs(remote) == {}


def test_every_unreachable_branch_tag_and_stash_entry_gets_its_own_rescue_ref(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, remote, clone = make_clone(tmp_path)
    git(clone, "switch", "-q", "-c", "agent/one")
    branch_tip = commit(clone, "one.txt")
    git(clone, "tag", "-a", "-m", "annotated", "v-local")
    tag_object = git(clone, "rev-parse", "refs/tags/v-local").stdout.strip()
    git(clone, "switch", "-q", "main")
    stashes = []
    for number in range(2):
        (clone / "seed.txt").write_text(f"stash {number}\n", encoding="utf-8")
        git(clone, "stash", "push", "-q", "-m", f"stash {number}")
        stashes.append(git(clone, "rev-parse", "refs/stash").stdout.strip())
    allow_test_host(monkeypatch)

    digest, output = dry_run_digest(project, clone, remote, capsys)
    assert "rescue_objects=4" in output
    assert "TIP refs/stash@{0}" in output and "TIP refs/stash@{1}" in output
    assert run_clone(project, clone, remote, apply=True, digest=digest) == 0
    assert not clone.exists()

    refs = rescue_refs(remote)
    assert sorted(refs.values()) == sorted([branch_tip, tag_object, *stashes])
    names = sorted(refs)
    stem = names[0]
    assert stem.startswith("refs/rescue/leaked-clone-")
    assert names == sorted([stem, f"{stem}-2", f"{stem}-3", f"{stem}-4"])
    # The stash entries are full commits on the remote, each with its own ref.
    for stash in stashes:
        assert git(remote, "cat-file", "-t", stash).stdout.strip() == "commit"
    assert git(remote, "cat-file", "-t", tag_object).stdout.strip() == "tag"


def test_rescue_names_skip_refs_the_remote_already_has(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, remote, clone = make_clone(tmp_path)
    git(clone, "switch", "-q", "-c", "agent/one")
    tip = commit(clone, "one.txt")
    date = dt.datetime.now(dt.timezone.utc).strftime("%Y%m%d")
    taken = f"refs/rescue/leaked-clone-{date}"
    seed = git(remote, "rev-parse", "refs/heads/main").stdout.strip()
    git(remote, "update-ref", taken, seed)
    allow_test_host(monkeypatch)
    digest, _output = dry_run_digest(project, clone, remote, capsys)
    assert run_clone(project, clone, remote, apply=True, digest=digest) == 0
    refs = rescue_refs(remote)
    assert refs[taken] == seed
    assert refs[f"{taken}-2"] == tip


# --- refusals ---------------------------------------------------------------


def test_refuses_modified_tracked_file(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, remote, clone = make_clone(tmp_path)
    (clone / "seed.txt").write_text("changed\n", encoding="utf-8")
    allow_test_host(monkeypatch)
    assert_refused_and_preserved(project, clone, remote, capsys, "seed.txt")


def test_refuses_untracked_file(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, remote, clone = make_clone(tmp_path)
    (clone / "notes.txt").write_text("only copy\n", encoding="utf-8")
    allow_test_host(monkeypatch)
    assert_refused_and_preserved(project, clone, remote, capsys, "notes.txt")


def test_refuses_staged_change(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, remote, clone = make_clone(tmp_path)
    (clone / "staged.txt").write_text("staged\n", encoding="utf-8")
    git(clone, "add", "staged.txt")
    allow_test_host(monkeypatch)
    assert_refused_and_preserved(project, clone, remote, capsys, "staged.txt")


def test_refuses_dirty_nested_repository_and_nested_worktree(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, remote, clone = make_clone(tmp_path)
    (clone / ".git" / "info" / "exclude").write_text("ignored/\n", encoding="utf-8")
    nested = clone / "ignored" / "nested"
    subprocess.run(
        ["git", "clone", "-q", str(remote), str(nested)],
        check=True,
        capture_output=True,
        text=True,
    )
    git(clone, "worktree", "add", "-q", "-b", "agent/inner", str(clone / "ignored" / "inner"))
    allow_test_host(monkeypatch)
    # Clean nested repositories are accepted (and inventoried separately).
    digest, output = dry_run_digest(
        project,
        clone,
        remote,
        capsys,
        "--remote-url",
        f"ignored/nested/.git={remote}",
    )
    assert "REPOSITORY key=ignored/nested/.git" in output
    assert "TIP worktree:ignored/inner:HEAD" in output

    (nested / "seed.txt").write_text("nested edit\n", encoding="utf-8")
    assert_refused_and_preserved(
        project,
        clone,
        remote,
        capsys,
        "checkout ignored/nested has 1 uncommitted",
        "--remote-url",
        f"ignored/nested/.git={remote}",
    )
    git(nested, "checkout", "--", "seed.txt")
    (clone / "ignored" / "inner" / "new.txt").write_text("inner\n", encoding="utf-8")
    assert_refused_and_preserved(
        project,
        clone,
        remote,
        capsys,
        "checkout ignored/inner has 1 uncommitted",
        "--remote-url",
        f"ignored/nested/.git={remote}",
    )


def test_refuses_content_outside_every_checkout(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, _repository, remote = lifecycle.make_project(tmp_path)
    holder = lifecycle.slots_directory(project) / "holder"
    subprocess.run(
        ["git", "clone", "-q", str(remote), str(holder / "product")],
        check=True,
        capture_output=True,
        text=True,
    )
    (holder / "scratch").mkdir()
    (holder / "scratch" / "result.log").write_text("only copy\n", encoding="utf-8")
    allow_test_host(monkeypatch)
    arguments = [
        "--project-root",
        str(project),
        "recover-ownerless-agent-clone",
        holder.relative_to(project).as_posix(),
        "--remote-url",
        f"product/.git={remote}",
    ]
    assert wrkslots.main(arguments) == 3
    captured = capsys.readouterr()
    assert "scratch/result.log" in captured.out
    assert holder.is_dir()


def test_refuses_live_process_inside_the_clone(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, remote, clone = make_clone(tmp_path)
    allow_test_host(monkeypatch, processes=True)
    holder = subprocess.Popen(
        [sys.executable, "-c", "import time; time.sleep(60)"], cwd=clone
    )
    try:
        assert run_clone(project, clone, remote) == 3
        output = combined(capsys)
        # lsof either names the process or, when it prints any diagnostic on this
        # host, is indeterminate; both refuse.
        assert "uses slot" in output or "process use is indeterminate" in output
        assert clone.is_dir()
        assert rescue_refs(remote) == {}
    finally:
        holder.kill()
        holder.wait()


def test_strict_lsof_refuses_what_the_relaxed_check_tolerates(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The relaxed check attributes a warning to an unrelated path; strict mode
    tolerates only a can't-stat warning for a mount the table lists."""

    slot, _elsewhere = strict_setup(tmp_path, monkeypatch)
    fake = fake_lsof(tmp_path, [cant_stat("/unrelated/mount", "fuse")])
    evidence, _malformed, returncode = wrkslots._lsof_slot_use(fake, slot)
    assert evidence is None and returncode == 1
    with pytest.raises(wrkslots.Refusal, match="lists no fuse mount at /unrelated/mount"):
        wrkslots._lsof_slot_use(fake, slot, strict_warnings=True)
    fake = fake_lsof(tmp_path, ["lsof: WARNING: can't opendir(/unrelated/dir): Permission denied"])
    evidence, _malformed, returncode = wrkslots._lsof_slot_use(fake, slot)
    assert evidence is None and returncode == 1
    with pytest.raises(wrkslots.Refusal, match="only a can't-stat warning"):
        wrkslots._lsof_slot_use(fake, slot, strict_warnings=True)


def fake_lsof(tmp_path: Path, stderr_lines: list[str]) -> Path:
    """An lsof that finds nothing and prints ``stderr_lines`` on stderr."""

    script = tmp_path / "lsof"
    body = "".join(
        f"printf '%s\\n' {shlex.quote(line)} >&2\n" for line in stderr_lines
    )
    script.write_text(f"#!/bin/sh\n{body}exit 1\n", encoding="utf-8")
    script.chmod(0o755)
    return script


# The mount that holds every test slot in the fake mount tables below, and the
# unrelated FUSE mount lsof cannot stat.  The device numbers are chosen to be
# none a real test directory has.
HOLDING_MOUNT = "36 1 250:250 / / rw,relatime shared:1 - btrfs /dev/fake-disk rw\n"
FUSE_DEVICE = "0:9999"


def cant_stat(point: Path | str, fstype: str = "fuse.squashfuse_ll") -> str:
    return f"lsof: WARNING: can't stat() {fstype} file system {point}"


def mount_line(
    point: Path | str,
    *,
    device: str = FUSE_DEVICE,
    fstype: str = "fuse.squashfuse_ll",
    source: str = "squashfuse_ll",
    mount_id: int = 2131,
) -> str:
    return f"{mount_id} 36 {device} / {point} rw,nosuid,nodev - {fstype} {source} rw\n"


def strict_setup(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, *extra_mounts: str
) -> tuple[Path, Path]:
    """Return (slot, unrelated directory) under a fake mount table."""

    slot = tmp_path / "slots" / "slot"
    slot.mkdir(parents=True)
    elsewhere = tmp_path / "xarfuse" / "uid-48"
    elsewhere.mkdir(parents=True)
    table = tmp_path / "mountinfo"
    table.write_text(HOLDING_MOUNT + "".join(extra_mounts), encoding="utf-8")
    monkeypatch.setattr(wrkslots, "_SELF_MOUNTINFO", table)
    return slot, elsewhere


def test_strict_lsof_tolerates_and_reports_an_unrelated_unstattable_mount(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    point = tmp_path / "xarfuse" / "uid-48" / "f3975eec-seed"
    slot, _elsewhere = strict_setup(tmp_path, monkeypatch, mount_line(point))
    fake = fake_lsof(
        tmp_path,
        [
            cant_stat(point),
            "      Output information may be incomplete.",
            f"lsof: assuming dev={os.makedev(0, 9999):#x} for {point} from /proc/self/mountinfo",
        ],
    )
    evidence, malformed, returncode = wrkslots._lsof_slot_use(
        fake, slot, strict_warnings=True
    )
    assert (evidence, malformed, returncode) == (None, False, 1)
    out = capsys.readouterr().out
    assert (
        f"NOTE lsof could not stat the fuse.squashfuse_ll file system at {point} "
        "(device 0:9999); tolerated: it is another file system" in out
    )


@pytest.mark.parametrize("where", ["slot", "ancestor", "descendant"])
def test_strict_lsof_refuses_a_warning_for_a_mount_related_to_the_slot(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, where: str
) -> None:
    slot = tmp_path / "slots" / "slot"
    point = {"slot": slot, "ancestor": slot.parent, "descendant": slot / "sub"}[where]
    slot, _elsewhere = strict_setup(tmp_path, monkeypatch, mount_line(point))
    fake = fake_lsof(tmp_path, [cant_stat(point)])
    with pytest.raises(
        wrkslots.Refusal, match="names the slot, an ancestor of it, or a descendant of it"
    ):
        wrkslots._lsof_slot_use(fake, slot, strict_warnings=True)
    # The relaxed check refuses it too.
    with pytest.raises(wrkslots.Refusal, match="indeterminate because lsof reported"):
        wrkslots._lsof_slot_use(fake, slot)


@pytest.mark.parametrize(
    "lines",
    [
        ["lsof: WARNING: can't opendir(/elsewhere/dir): Permission denied"],
        ["lsof: WARNING: can't stat(/elsewhere/file): Permission denied"],
        ["Output information may be incomplete."],
        ["lsof: some future diagnostic"],
        # A tolerable warning does not excuse a second, unknown line.
        ["CANT_STAT", "lsof: WARNING: something else entirely"],
        # lsof's device note names a mount it did not warn about.
        ["CANT_STAT", "lsof: assuming dev=0x10 for /other/mount from /proc/self/mountinfo"],
    ],
)
def test_strict_lsof_refuses_every_other_diagnostic(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, lines: list[str]
) -> None:
    point = tmp_path / "xarfuse" / "uid-48" / "seed"
    slot, _elsewhere = strict_setup(tmp_path, monkeypatch, mount_line(point))
    fake = fake_lsof(tmp_path, [cant_stat(point) if line == "CANT_STAT" else line for line in lines])
    with pytest.raises(wrkslots.Refusal, match="indeterminate because lsof reported"):
        wrkslots._lsof_slot_use(fake, slot, strict_warnings=True)


def test_strict_lsof_refuses_a_warned_mount_missing_from_the_mount_table(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    point = tmp_path / "xarfuse" / "uid-48" / "seed"
    # Listed, but as another file system type than lsof named.
    slot, _elsewhere = strict_setup(tmp_path, monkeypatch, mount_line(point, fstype="fuse"))
    fake = fake_lsof(tmp_path, [cant_stat(point)])
    with pytest.raises(wrkslots.Refusal, match="lists no fuse.squashfuse_ll mount"):
        wrkslots._lsof_slot_use(fake, slot, strict_warnings=True)
    fake = fake_lsof(tmp_path, [cant_stat(tmp_path / "xarfuse" / "uid-48" / "gone")])
    with pytest.raises(wrkslots.Refusal, match="lists no fuse.squashfuse_ll mount"):
        wrkslots._lsof_slot_use(fake, slot, strict_warnings=True)


@pytest.mark.parametrize(
    "alias",
    [
        # A bind mount of the slot's own file system shows the table device of
        # the mount holding the slot ...
        {"device": "250:250", "fstype": "btrfs", "source": "/dev/fake-disk", "why": "shares a device"},
        # ... or, on btrfs, a different one with the same type and source.
        {"device": "0:9998", "fstype": "btrfs", "source": "/dev/fake-disk", "why": "type and source"},
        # A mount whose device is the slot's own st_dev.
        {"device": "SLOT", "fstype": "ext4", "source": "/dev/other", "why": "shares a device"},
        # Overlay and union mounts show other directories under their own device.
        {"device": "0:9997", "fstype": "overlay", "source": "overlay", "why": "a overlay mount shows"},
        {"device": "0:9996", "fstype": "fuse.bindfs", "source": "bindfs", "why": "a fuse.bindfs mount shows"},
        # A mount whose source lies inside the slot.
        {"device": "0:9995", "fstype": "fuse.squashfuse_ll", "source": "SLOT_IMAGE", "why": "source .* names the slot"},
        {"device": "0:9994", "fstype": "fuse.sshfs", "source": "HOST_SLOT", "why": "source .* names the slot"},
        {"device": "0:9993", "fstype": "nfs4", "source": "IPV6_SLOT", "why": "source .* names the slot"},
    ],
)
def test_strict_lsof_refuses_an_unstattable_mount_that_may_alias_the_slot(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, alias: dict[str, str]
) -> None:
    slot_path = tmp_path / "slots" / "slot"
    slot_path.mkdir(parents=True)
    metadata = os.stat(slot_path)
    device = (
        f"{os.major(metadata.st_dev)}:{os.minor(metadata.st_dev)}"
        if alias["device"] == "SLOT"
        else alias["device"]
    )
    source = {
        "SLOT_IMAGE": str(slot_path / "image.sqfs"),
        "HOST_SLOT": f"localhost:{slot_path}",
        "IPV6_SLOT": f"[::1]:{slot_path.parent}",
    }.get(alias["source"], alias["source"])
    point = tmp_path / "xarfuse" / "uid-48" / "alias"
    slot_path.rmdir()
    slot, _elsewhere = strict_setup(
        tmp_path,
        monkeypatch,
        mount_line(point, device=device, fstype=alias["fstype"], source=source),
    )
    fake = fake_lsof(tmp_path, [cant_stat(point, alias["fstype"])])
    with pytest.raises(wrkslots.Refusal, match=alias["why"]):
        wrkslots._lsof_slot_use(fake, slot, strict_warnings=True)


@pytest.mark.parametrize(
    ("aliased", "below", "refused"),
    [
        # `mount --bind <slot> <elsewhere>`: a mount below the alias is below the slot.
        ("slot", "sub", True),
        # `mount --bind <slot parent> <elsewhere>`: the slot itself, below it, and
        # a sibling directory, which is not the slot.
        ("parent", "slot", True),
        ("parent", "slot/sub", True),
        ("parent", "sibling", False),
        # `mount --bind <slot grandparent> <elsewhere>`: the slot's parent.
        ("grandparent", "slots", True),
    ],
)
def test_strict_lsof_checks_bind_aliases_of_the_slot_and_its_ancestors(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    aliased: str,
    below: str,
    refused: bool,
) -> None:
    alias_root = tmp_path / "xarfuse" / "uid-48"
    point = alias_root.joinpath(*below.split("/"))
    slot, elsewhere = strict_setup(tmp_path, monkeypatch, mount_line(point))
    assert elsewhere == alias_root
    target = {"slot": slot, "parent": slot.parent, "grandparent": slot.parent.parent}[aliased]
    # Stand-in for `mount --bind <target> <elsewhere>`, which a test cannot make:
    # every directory below the alias has the device and inode of the one it shows.
    real_stat = os.stat

    def aliased_stat(
        path: str | os.PathLike[str] | int,
        *,
        dir_fd: int | None = None,
        follow_symlinks: bool = True,
    ) -> os.stat_result:
        if not isinstance(path, int):
            candidate = Path(os.fspath(path))
            if candidate == elsewhere or elsewhere in candidate.parents:
                shown = target / candidate.relative_to(elsewhere)
                return real_stat(shown, dir_fd=dir_fd, follow_symlinks=follow_symlinks)
        return real_stat(path, dir_fd=dir_fd, follow_symlinks=follow_symlinks)

    (slot / "sub").mkdir()
    (slot.parent / "sibling").mkdir()
    monkeypatch.setattr(os, "stat", aliased_stat)
    fake = fake_lsof(tmp_path, [cant_stat(point)])
    if refused:
        with pytest.raises(wrkslots.Refusal, match="under another name"):
            wrkslots._lsof_slot_use(fake, slot, strict_warnings=True)
    else:
        assert wrkslots._lsof_slot_use(fake, slot, strict_warnings=True)[0] is None
        assert f"NOTE lsof could not stat the fuse.squashfuse_ll file system at {point}" in (
            capsys.readouterr().out
        )


def test_strict_lsof_refuses_a_bind_of_the_slot_file_system_under_a_stacked_mount(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    point = tmp_path / "xarfuse" / "uid-48" / "seed"
    slot, _elsewhere = strict_setup(
        tmp_path,
        monkeypatch,
        mount_line(point, device="250:250", fstype="btrfs", source="/dev/fake-disk", mount_id=2130),
        mount_line(point),
    )
    fake = fake_lsof(tmp_path, [cant_stat(point)])
    with pytest.raises(wrkslots.Refusal, match="shares a device"):
        wrkslots._lsof_slot_use(fake, slot, strict_warnings=True)


def test_strict_lsof_refuses_when_one_of_several_warnings_is_related(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    unrelated = tmp_path / "xarfuse" / "uid-48" / "seed"
    slot = tmp_path / "slots" / "slot"
    slot, _elsewhere = strict_setup(
        tmp_path, monkeypatch, mount_line(unrelated), mount_line(slot / "sub", mount_id=2132)
    )
    fake = fake_lsof(tmp_path, [cant_stat(unrelated), cant_stat(slot / "sub")])
    with pytest.raises(wrkslots.Refusal, match="names the slot"):
        wrkslots._lsof_slot_use(fake, slot, strict_warnings=True)


def test_strict_lsof_compares_the_resolved_slot_path(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    slot, _elsewhere = strict_setup(tmp_path, monkeypatch, mount_line(tmp_path / "slots"))
    link = tmp_path / "link-to-slot"
    link.symlink_to(slot)
    fake = fake_lsof(tmp_path, [cant_stat(tmp_path / "slots")])
    with pytest.raises(wrkslots.Refusal, match="names the slot, an ancestor"):
        wrkslots._lsof_slot_use(fake, link, strict_warnings=True)


def test_strict_lsof_refuses_what_it_cannot_check(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    point = tmp_path / "xarfuse" / "uid-48" / "seed"
    # lsof's device note disagrees with the mount table.
    slot, _elsewhere = strict_setup(tmp_path, monkeypatch, mount_line(point))
    fake = fake_lsof(
        tmp_path,
        [cant_stat(point), f"lsof: assuming dev=0x99 for {point} from /proc/self/mountinfo"],
    )
    with pytest.raises(wrkslots.Refusal, match="the mount table gives .* device 0:9999"):
        wrkslots._lsof_slot_use(fake, slot, strict_warnings=True)
    # The table gives no device for the mount.
    table = tmp_path / "mountinfo"
    table.write_text(HOLDING_MOUNT + mount_line(point, device="x:y"), encoding="utf-8")
    fake = fake_lsof(tmp_path, [cant_stat(point)])
    with pytest.raises(wrkslots.Refusal, match="gives no device"):
        wrkslots._lsof_slot_use(fake, slot, strict_warnings=True)
    # The table lists no mount holding the slot.
    table.write_text(mount_line(point), encoding="utf-8")
    with pytest.raises(wrkslots.Refusal, match="mount holding the slot is not in the mount table"):
        wrkslots._lsof_slot_use(fake, slot, strict_warnings=True)
    # A directory above the mount point cannot be inspected.
    hidden = tmp_path / "absent" / "seed"
    table.write_text(HOLDING_MOUNT + mount_line(hidden), encoding="utf-8")
    fake = fake_lsof(tmp_path, [cant_stat(hidden)])
    with pytest.raises(wrkslots.Refusal, match="cannot inspect .*absent, which holds the mount"):
        wrkslots._lsof_slot_use(fake, slot, strict_warnings=True)
    # The slot cannot be inspected.
    gone = tmp_path / "slots" / "gone"
    with pytest.raises(wrkslots.Refusal, match="cannot inspect the slot"):
        wrkslots._lsof_slot_use(fake, gone, strict_warnings=True)


def test_refuses_unread_handoff_and_preserves_a_read_one(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, remote, clone = make_clone(tmp_path)
    (clone / "HANDOFF.md").write_text("resume here\n", encoding="utf-8")
    digest_of_handoff = hashlib.sha256(b"resume here\n").hexdigest()
    allow_test_host(monkeypatch)
    assert_refused_and_preserved(project, clone, remote, capsys, "unread handoff")

    digest, _output = dry_run_digest(
        project, clone, remote, capsys, "--handoff-sha256", digest_of_handoff
    )
    assert (
        run_clone(
            project,
            clone,
            remote,
            "--handoff-sha256",
            digest_of_handoff,
            apply=True,
            digest=digest,
        )
        == 0
    )
    assert not clone.exists()
    copies = list(
        lifecycle.control_directory(project).glob("HANDOFF-OWNERLESS-CLONE-RETIRED.*.md")
    )
    assert [copy.read_text(encoding="utf-8") for copy in copies] == ["resume here\n"]


def test_apply_refuses_when_the_clone_changed_after_the_dry_run(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, remote, clone = make_clone(tmp_path)
    allow_test_host(monkeypatch)
    digest, _output = dry_run_digest(project, clone, remote, capsys)
    git(clone, "switch", "-q", "-c", "agent/late")
    commit(clone, "late.txt")
    assert run_clone(project, clone, remote, apply=True, digest=digest) == 3
    assert "not the reviewed" in combined(capsys)
    assert clone.is_dir()
    assert rescue_refs(remote) == {}


def test_apply_requires_the_reviewed_digest(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, remote, clone = make_clone(tmp_path)
    allow_test_host(monkeypatch)
    arguments = [
        "--project-root",
        str(project),
        "recover-ownerless-agent-clone",
        clone.relative_to(project).as_posix(),
        "--remote-url",
        f".git={remote}",
        "--apply",
        "--coordinator-authorized",
        "--coordinator-pid",
        str(os.getpid()),
    ]
    assert wrkslots.main(arguments) == 3
    assert "--inventory-sha256" in combined(capsys)
    assert clone.is_dir()


def test_refuses_missing_unknown_and_unrelated_remote_urls(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, remote, clone = make_clone(tmp_path)
    git(clone, "switch", "-q", "-c", "agent/one")
    commit(clone, "one.txt")
    allow_test_host(monkeypatch)
    arguments = [
        "--project-root",
        str(project),
        "recover-ownerless-agent-clone",
        clone.relative_to(project).as_posix(),
    ]
    assert wrkslots.main(arguments) == 3
    assert "missing: .git (configured remotes: origin=" in combined(capsys)
    assert wrkslots.main([*arguments, "--remote-url", f"nope/.git={remote}"]) == 3
    assert "not in the clone: nope/.git" in combined(capsys)

    unrelated = tmp_path / "unrelated.git"
    subprocess.run(["git", "init", "-q", "--bare", str(unrelated)], check=True)
    source = tmp_path / "unrelated-source"
    subprocess.run(["git", "clone", "-q", str(unrelated), str(source)], check=True,
                   capture_output=True)
    git(source, "config", "user.name", "x")
    git(source, "config", "user.email", "x@example.invalid")
    commit(source, "other.txt")
    git(source, "push", "-q", "origin", "HEAD:refs/heads/main")
    assert run_clone(project, clone, remote, remote_url=str(unrelated)) == 3
    assert "advertises no commit" in combined(capsys)
    assert rescue_refs(unrelated) == {}

    inside = clone / ".git"
    assert run_clone(project, clone, remote, remote_url=str(inside)) == 3
    assert "inside the clone" in combined(capsys)


def test_refuses_a_worktree_of_the_clone_that_lives_outside_it(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, remote, clone = make_clone(tmp_path)
    outside = lifecycle.slots_directory(project) / "sibling"
    git(clone, "worktree", "add", "-q", "-b", "agent/sibling", str(outside))
    allow_test_host(monkeypatch)
    assert run_clone(project, clone, remote) == 3
    assert "outside the clone" in combined(capsys)
    assert clone.is_dir() and outside.is_dir()


def test_refuses_and_preserves_when_the_rescue_push_is_rejected(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, remote, clone = make_clone(tmp_path)
    git(clone, "switch", "-q", "-c", "agent/one")
    commit(clone, "one.txt")
    allow_test_host(monkeypatch)
    digest, _output = dry_run_digest(project, clone, remote, capsys)
    hook = remote / "hooks" / "pre-receive"
    hook.write_text("#!/bin/sh\necho rejected >&2\nexit 1\n", encoding="utf-8")
    hook.chmod(0o755)
    assert run_clone(project, clone, remote, apply=True, digest=digest) == 3
    assert "removal journal was retired" in combined(capsys)
    assert clone.is_dir()
    assert rescue_refs(remote) == {}
    # Nothing was removed, so the journal is retired rather than left to block
    # every other mutation, and the abandonment is recorded.
    config = wrkslots._load_config(str(project), "testhost")
    assert wrkslots._outstanding_journals(config) == []
    assert any(
        event["kind"] == "ownerless-agent-clone-abandoned"
        for event in wrkslots._load_events(config)
    )
    hook.unlink()
    assert run_clone(project, clone, remote, apply=True, digest=digest) == 0
    assert not clone.exists()
    assert len(rescue_refs(remote)) == 1


def test_interrupted_after_fence_resumes_with_recover(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, remote, clone = make_clone(tmp_path)
    git(clone, "switch", "-q", "-c", "agent/one")
    tip = commit(clone, "one.txt")
    git(clone, "worktree", "add", "-q", "-b", "agent/inner", str(clone / "inner"))
    (clone / ".git" / "info" / "exclude").write_text("inner/\n", encoding="utf-8")
    allow_test_host(monkeypatch)
    digest, _output = dry_run_digest(project, clone, remote, capsys)

    def crash_at_fence(point: str) -> None:
        if point == "after-ownerless-clone-fence":
            raise Crash(point)

    monkeypatch.setattr(wrkslots, "_interrupt_for_test", crash_at_fence)
    with pytest.raises(Crash):
        run_clone(project, clone, remote, apply=True, digest=digest)
    assert not clone.exists()
    fenced = list(clone.parent.glob(".leaked-clone.ownerless-clone.*"))
    assert len(fenced) == 1

    monkeypatch.setattr(wrkslots, "_interrupt_for_test", lambda _point: None)
    assert (
        wrkslots.main(
            [
                "--project-root",
                str(project),
                "recover",
                "--coordinator-authorized",
                "--coordinator-pid",
                str(os.getpid()),
            ]
        )
        == 0
    )
    assert not clone.exists() and not fenced[0].exists()
    assert list(rescue_refs(remote).values()) == [tip]
    assert removal_event(project)["slot"] == "leaked-clone"


# --- detached ownerless worktrees -------------------------------------------


def run_detached_worktree(project: Path, target: Path, head: str, digest: str, *, apply: bool) -> int:
    arguments = [
        "--project-root",
        str(project),
        "recover-ownerless-agent-worktree",
        target.relative_to(project).as_posix(),
        "--repository",
        "repo",
        "--head",
        head,
        "--detached",
        "--remote",
        "origin",
        "--remote-url-sha256",
        digest,
    ]
    if apply:
        arguments.extend(
            ("--apply", "--coordinator-authorized", "--coordinator-pid", str(os.getpid()))
        )
    return wrkslots.main(arguments)


def test_detached_worktree_on_a_pushed_commit_is_retired(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, repository, _remote = lifecycle.make_project(tmp_path)
    target = lifecycle.slots_directory(project) / "detached"
    git(repository, "worktree", "add", "-q", "--detach", str(target), "origin/main")
    head = git(target, "rev-parse", "HEAD").stdout.strip()
    digest = wrkslots._GitVcs().remote_url_sha256(target, "origin")
    allow_test_host(monkeypatch)
    assert run_detached_worktree(project, target, head, digest, apply=False) == 0
    assert "detail=ownerless-agent-detached" in combined(capsys)
    assert run_detached_worktree(project, target, head, digest, apply=True) == 0
    assert not target.exists()
    assert target.absolute() not in wrkslots._GitVcs().listed_worktrees(repository)


def test_detached_worktree_on_an_unpushed_commit_is_refused(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, repository, _remote = lifecycle.make_project(tmp_path)
    target = lifecycle.slots_directory(project) / "detached"
    git(repository, "worktree", "add", "-q", "--detach", str(target), "origin/main")
    git(target, "config", "user.name", "Wrkslots Test")
    git(target, "config", "user.email", "wrkslots@example.invalid")
    head = commit(target, "local.txt")
    digest = wrkslots._GitVcs().remote_url_sha256(target, "origin")
    allow_test_host(monkeypatch)
    assert run_detached_worktree(project, target, head, digest, apply=True) == 3
    assert "no branch of remote origin contains" in combined(capsys)
    assert target.is_dir()


def test_detached_flag_refuses_a_checkout_on_a_branch(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, repository, _remote = lifecycle.make_project(tmp_path)
    target = lifecycle.slots_directory(project) / "onbranch"
    git(repository, "worktree", "add", "-q", "-b", "agent/onbranch", str(target), "HEAD")
    head = git(target, "rev-parse", "HEAD").stdout.strip()
    digest = wrkslots._GitVcs().remote_url_sha256(target, "origin")
    allow_test_host(monkeypatch)
    assert run_detached_worktree(project, target, head, digest, apply=False) == 3
    assert "not detached" in combined(capsys)
    assert target.is_dir()


def test_crash_after_the_rescue_push_keeps_the_rescue_ref_in_the_record(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, remote, clone = make_clone(tmp_path)
    git(clone, "switch", "-q", "-c", "agent/one")
    tip = commit(clone, "one.txt")
    allow_test_host(monkeypatch)
    digest, _output = dry_run_digest(project, clone, remote, capsys)
    original_push = wrkslots._ownerless_clone_push

    def push_then_die(*args: object, **kwargs: object) -> None:
        original_push(*args, **kwargs)  # type: ignore[arg-type]
        raise Crash("after push")

    monkeypatch.setattr(wrkslots, "_ownerless_clone_push", push_then_die)
    with pytest.raises(Crash):
        run_clone(project, clone, remote, apply=True, digest=digest)
    assert clone.is_dir()
    [name] = rescue_refs(remote)

    monkeypatch.setattr(wrkslots, "_ownerless_clone_push", original_push)
    assert run_clone(project, clone, remote, apply=True, digest=digest) == 0
    assert not clone.exists()
    assert rescue_refs(remote) == {name: tip}
    payload = removal_event(project)
    receipts = [
        dict(wrkslots._as_mapping(value, "receipt"))
        for value in wrkslots._as_list(payload["rescue"], "rescue")
    ]
    assert [(r["ref"], r["object"]) for r in receipts] == [(name, tip)]


def test_read_only_directories_inside_the_clone_are_removed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, remote, clone = make_clone(tmp_path)
    commit(clone, ".gitignore", "cache/\n")
    git(clone, "push", "-q", "origin", "main")
    locked = clone / "cache" / "module"
    locked.mkdir(parents=True)
    (locked / "file.go").write_text("package x\n", encoding="utf-8")
    locked.chmod(0o555)
    allow_test_host(monkeypatch)
    digest, output = dry_run_digest(project, clone, remote, capsys, "--discard-ignored")
    assert "IGNORED cache/module/file.go bytes=10" in output
    assert (
        run_clone(project, clone, remote, "--discard-ignored", apply=True, digest=digest) == 0
    )
    assert not clone.exists()


def test_ignored_files_need_discard_ignored_and_local_ignores_count_as_untracked(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, remote, clone = make_clone(tmp_path)
    commit(clone, ".gitignore", "build/\n")
    git(clone, "push", "-q", "origin", "main")
    (clone / "build").mkdir()
    (clone / "build" / "receipt.json").write_text("{}\n", encoding="utf-8")
    allow_test_host(monkeypatch)
    assert_refused_and_preserved(
        project, clone, remote, capsys, "build/receipt.json"
    )
    # A pattern in .git/info/exclude is a local setting, not a project
    # declaration, so a file it hides is untracked work and always refuses.
    (clone / ".git" / "info" / "exclude").write_text("*.md\n", encoding="utf-8")
    (clone / "design-notes.md").write_text("only copy\n", encoding="utf-8")
    assert_refused_and_preserved(
        project,
        clone,
        remote,
        capsys,
        "design-notes.md (ignored only by .git/info/exclude)",
        "--discard-ignored",
    )
    (clone / "design-notes.md").unlink()
    digest, _output = dry_run_digest(project, clone, remote, capsys, "--discard-ignored")
    assert (
        run_clone(project, clone, remote, "--discard-ignored", apply=True, digest=digest) == 0
    )
    assert not clone.exists()


def test_refuses_a_local_remote_that_depends_on_the_clone_or_a_slot(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, remote, clone = make_clone(tmp_path)
    git(clone, "switch", "-q", "-c", "agent/unpushed")
    commit(clone, "work.txt")
    allow_test_host(monkeypatch)
    # A bare clone made with --shared borrows the clone's objects.
    borrower = tmp_path / "backup.git"
    subprocess.run(
        ["git", "clone", "-q", "--bare", "--shared", str(clone), str(borrower)],
        check=True,
        capture_output=True,
    )
    assert run_clone(project, clone, remote, remote_url=str(borrower)) == 3
    assert "an object directory it uses" in combined(capsys)
    # A directory whose .git file names the clone's own Git directory.
    alias = tmp_path / "alias"
    alias.mkdir()
    (alias / ".git").write_text(f"gitdir: {clone / '.git'}\n", encoding="utf-8")
    assert run_clone(project, clone, remote, remote_url=str(alias)) == 3
    assert "its Git directory" in combined(capsys)
    # Another slot is no more durable than this one.
    sibling = lifecycle.slots_directory(project) / "sibling.git"
    subprocess.run(
        ["git", "clone", "-q", "--bare", str(remote), str(sibling)],
        check=True,
        capture_output=True,
    )
    assert run_clone(project, clone, remote, remote_url=str(sibling)) == 3
    assert "inside slot root" in combined(capsys)
    assert clone.is_dir()


def test_refuses_when_another_slot_borrows_objects_from_the_clone(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, remote, clone = make_clone(tmp_path)
    mirror = lifecycle.slots_directory(project) / "other-slot" / "mirror.git"
    subprocess.run(
        ["git", "clone", "-q", "--bare", "--shared", str(clone), str(mirror)],
        check=True,
        capture_output=True,
    )
    allow_test_host(monkeypatch)
    assert_refused_and_preserved(project, clone, remote, capsys, "borrows objects")


def test_a_missing_worktree_keeps_its_refs_and_staged_index_in_the_proof(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, remote, clone = make_clone(tmp_path)
    away = tmp_path / "away"
    git(clone, "worktree", "add", "-q", "--detach", str(away))
    git(away, "config", "user.name", "x")
    keep = commit(away, "keep.txt")
    git(away, "update-ref", "refs/worktree/keep", keep)
    git(away, "reset", "-q", "--hard", "HEAD~1")
    subprocess.run(["rm", "-rf", "--", str(away)], check=True)
    allow_test_host(monkeypatch)
    digest, output = dry_run_digest(project, clone, remote, capsys)
    assert f"refs/worktree/keep {keep} commit rescue->" in output
    assert run_clone(project, clone, remote, apply=True, digest=digest) == 0
    assert keep in rescue_refs(remote).values()


def test_refuses_a_shallow_or_partial_local_remote(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, remote, clone = make_clone(tmp_path)
    git(clone, "switch", "-q", "-c", "agent/work")
    for name in ("a.txt", "b.txt", "c.txt"):
        commit(clone, name)
    allow_test_host(monkeypatch)
    shallow = tmp_path / "mirror-shallow.git"
    subprocess.run(
        ["git", "clone", "-q", "--bare", "--depth", "1", "--no-local", f"file://{clone}", str(shallow)],
        check=True,
        capture_output=True,
    )
    assert run_clone(project, clone, remote, remote_url=str(shallow)) == 3
    assert "is a shallow repository" in combined(capsys)
    partial = tmp_path / "mirror-partial.git"
    subprocess.run(
        [
            "git",
            "-c",
            "uploadpack.allowFilter=true",
            "clone",
            "-q",
            "--bare",
            "--filter=blob:none",
            "--no-local",
            f"file://{clone}",
            str(partial),
        ],
        check=True,
        capture_output=True,
    )
    assert run_clone(project, clone, remote, remote_url=str(partial)) == 3
    assert "is a partial clone" in combined(capsys)
    assert clone.is_dir()


def test_an_fsmonitor_hook_cannot_hide_a_modified_file(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, remote, clone = make_clone(tmp_path)
    hook = tmp_path / "fsmonitor-hook"
    hook.write_text("#!/bin/sh\nprintf 'tok\\0'\n", encoding="utf-8")
    hook.chmod(0o755)
    # Past the racy-timestamp window, so the index entry is trusted as clean.
    time.sleep(1.2)
    git(clone, "config", "core.fsmonitor", str(hook))
    git(clone, "update-index", "--fsmonitor")
    git(clone, "status", "--porcelain")
    time.sleep(1.2)
    (clone / "seed.txt").write_text("hidden edit\n", encoding="utf-8")
    # The clone's own configuration hides the edit from an ordinary status.
    assert git(clone, "status", "--porcelain").stdout == ""
    allow_test_host(monkeypatch)
    assert run_clone(project, clone, remote) == 3
    assert "checkout . has 1 uncommitted or untracked path(s): seed.txt" in combined(capsys)
    assert clone.is_dir()


def test_refuses_staged_changes_in_a_git_directory_without_a_working_tree(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, _repository, remote = lifecycle.make_project(tmp_path)
    holder = lifecycle.slots_directory(project) / "holder"
    source = tmp_path / "source"
    subprocess.run(["git", "clone", "-q", str(remote), str(source)], check=True,
                   capture_output=True)
    git(source, "config", "user.name", "x")
    git(source, "config", "user.email", "x@example.invalid")
    (source / "staged.txt").write_text("only in the index\n", encoding="utf-8")
    git(source, "add", "staged.txt")
    holder.mkdir(parents=True)
    (source / ".git").rename(holder / "old.git")
    allow_test_host(monkeypatch)
    arguments = [
        "--project-root",
        str(project),
        "recover-ownerless-agent-clone",
        holder.relative_to(project).as_posix(),
        "--remote-url",
        f"old.git={remote}",
    ]
    assert wrkslots.main(arguments) == 3
    assert "has no working tree but its index holds staged changes" in combined(capsys)
    assert holder.is_dir()


def test_record_names_a_rescue_ref_left_by_an_abandoned_run(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, remote, clone = make_clone(tmp_path)
    git(clone, "switch", "-q", "-c", "agent/one")
    tip = commit(clone, "one.txt")
    allow_test_host(monkeypatch)
    digest, _output = dry_run_digest(project, clone, remote, capsys)

    def refuse_after_push(point: str) -> None:
        if point == "after-ownerless-clone-rescue":
            raise wrkslots.Refusal("simulated refusal after the push")

    monkeypatch.setattr(wrkslots, "_interrupt_for_test", refuse_after_push)
    assert run_clone(project, clone, remote, apply=True, digest=digest) == 3
    assert "removal journal was retired" in combined(capsys)
    [name] = rescue_refs(remote)
    monkeypatch.setattr(wrkslots, "_interrupt_for_test", lambda _point: None)
    digest, output = dry_run_digest(project, clone, remote, capsys)
    assert "rescue_objects=0" in output
    assert run_clone(project, clone, remote, apply=True, digest=digest) == 0
    record = wrkslots._as_mapping(removal_event(project)["record"], "record")
    [repository] = wrkslots._as_list(record["repositories"], "repositories")
    tips = {
        tip_record["label"]: tip_record["survives_as"]
        for tip_record in (
            wrkslots._as_mapping(value, "tip")
            for value in wrkslots._as_list(
                wrkslots._as_mapping(repository, "repository")["tips"], "tips"
            )
        )
    }
    assert tips["refs/heads/agent/one"] == f"kept-by:{name}"
    assert rescue_refs(remote) == {name: tip}


def test_refuses_dirty_worktrees_kept_inside_a_git_directory(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, _repository, remote = lifecycle.make_project(tmp_path)
    bare = lifecycle.slots_directory(project) / "bare-with-worktree"
    subprocess.run(["git", "clone", "-q", "--bare", str(remote), str(bare)], check=True,
                   capture_output=True)
    git(bare, "worktree", "add", "-q", "wt", "main")
    allow_test_host(monkeypatch)
    arguments = [
        "--project-root",
        str(project),
        "recover-ownerless-agent-clone",
        bare.relative_to(project).as_posix(),
        "--remote-url",
        f".={remote}",
    ]
    assert wrkslots.main(arguments) == 0
    assert "checkouts=1" in combined(capsys)
    (bare / "wt" / "seed.txt").write_text("edited\n", encoding="utf-8")
    (bare / "wt" / "brand-new.txt").write_text("new\n", encoding="utf-8")
    assert wrkslots.main(arguments) == 3
    output = combined(capsys)
    assert "checkout wt has 2 uncommitted" in output
    assert bare.is_dir()

    (tmp_path / "second").mkdir()
    project2, remote2, clone = make_clone(tmp_path / "second")
    git(clone, "worktree", "add", "-q", "--detach", str(clone / ".git" / "wt"))
    (clone / ".git" / "wt" / "seed.txt").write_text("edited\n", encoding="utf-8")
    assert run_clone(project2, clone, remote2) == 3
    assert "checkout .git/wt has 1 uncommitted" in combined(capsys)
    assert clone.is_dir()


def test_refuses_files_git_cannot_see_inside_an_uninitialised_gitlink(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, remote, clone = make_clone(tmp_path)
    seed = git(clone, "rev-parse", "HEAD").stdout.strip()
    git(clone, "update-index", "--add", "--cacheinfo", f"160000,{seed},sub")
    git(clone, "commit", "-q", "-m", "gitlink")
    git(clone, "push", "-q", "origin", "main")
    (clone / "sub").mkdir(exist_ok=True)
    (clone / "sub" / "notes.txt").write_text("only copy\n", encoding="utf-8")
    # Git reports nothing for the file.
    assert git(clone, "status", "--porcelain", "--untracked-files=all").stdout == ""
    allow_test_host(monkeypatch)
    assert_refused_and_preserved(project, clone, remote, capsys, "sub/notes.txt")


def test_refuses_a_tracked_file_under_a_cache_glob(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, _repository, remote = lifecycle.make_project(tmp_path, cache_globs=("target",))
    clone = lifecycle.slots_directory(project) / "leaked-clone"
    subprocess.run(["git", "clone", "-q", str(remote), str(clone)], check=True,
                   capture_output=True)
    git(clone, "config", "user.name", "x")
    git(clone, "config", "user.email", "x@example.invalid")
    (clone / "target").mkdir()
    commit(clone, "target/tracked.txt", "tracked\n")
    git(clone, "push", "-q", "origin", "main")
    (clone / "target" / "tracked.txt").write_text("edited\n", encoding="utf-8")
    allow_test_host(monkeypatch)
    assert_refused_and_preserved(project, clone, remote, capsys, "overlaps tracked source")


def test_core_ignorecase_cannot_hide_an_untracked_file(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, remote, clone = make_clone(tmp_path)
    git(clone, "config", "core.ignorecase", "true")
    (clone / "SEED.TXT").write_text("only copy\n", encoding="utf-8")
    allow_test_host(monkeypatch)
    assert_refused_and_preserved(project, clone, remote, capsys, "SEED.TXT")


def test_refuses_content_hidden_inside_git_directories(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, remote, clone = make_clone(tmp_path)
    allow_test_host(monkeypatch)
    # A file Git does not own at the top of the clone's Git directory.
    (clone / ".git" / "agent-notes.md").write_text("only copy\n", encoding="utf-8")
    assert_refused_and_preserved(project, clone, remote, capsys, ".git/agent-notes.md")
    (clone / ".git" / "agent-notes.md").unlink()
    # A whole repository tucked into the object store.
    stash = clone / ".git" / "objects" / "stash-clone"
    subprocess.run(["git", "clone", "-q", str(remote), str(stash)], check=True,
                   capture_output=True)
    assert_refused_and_preserved(project, clone, remote, capsys, ".git/objects/stash-clone")
    subprocess.run(["rm", "-rf", "--", str(stash)], check=True)
    # Something shaped like a Git directory inside the working tree, holding
    # files that are not Git's.
    (clone / ".git" / "info" / "exclude").write_text("scratch/\n", encoding="utf-8")
    scratch = clone / "scratch"
    (scratch / "objects").mkdir(parents=True)
    (scratch / "refs").mkdir()
    (scratch / "HEAD").write_text("ref: refs/heads/main\n", encoding="utf-8")
    (scratch / "precious-notes.md").write_text("only copy\n", encoding="utf-8")
    assert run_clone(project, clone, remote, "--remote-url", f"scratch={remote}") == 3
    assert "scratch/precious-notes.md" in combined(capsys)
    assert clone.is_dir()


def test_refuses_a_configured_content_filter(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, remote, clone = make_clone(tmp_path)
    git(clone, "config", "filter.keep.clean", "cat")
    allow_test_host(monkeypatch)
    assert_refused_and_preserved(project, clone, remote, capsys, "configures a content filter")


def test_a_commit_kept_only_by_orig_head_is_rescued(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, remote, clone = make_clone(tmp_path)
    dropped = commit(clone, "dropped.txt")
    git(clone, "reset", "-q", "--hard", "HEAD~1")
    allow_test_host(monkeypatch)
    digest, output = dry_run_digest(project, clone, remote, capsys)
    assert f"TIP ORIG_HEAD {dropped} commit rescue->" in output
    assert run_clone(project, clone, remote, apply=True, digest=digest) == 0
    assert dropped in rescue_refs(remote).values()


@pytest.mark.parametrize(
    "planted",
    [
        "objects/tmp_stash/precious.md",
        "objects/commit-graph/precious.md",
        "objects/info/packs/precious.md",
        "objects/ab/" + "c" * 38,
        "FETCH_HEAD/precious.md",
        "modules/fake/precious.md",
        "branches/precious.md",
        "lfs/tmp/precious.md",
        "precious.md.lock",
    ],
)
def test_refuses_anything_in_a_git_directory_not_shaped_like_git(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    planted: str,
) -> None:
    project, remote, clone = make_clone(tmp_path)
    path = clone / ".git" / planted
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("only copy\n", encoding="utf-8")
    allow_test_host(monkeypatch)
    assert_refused_and_preserved(project, clone, remote, capsys, "not Git's own")


def test_refuses_a_filter_in_a_linked_worktree_config_and_the_ident_attribute(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, remote, clone = make_clone(tmp_path)
    (clone / ".git" / "info" / "exclude").write_text("wt/\n", encoding="utf-8")
    git(clone, "worktree", "add", "-q", "--detach", str(clone / "wt"))
    git(clone, "config", "extensions.worktreeConfig", "true")
    git(clone / "wt", "config", "--worktree", "filter.keep.clean", "cat")
    allow_test_host(monkeypatch)
    assert_refused_and_preserved(project, clone, remote, capsys, "configures a content filter")
    git(clone / "wt", "config", "--worktree", "--unset", "filter.keep.clean")
    (clone / ".git" / "info" / "attributes").write_text("*.txt ident\n", encoding="utf-8")
    assert_refused_and_preserved(project, clone, remote, capsys, "ident attribute")


def test_refuses_ref_files_git_silently_skips(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, remote, clone = make_clone(tmp_path)
    git(clone, "switch", "-q", "-c", "agent/work")
    commit(clone, "work.txt")
    git(clone, "switch", "-q", "main")
    git(clone, "reflog", "expire", "--expire=now", "--all")
    heads = clone / ".git" / "refs" / "heads" / "agent"
    # What a ref update killed partway leaves: the only name is a lock file.
    (heads / "work").rename(heads / "work.lock")
    allow_test_host(monkeypatch)
    assert_refused_and_preserved(project, clone, remote, capsys, "refs/heads/agent/work.lock")
    (heads / "work.lock").unlink()
    (clone / ".git" / "refs" / "heads" / ".notes.md").write_text("only copy\n", encoding="utf-8")
    assert_refused_and_preserved(project, clone, remote, capsys, "refs/heads/.notes.md")


def test_refuses_a_ref_a_linked_worktree_never_reads(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, remote, clone = make_clone(tmp_path)
    (clone / ".git" / "info" / "exclude").write_text("wt/\n", encoding="utf-8")
    git(clone, "worktree", "add", "-q", "--detach", str(clone / "wt"))
    tip = commit(clone / "wt", "feature.txt")
    git(clone / "wt", "checkout", "-q", "--detach", "HEAD~1")
    git(clone, "reflog", "expire", "--expire=now", "--all")
    stray = clone / ".git" / "worktrees" / "wt" / "refs" / "heads" / "feature"
    stray.parent.mkdir(parents=True)
    stray.write_text(f"{tip}\n", encoding="utf-8")
    allow_test_host(monkeypatch)
    assert_refused_and_preserved(project, clone, remote, capsys, "worktrees/wt/refs/heads")


def test_a_clone_with_an_initialised_submodule_and_worktree_config_is_ready(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, remote, clone = make_clone(tmp_path)
    sub_remote = tmp_path / "sub.git"
    # Born on main whatever init.defaultBranch says: only main is pushed, and a remote whose HEAD
    # names an unborn branch gives the submodule nothing to check out.
    subprocess.run(
        ["git", "init", "-q", "--bare", "--initial-branch=main", str(sub_remote)], check=True
    )
    sub_source = tmp_path / "sub-source"
    subprocess.run(["git", "clone", "-q", str(sub_remote), str(sub_source)], check=True,
                   capture_output=True)
    git(sub_source, "config", "user.name", "x")
    git(sub_source, "config", "user.email", "x@example.invalid")
    commit(sub_source, "s.txt")
    git(sub_source, "push", "-q", "origin", "HEAD:refs/heads/main")
    git(clone, "-c", "protocol.file.allow=always", "submodule", "add", "-q", str(sub_remote), "subm")
    git(clone, "commit", "-q", "-m", "add submodule")
    git(clone, "push", "-q", "origin", "main")
    git(clone, "config", "extensions.worktreeConfig", "true")
    git(clone, "config", "--worktree", "core.sparseCheckout", "false")
    assert (clone / ".git" / "config.worktree").is_file()
    allow_test_host(monkeypatch)
    digest, output = dry_run_digest(
        project, clone, remote, capsys, "--remote-url", f".git/modules/subm={sub_remote}"
    )
    assert "REPOSITORY key=.git/modules/subm" in output and "checkouts=1" in output
    assert (
        run_clone(
            project,
            clone,
            remote,
            "--remote-url",
            f".git/modules/subm={sub_remote}",
            apply=True,
            digest=digest,
        )
        == 0
    )
    assert not clone.exists()


def test_a_submodule_initialised_in_a_linked_worktree_is_ready(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, remote, clone = make_clone(tmp_path)
    sub_remote = tmp_path / "sub.git"
    # Born on main whatever init.defaultBranch says: only main is pushed, and a remote whose HEAD
    # names an unborn branch gives the submodule nothing to check out.
    subprocess.run(
        ["git", "init", "-q", "--bare", "--initial-branch=main", str(sub_remote)], check=True
    )
    sub_source = tmp_path / "sub-source"
    subprocess.run(["git", "clone", "-q", str(sub_remote), str(sub_source)], check=True,
                   capture_output=True)
    git(sub_source, "config", "user.name", "x")
    git(sub_source, "config", "user.email", "x@example.invalid")
    commit(sub_source, "s.txt")
    git(sub_source, "push", "-q", "origin", "HEAD:refs/heads/main")
    git(clone, "-c", "protocol.file.allow=always", "submodule", "add", "-q", str(sub_remote), "subm")
    git(clone, "commit", "-q", "-m", "add submodule")
    git(clone, "push", "-q", "origin", "main")
    (clone / ".git" / "info" / "exclude").write_text("wt/\n", encoding="utf-8")
    git(clone, "worktree", "add", "-q", "--detach", str(clone / "wt"))
    git(clone / "wt", "-c", "protocol.file.allow=always", "submodule", "update", "--init", "-q")
    keys = sorted(
        path.relative_to(clone).as_posix() for path in (clone / ".git").rglob("modules/subm")
    )
    assert len(keys) == 2
    extra: list[str] = []
    for key in keys:
        extra += ["--remote-url", f"{key}={sub_remote}"]
    allow_test_host(monkeypatch)
    digest, output = dry_run_digest(project, clone, remote, capsys, *extra)
    assert "blockers=0" in output
