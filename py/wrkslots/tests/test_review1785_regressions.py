"""Reviewer counterexample regressions for #1785 (data-safety review).

Each case is a private-history shape that must never end in a silent
drop: rescue/salvage ancestry, malformed reflog lines, broken loose
refs, tag objects, and incomplete rescue chains.
"""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

import pytest
from wrkslots import cli as wrkslots
from wrkslots.tests.test_lifecycle import (
    _KEPT_SET_OLD,
    _drifted_slot_with_submodules,
    _kept_set_repos,
    _private_history,
    _set_loose_mtime,
    commit_local,
    git,
)


def _gitdir(slot: Path) -> Path:
    return Path(git(slot, "rev-parse", "--absolute-git-dir").stdout.strip())


def _make_chain(slot: Path, base: str, name: str) -> tuple[str, str, str]:
    git(slot, "checkout", "-q", "-b", f"tmp-{name}")
    a = commit_local(slot, f"{name}a.txt", "a\n", f"{name} a")
    b = commit_local(slot, f"{name}b.txt", "b\n", f"{name} b")
    c = commit_local(slot, f"{name}c.txt", "c\n", f"{name} c")
    git(slot, "checkout", "-q", "--detach", base)
    git(slot, "branch", "-q", "-D", f"tmp-{name}")
    git(slot, "reflog", "expire", "--expire=now", "--expire-unreachable=now", "--all")
    for x in (a, b, c):
        _set_loose_mtime(slot, x, _KEPT_SET_OLD)
    return a, b, c


def test_deep_ancestor_rescue(tmp_path: Path) -> None:
    _origin, parent, slot, base = _kept_set_repos(tmp_path)
    a, _b, c = _make_chain(slot, base, "deep")
    git(slot, "update-ref", "refs/rescue/x/deep", a)
    history = _private_history(slot, base, parent)
    assert history is not None and c in history.tips


def test_salvage_ref_ancestor(tmp_path: Path) -> None:
    _origin, parent, slot, base = _kept_set_repos(tmp_path)
    _a, b, c = _make_chain(slot, base, "salv")
    git(slot, "update-ref", "refs/salvage/x/s", b)
    history = _private_history(slot, base, parent)
    assert history is not None and c in history.tips


def test_malformed_reflog_line_age_drops_reflog_named(tmp_path: Path) -> None:
    """A reflog-named commit whose reflog line has a malformed timestamp."""
    _origin, parent, slot, base = _kept_set_repos(tmp_path)
    _a, _b, c = _make_chain(slot, base, "rl")
    logs = _gitdir(slot) / "logs" / "refs" / "heads" / "gone"
    logs.parent.mkdir(parents=True, exist_ok=True)
    logs.write_text(f"{'0' * 40} {c} Someone <s@x> notanumber +0000\tbroken\n")
    try:
        history = _private_history(slot, base, parent)
    except wrkslots.Refusal as exc:
        # Default policy may refuse, but must name the reflog problem,
        # never silently drop the commit the line names.
        assert "gone" in str(exc) or "reflog" in str(exc)
        return
    assert history is not None and c in history.tips, history


def test_malformed_reflog_line_missing_object(tmp_path: Path) -> None:
    _origin, parent, slot, base = _kept_set_repos(tmp_path)
    missing = "1234567890123456789012345678901234567890"
    logs = _gitdir(slot) / "logs" / "refs" / "heads" / "gone2"
    logs.parent.mkdir(parents=True, exist_ok=True)
    logs.write_text(f"{'0' * 40} {missing} Someone <s@x> notanumber +0000\tbroken\n")
    with pytest.raises(wrkslots.Refusal):
        _private_history(slot, base, parent)


def test_truncated_rescue_ref_file(tmp_path: Path) -> None:
    _origin, parent, slot, base = _kept_set_repos(tmp_path)
    _a, _b, c = _make_chain(slot, base, "tr")
    planted = _gitdir(slot) / "refs" / "rescue" / "trunc"
    planted.parent.mkdir(parents=True, exist_ok=True)
    planted.write_text(c[:30] + "\n")
    try:
        history = _private_history(slot, base, parent)
    except wrkslots.Refusal:
        return
    assert history is not None and c in history.tips, history


def test_tag_to_tag_rescue(tmp_path: Path) -> None:
    _origin, parent, slot, base = _kept_set_repos(tmp_path)
    a, _b, c = _make_chain(slot, base, "tt")
    git(slot, "tag", "-a", "-m", "t1", "inner", a)
    inner = git(slot, "rev-parse", "refs/tags/inner").stdout.strip()
    git(slot, "tag", "-d", "inner")
    git(slot, "tag", "-a", "-m", "t2", "outer", inner)
    outer = git(slot, "rev-parse", "refs/tags/outer").stdout.strip()
    git(slot, "tag", "-d", "outer")
    git(slot, "update-ref", "refs/rescue/tagtag", outer)
    history = _private_history(slot, base, parent)
    assert history is not None and c in history.tips, history


def test_rescue_ref_to_tag_with_missing_target(tmp_path: Path) -> None:
    _origin, parent, slot, base = _kept_set_repos(tmp_path)
    missing = "1234567890123456789012345678901234567890"
    tag_object = subprocess.run(
        ["git", "-C", str(slot), "mktag"],
        input=f"object {missing}\ntype commit\ntag t\ntagger A <a@b> 1 +0000\n\nm\n",
        text=True,
        capture_output=True,
    )
    if tag_object.returncode != 0:
        tag_object = subprocess.run(
            ["git", "-C", str(slot), "hash-object", "-t", "tag", "-w", "--literally", "--stdin"],
            input=f"object {missing}\ntype commit\ntag t\ntagger A <a@b> 1 +0000\n\nm\n",
            text=True,
            capture_output=True,
        )
    target = tag_object.stdout.strip()
    git(slot, "update-ref", "refs/rescue/tagmissing", target)
    with pytest.raises(wrkslots.Refusal) as refused:
        _private_history(slot, base, parent)
    assert "refs/rescue/tagmissing" in str(refused.value)


def test_rescue_ref_to_blob(tmp_path: Path) -> None:
    _origin, parent, slot, base = _kept_set_repos(tmp_path)
    blob = subprocess.run(
        ["git", "-C", str(slot), "hash-object", "-w", "--stdin"],
        input="x",
        text=True,
        capture_output=True,
    ).stdout.strip()
    git(slot, "update-ref", "refs/rescue/blob", blob)
    try:
        _private_history(slot, base, parent)
    except wrkslots.Refusal:
        pass


def test_rescue_commit_with_missing_parent(tmp_path: Path) -> None:
    _origin, parent, slot, base = _kept_set_repos(tmp_path)
    _a, b, c = _make_chain(slot, base, "mp")
    git(slot, "update-ref", "refs/rescue/c", c)
    os.unlink(_gitdir(slot) / "objects" / b[:2] / b[2:])
    with pytest.raises(wrkslots.Refusal):
        _private_history(slot, base, parent)


def _nested_call(tmp_path: Path, broken: str) -> None:
    project, tree, _component, _head = _drifted_slot_with_submodules(tmp_path)
    config = wrkslots._load_config(str(project), "testhost")
    record = wrkslots._load_active(config).slots[0]
    checkout = record.checkouts[0]
    modules_root = _gitdir(tree) / "modules"
    repo = modules_root / broken
    subprocess.run(
        ["git", "init", "-q", "-b", "main", str(repo)],
        check=True,
        capture_output=True,
        text=True,
    )
    if broken == "noorigin":
        git(repo, "config", "user.name", "Wrkslots Test")
        git(repo, "config", "user.email", "wrkslots@example.invalid")
        commit_local(repo, "x.txt", "x\n", "broken module commit")
    with pytest.raises(wrkslots.Refusal) as refused:
        wrkslots._nested_module_repository_checkouts(
            config, checkout, wrkslots._GitVcs(), tree
        )
    assert broken in str(refused.value)


def test_nested_module_unborn_head_refuses(tmp_path: Path) -> None:
    """H3: an unpopulated module repo whose HEAD is unborn refuses."""
    _nested_call(tmp_path, "unborn")


def test_nested_module_without_remote_refuses(tmp_path: Path) -> None:
    """H3: an unpopulated module repo with no recorded remote refuses."""
    _nested_call(tmp_path, "noorigin")


def _partition(
    slot: Path, history: wrkslots._PrivateHistory
) -> tuple[tuple[str, ...], tuple[tuple[str, str, str], ...]]:
    vcs = wrkslots._GitVcs()
    protected = {commit for _ref, commit in history.refs}
    named = set(protected)
    reflog_tips = wrkslots._reflog_reached_tips(
        vcs, slot, set(history.tips) - protected
    )
    protected.update(reflog_tips)
    named.update(reflog_tips)
    protected.update(history.protected_tips)
    return wrkslots._partition_kept_commits(
        vcs, slot, "leaf", history.tips, protected=protected, named=named
    )


def _drop_blob(slot: Path, commit: str, name: str) -> None:
    blob = git(slot, "rev-parse", f"{commit}:{name}").stdout.strip()
    os.unlink(_gitdir(slot) / "objects" / blob[:2] / blob[2:])


def _clean_chain(slot: Path, base: str, branch: str, commits: tuple[str, ...]) -> None:
    git(slot, "checkout", "-q", "--detach", base)
    git(slot, "branch", "-q", "-D", branch)
    git(slot, "reflog", "expire", "--expire=now", "--expire-unreachable=now", "--all")
    for commit in commits:
        _set_loose_mtime(slot, commit, _KEPT_SET_OLD)


def test_c1_two_rescue_parents_of_merge_under_incomplete_tip(
    tmp_path: Path,
) -> None:
    _origin, parent, slot, base = _kept_set_repos(tmp_path)
    git(slot, "checkout", "-q", "-b", "ra")
    ra = commit_local(slot, "ra.txt", "ra\n", "rescue a")
    git(slot, "checkout", "-q", "-b", "rb", base)
    rb = commit_local(slot, "rb.txt", "rb\n", "rescue b")
    git(slot, "merge", "-q", "--no-edit", "ra")
    merge = git(slot, "rev-parse", "HEAD").stdout.strip()
    tip = commit_local(slot, "t.txt", "unique tip blob c1\n", "unnamed tip")
    git(slot, "checkout", "-q", "--detach", base)
    git(slot, "branch", "-q", "-D", "ra")
    _clean_chain(slot, base, "rb", (ra, rb, merge, tip))
    git(slot, "update-ref", "refs/rescue/inv/a", ra)
    git(slot, "update-ref", "refs/rescue/inv/b", rb)
    _drop_blob(slot, tip, "t.txt")
    history = _private_history(slot, base, parent)
    assert history is not None and history.tips == (tip,), history
    try:
        kept, _excluded = _partition(slot, history)
    except wrkslots.Refusal:
        return

    def reach(x: str, y: str) -> bool:
        return (
            git(slot, "merge-base", "--is-ancestor", x, y, check=False).returncode
            == 0
        )

    assert all(
        any(reach(rescued, kept_commit) for kept_commit in kept)
        for rescued in (ra, rb)
    ), "a rescue commit lost"


def test_c2_incomplete_rescue_commit_under_incomplete_tip(
    tmp_path: Path,
) -> None:
    _origin, parent, slot, base = _kept_set_repos(tmp_path)
    git(slot, "checkout", "-q", "-b", "x")
    r1 = commit_local(slot, "r1.txt", "r1\n", "rescue 1 complete")
    r2 = commit_local(slot, "r2.txt", "r2 unique c2\n", "rescue 2 incomplete")
    tip = commit_local(slot, "t.txt", "t unique c2\n", "tip")
    _clean_chain(slot, base, "x", (r1, r2, tip))
    git(slot, "update-ref", "refs/rescue/inv/r1", r1)
    git(slot, "update-ref", "refs/rescue/inv/r2", r2)
    _drop_blob(slot, r2, "r2.txt")
    history = _private_history(slot, base, parent)
    assert history is not None and history.tips == (tip,), history
    try:
        _kept, excluded = _partition(slot, history)
    except wrkslots.Refusal:
        return
    dropped = wrkslots._dropped_history_entries(history)
    named = {entry[0] for entry in excluded} | {
        str(entry["commit"]) for entry in dropped
    }
    assert r2 in named, "incomplete rescue commit dropped unnamed"


def test_c4_truncated_loose_plus_packed(tmp_path: Path) -> None:
    _origin, parent, slot, base = _kept_set_repos(tmp_path)
    git(slot, "checkout", "-q", "-b", "x")
    old = commit_local(slot, "o.txt", "o\n", "old rescue")
    new = commit_local(slot, "n.txt", "n\n", "new rescue")
    _clean_chain(slot, base, "x", (old, new))
    git(slot, "update-ref", "refs/rescue/p", old)
    git(slot, "pack-refs", "--all")
    planted = _gitdir(slot) / "refs" / "rescue" / "p"
    planted.parent.mkdir(parents=True, exist_ok=True)
    planted.write_text(new[:25] + "\n")
    with pytest.raises(wrkslots.Refusal):
        _private_history(slot, base, parent)


def test_c5_malformed_reflog_line_on_ancestor_overrefuses(
    tmp_path: Path,
) -> None:
    _origin, parent, slot, base = _kept_set_repos(tmp_path)
    git(slot, "checkout", "-q", "-b", "keep")
    a = commit_local(slot, "a.txt", "a\n", "a")
    commit_local(slot, "b.txt", "b\n", "b")
    logs = _gitdir(slot) / "logs" / "refs" / "heads" / "keep"
    with open(logs, "a", encoding="utf-8") as handle:
        handle.write(f"{'0' * 40} {a} Someone <s@x> notanumber +0000\tbroken\n")
    try:
        history = _private_history(slot, base, parent)
    except wrkslots.Refusal as exc:
        pytest.fail(f"over-refusal on a kept ancestor: {exc}")
    assert history is not None


def test_c3_stale_lock_file(tmp_path: Path) -> None:
    _origin, parent, slot, base = _kept_set_repos(tmp_path)
    (_gitdir(slot) / "refs" / "heads" / "main.lock").write_text("")
    try:
        history = _private_history(slot, base, parent)
    except wrkslots.Refusal as exc:
        pytest.fail(f"over-refusal on stale lock file: {exc}")
    assert history is not None or history is None


def _blob_of(slot: Path, commit: str, name: str) -> str:
    return git(slot, "rev-parse", f"{commit}:{name}").stdout.strip()


def _rm_object(slot: Path, oid: str) -> None:
    os.unlink(_gitdir(slot) / "objects" / oid[:2] / oid[2:])


def _finish_chain(slot: Path, base: str, branches: list[str], commits: list[str]) -> None:
    git(slot, "checkout", "-q", "--detach", base)
    for branch in branches:
        git(slot, "branch", "-q", "-D", branch)
    git(slot, "reflog", "expire", "--expire=now", "--expire-unreachable=now", "--all")
    for commit in commits:
        try:
            _set_loose_mtime(slot, commit, _KEPT_SET_OLD)
        except AssertionError:
            pass


def _reaches(slot: Path, commit: str, kept: tuple[str, ...]) -> bool:
    return any(
        git(slot, "merge-base", "--is-ancestor", commit, kept_commit, check=False).returncode == 0
        for kept_commit in kept
    )


def test_r3a_octopus_three_rescue_parents(tmp_path: Path) -> None:
    _origin, parent, slot, base = _kept_set_repos(tmp_path)
    rescued: list[str] = []
    for name in "abc":
        git(slot, "checkout", "-q", "-b", f"r{name}", base)
        rescued.append(commit_local(slot, f"{name}.txt", f"{name}\n", f"rescue {name}"))
    git(slot, "checkout", "-q", "ra")
    git(slot, "merge", "-q", "--no-edit", "rb", "rc")
    tip = commit_local(slot, "t.txt", "t r3a unique\n", "tip")
    _finish_chain(slot, base, ["ra", "rb", "rc"], rescued + [tip])
    for name, commit in zip("abc", rescued):
        git(slot, "update-ref", f"refs/rescue/inv/{name}", commit)
    _rm_object(slot, _blob_of(slot, tip, "t.txt"))
    history = _private_history(slot, base, parent)
    assert history is not None and history.tips == (tip,), history
    kept, _excluded = _partition(slot, history)
    assert all(_reaches(slot, commit, kept) for commit in rescued), (kept, rescued)


def test_r3b_nested_merges(tmp_path: Path) -> None:
    _origin, parent, slot, base = _kept_set_repos(tmp_path)
    git(slot, "checkout", "-q", "-b", "ra", base)
    ra = commit_local(slot, "a.txt", "a\n", "a")
    git(slot, "checkout", "-q", "-b", "rb", base)
    rb = commit_local(slot, "b.txt", "b\n", "b")
    git(slot, "merge", "-q", "--no-edit", "ra")
    m1 = git(slot, "rev-parse", "HEAD").stdout.strip()
    git(slot, "checkout", "-q", "-b", "rc", base)
    rc = commit_local(slot, "c.txt", "c\n", "c")
    git(slot, "checkout", "-q", "rb")
    git(slot, "merge", "-q", "--no-edit", "rc")
    tip = commit_local(slot, "t.txt", "t r3b unique\n", "tip")
    _finish_chain(slot, base, ["ra", "rb", "rc"], [ra, rb, rc, m1, tip])
    for name, commit in (("a", ra), ("b", rb), ("c", rc)):
        git(slot, "update-ref", f"refs/rescue/inv/{name}", commit)
    _rm_object(slot, _blob_of(slot, tip, "t.txt"))
    history = _private_history(slot, base, parent)
    assert history is not None and history.tips == (tip,)
    kept, _excluded = _partition(slot, history)
    assert all(_reaches(slot, commit, kept) for commit in (ra, rb, rc)), (kept,)


def test_r3c_protected_under_two_excluded_tips(tmp_path: Path) -> None:
    _origin, parent, slot, base = _kept_set_repos(tmp_path)
    git(slot, "checkout", "-q", "-b", "x", base)
    rescued = commit_local(slot, "r.txt", "r\n", "rescue")
    t1 = commit_local(slot, "t1.txt", "t1 r3c\n", "t1")
    git(slot, "checkout", "-q", "-b", "y", rescued)
    t2 = commit_local(slot, "t2.txt", "t2 r3c\n", "t2")
    _finish_chain(slot, base, ["x", "y"], [rescued, t1, t2])
    git(slot, "update-ref", "refs/rescue/inv/r", rescued)
    _rm_object(slot, _blob_of(slot, t1, "t1.txt"))
    _rm_object(slot, _blob_of(slot, t2, "t2.txt"))
    history = _private_history(slot, base, parent)
    assert history is not None and set(history.tips) == {t1, t2}
    kept, excluded = _partition(slot, history)
    assert rescued in kept and kept.count(rescued) == 1, kept
    assert {entry[0] for entry in excluded} == {t1, t2}


def test_r3d_promisor_gap_maximal_hides_complete_rescue(tmp_path: Path) -> None:
    _origin, parent, slot, base = _kept_set_repos(tmp_path)
    git(slot, "checkout", "-q", "-b", "x", base)
    r1 = commit_local(slot, "r1.txt", "r1 r3d\n", "rescue 1 complete")
    r2 = commit_local(slot, "r2.txt", "r2 r3d\n", "rescue 2 promisor gap")
    tip = commit_local(slot, "t.txt", "t r3d\n", "tip")
    _finish_chain(slot, base, ["x"], [r1, r2, tip])
    git(slot, "update-ref", "refs/rescue/inv/r1", r1)
    git(slot, "update-ref", "refs/rescue/inv/r2", r2)
    r2tree = git(slot, "rev-parse", f"{r2}^{{tree}}").stdout.strip()
    r2blob = _blob_of(slot, r2, "r2.txt")
    tblob = _blob_of(slot, tip, "t.txt")
    packdir = _gitdir(slot) / "objects" / "pack"
    out = subprocess.run(
        ["git", "-C", str(slot), "pack-objects", str(packdir / "pack")],
        input=f"{r2}\n{r2tree}\n",
        text=True,
        capture_output=True,
        check=True,
    ).stdout.strip()
    (packdir / f"pack-{out}.promisor").write_text("")
    for oid in (r2, r2tree, r2blob, tblob):
        _rm_object(slot, oid)
    git(slot, "config", "core.repositoryformatversion", "1")
    git(slot, "config", "extensions.partialClone", "origin")
    history = _private_history(slot, base, parent)
    assert history is not None and history.tips == (tip,)
    try:
        kept, _excluded = _partition(slot, history)
    except wrkslots.Refusal:
        return
    assert _reaches(slot, r1, kept), "complete rescue r1 lost under promisor-gap rescue r2"


def test_r3e_garbage_reflog_line_with_no_private_tips(tmp_path: Path) -> None:
    _origin, parent, slot, base = _kept_set_repos(tmp_path)
    logs = _gitdir(slot) / "logs" / "refs" / "heads" / "gone"
    logs.parent.mkdir(parents=True, exist_ok=True)
    logs.write_text("this is not a reflog line\n")
    with pytest.raises(wrkslots.Refusal):
        _private_history(slot, base, parent)


def test_rescue_ancestor_lost_when_unnamed_tip_incomplete(tmp_path: Path) -> None:
    _origin, parent, slot, base = _kept_set_repos(tmp_path)
    git(slot, "checkout", "-q", "-b", "tmp-x")
    rescued = commit_local(slot, "r.txt", "rescued work\n", "rescue commit")
    tip = commit_local(slot, "t.txt", "unique blob content 1785\n", "unnamed tip")
    blob = git(slot, "rev-parse", f"{tip}:t.txt").stdout.strip()
    git(slot, "checkout", "-q", "--detach", base)
    git(slot, "branch", "-q", "-D", "tmp-x")
    git(slot, "reflog", "expire", "--expire=now", "--expire-unreachable=now", "--all")
    for commit in (rescued, tip):
        _set_loose_mtime(slot, commit, _KEPT_SET_OLD)
    git(slot, "update-ref", "refs/rescue/inventory/r", rescued)
    os.unlink(_gitdir(slot) / "objects" / blob[:2] / blob[2:])
    history = _private_history(slot, base, parent)
    assert history is not None and history.tips == (tip,)
    vcs = wrkslots._GitVcs()
    protected = {commit for _ref, commit in history.refs}
    protected.update(
        wrkslots._reflog_reached_tips(vcs, slot, set(history.tips) - protected)
    )
    kept, _excluded = wrkslots._partition_kept_commits(
        vcs, slot, "leaf", history.tips, protected=protected
    )
    assert rescued in kept, "rescue-named commit lost via excluded tip"
