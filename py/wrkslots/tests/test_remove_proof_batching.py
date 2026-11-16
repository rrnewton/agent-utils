"""#1681: advertised-tips proof is asked once per removal; parent death stops a remove."""

from __future__ import annotations

import contextlib
import subprocess
import sys
import time
from pathlib import Path
from typing import Mapping, Sequence

import pytest
from wrkslots import cli as wrkslots


def _git(cwd: Path, *args: str) -> str:
    result = subprocess.run(
        ["git", "-C", str(cwd), *args],
        capture_output=True, text=True, check=True, timeout=60,
    )
    return result.stdout.strip()


def _repo(tmp_path: Path) -> Path:
    repo = tmp_path / "repo"
    repo.mkdir()
    _git(repo, "init", "-q", "-b", "main")
    _git(repo, "config", "user.email", "t@e.invalid")
    _git(repo, "config", "user.name", "T")
    (repo / "f.txt").write_text("x\n")
    _git(repo, "add", "f.txt")
    _git(repo, "commit", "-q", "-m", "one")
    _git(repo, "remote", "add", "origin", str(repo))
    return repo


def test_advertised_tips_memoized_within_removal_scope(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = _repo(tmp_path)
    vcs = wrkslots._GitVcs()
    authority = vcs.remote_authority(repo, "origin")
    calls: list[str] = []
    original = wrkslots._GitVcs._ls_remote_inventory

    def counting(self: wrkslots._GitVcs, *args: object, **kwargs: object) -> dict[str, str]:
        calls.append("ls-remote")
        return original(self, *args, **kwargs)  # type: ignore[arg-type]

    monkeypatch.setattr(wrkslots._GitVcs, "_ls_remote_inventory", counting)
    with wrkslots._remote_containment_scope():
        first = vcs.remote_advertised_tips(repo, "origin", authority)
        second = vcs.remote_advertised_tips(repo, "origin", authority)
    assert first == second and first
    assert calls == ["ls-remote"]


def test_advertised_tips_not_memoized_outside_scope(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = _repo(tmp_path)
    vcs = wrkslots._GitVcs()
    authority = vcs.remote_authority(repo, "origin")
    calls: list[str] = []
    original = wrkslots._GitVcs._ls_remote_inventory

    def counting(self: wrkslots._GitVcs, *args: object, **kwargs: object) -> dict[str, str]:
        calls.append("ls-remote")
        return original(self, *args, **kwargs)  # type: ignore[arg-type]

    monkeypatch.setattr(wrkslots._GitVcs, "_ls_remote_inventory", counting)
    vcs.remote_advertised_tips(repo, "origin", authority)
    vcs.remote_advertised_tips(repo, "origin", authority)
    assert calls == ["ls-remote", "ls-remote"]


def _mirror_scope(root: Path) -> contextlib.AbstractContextManager[None]:
    return wrkslots._remote_containment_scope(root)


def test_retire_proof_mirrors_idle_orphaned_and_locked(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import fcntl as _fcntl
    import json as _json
    import os as _os

    config = object.__new__(wrkslots.Config)
    object.__setattr__(config, "control", tmp_path)
    root = tmp_path / "proof-mirrors"
    now = 2_000_000_000.0

    def make(name: str, fetched_at: float) -> Path:
        entry = root / name
        entry.mkdir(parents=True)
        (entry / "record.json").write_text(
            _json.dumps({"fetched_at": fetched_at})
        )
        return entry

    idle = make("idle", now - 8 * 24 * 3600)
    orphan = make("orphan", now)
    kept = make("kept", now)
    locked = make("locked", now - 8 * 24 * 3600)
    descriptor = _os.open(locked / ".lock", _os.O_RDWR | _os.O_CREAT, 0o600)
    _fcntl.flock(descriptor, _fcntl.LOCK_EX)
    try:
        monkeypatch.setattr(
            wrkslots, "_live_remote_digests", lambda cfg: {"kept", "locked"}
        )
        dropped = wrkslots._retire_proof_mirrors(config, now=now)
    finally:
        _os.close(descriptor)
    assert dropped == ["idle", "orphan"]
    assert not idle.exists() and not orphan.exists()
    assert kept.exists() and locked.exists()


def test_mirror_serves_second_proof_without_network(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = _repo(tmp_path)
    vcs = wrkslots._GitVcs()
    authority = vcs.remote_authority(repo, "origin")
    root = tmp_path / "proof-mirrors"
    with _mirror_scope(root):
        first = vcs._proof_mirror(repo, authority)
        assert first is not None and first[0]

        def no_network(*args: object, **kwargs: object) -> dict[str, str]:
            raise AssertionError("network used for a fresh mirror proof")

        monkeypatch.setattr(wrkslots._GitVcs, "_ls_remote_inventory", no_network)
        second = vcs._proof_mirror(repo, authority)
    assert second is not None and second[0] == first[0]


def test_concurrent_removals_share_one_mirror_refresh(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import contextvars
    import threading

    repo = _repo(tmp_path)
    vcs = wrkslots._GitVcs()
    authority = vcs.remote_authority(repo, "origin")
    root = tmp_path / "proof-mirrors"
    calls: list[str] = []
    entered = threading.Event()
    release = threading.Event()
    original = wrkslots._GitVcs._ls_remote_inventory

    def counting(self: wrkslots._GitVcs, *args: object, **kwargs: object) -> dict[str, str]:
        calls.append("ls-remote")
        entered.set()
        release.wait(timeout=30)
        return original(self, *args, **kwargs)  # type: ignore[arg-type]

    monkeypatch.setattr(wrkslots._GitVcs, "_ls_remote_inventory", counting)
    results: list[object] = []

    def ask(ctx: contextvars.Context) -> None:
        results.append(ctx.run(vcs._proof_mirror, repo, authority))

    with _mirror_scope(root):
        ctx = contextvars.copy_context()
        threads = [threading.Thread(target=ask, args=(ctx,)) for _ in range(2)]
        # Each thread needs its own copy of the scope context.
        threads = [
            threading.Thread(target=ask, args=(contextvars.copy_context(),))
            for _ in range(2)
        ]
        for thread in threads:
            thread.start()
        assert entered.wait(timeout=30)
        release.set()
        for thread in threads:
            thread.join(timeout=60)
    assert all(result is not None for result in results)
    assert calls == ["ls-remote"]


def test_stale_mirror_with_unreachable_remote_falls_back(tmp_path: Path) -> None:
    import hashlib as _hashlib

    repo = _repo(tmp_path)
    vcs = wrkslots._GitVcs()
    url = str(tmp_path / "no-such-remote")
    authority = wrkslots._RemoteAuthority(
        url, _hashlib.sha256(url.encode()).hexdigest()
    )
    with _mirror_scope(tmp_path / "proof-mirrors"):
        assert vcs._proof_mirror(repo, authority) is None


def test_corrupt_mirror_is_discarded(tmp_path: Path) -> None:
    repo = _repo(tmp_path)
    vcs = wrkslots._GitVcs()
    authority = vcs.remote_authority(repo, "origin")
    root = tmp_path / "proof-mirrors"
    with _mirror_scope(root):
        first = vcs._proof_mirror(repo, authority)
        assert first is not None
        directory = wrkslots._proof_mirror_directory(root, authority.url)
        (directory / "mirror.git").rename(directory / "mirror.git.gone")
        assert vcs._proof_mirror(repo, authority) is None
        assert not directory.exists()


def test_mirror_refresh_picks_up_force_push_past_bound(tmp_path: Path) -> None:
    import json as _json
    import time as _time

    repo = _repo(tmp_path)
    vcs = wrkslots._GitVcs()
    authority = vcs.remote_authority(repo, "origin")
    root = tmp_path / "proof-mirrors"
    with _mirror_scope(root):
        first = vcs._proof_mirror(repo, authority)
        assert first is not None
        old_tip = first[0]["refs/heads/main"]
        (repo / "h.txt").write_text("z\n")
        _git(repo, "add", "h.txt")
        _git(repo, "commit", "-q", "-m", "three")
        new_tip = _git(repo, "rev-parse", "HEAD")
        assert new_tip != old_tip
        directory = wrkslots._proof_mirror_directory(root, authority.url)
        record = _json.loads((directory / "record.json").read_text())
        record["fetched_at"] = _time.time() - 10000
        (directory / "record.json").write_text(_json.dumps(record))
        wrkslots._PROOF_MIRROR_ATTEMPTS.get().clear()  # type: ignore[union-attr]
        second = vcs._proof_mirror(repo, authority)
    assert second is not None and second[0]["refs/heads/main"] == new_tip


def test_thin_bundle_verify_skips_full_fsck(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import hashlib as _hashlib

    repo = _repo(tmp_path)
    base = _git(repo, "rev-parse", "HEAD")
    (repo / "g.txt").write_text("y\n")
    _git(repo, "add", "g.txt")
    _git(repo, "commit", "-q", "-m", "two")
    head = _git(repo, "rev-parse", "HEAD")
    ref = "refs/heads/wrkslots-salvage/test"
    _git(repo, "update-ref", ref, head)
    bundle = tmp_path / "thin.bundle"
    _git(repo, "bundle", "create", str(bundle), ref, f"^{base}")
    data = bundle.read_bytes()
    seen: list[str] = []
    original_run = wrkslots._GitVcs._run

    def recording(repository: Path, args: Sequence[str], **kwargs: object) -> object:
        seen.extend(args)
        return original_run(repository, args, **kwargs)  # type: ignore[arg-type]

    monkeypatch.setattr(wrkslots._GitVcs, "_run", staticmethod(recording))
    wrkslots._verify_local_salvage_bundle(
        bundle,
        _hashlib.sha256(data).hexdigest(),
        len(data),
        ref,
        head,
        wrkslots._GitVcs(),
        prerequisites=(base,),
        prerequisite_stores=(repo / ".git" / "objects",),
    )
    assert "fsck" not in seen


def test_thin_bundle_verify_fails_closed_without_prerequisite(tmp_path: Path) -> None:
    import hashlib as _hashlib

    repo = _repo(tmp_path)
    base = _git(repo, "rev-parse", "HEAD")
    (repo / "g.txt").write_text("y\n")
    _git(repo, "add", "g.txt")
    _git(repo, "commit", "-q", "-m", "two")
    head = _git(repo, "rev-parse", "HEAD")
    ref = "refs/heads/wrkslots-salvage/test"
    _git(repo, "update-ref", ref, head)
    bundle = tmp_path / "thin.bundle"
    _git(repo, "bundle", "create", str(bundle), ref, f"^{base}")
    empty = tmp_path / "empty.git"
    _git(tmp_path, "init", "--bare", "-q", str(empty))
    data = bundle.read_bytes()
    with pytest.raises(wrkslots.Refusal):
        wrkslots._verify_local_salvage_bundle(
            bundle,
            _hashlib.sha256(data).hexdigest(),
            len(data),
            ref,
            head,
            wrkslots._GitVcs(),
            prerequisites=(base,),
            prerequisite_stores=(empty / "objects",),
        )


def test_expired_heads_evidence_revalidated_without_fetch(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = _repo(tmp_path)
    vcs = wrkslots._GitVcs()
    authority = vcs.remote_authority(repo, "origin")
    head = _git(repo, "rev-parse", "HEAD")
    calls: list[str] = []
    original = wrkslots._GitVcs._ls_remote_inventory

    def counting(self: wrkslots._GitVcs, *args: object, **kwargs: object) -> dict[str, str]:
        calls.append("ls-remote")
        return original(self, *args, **kwargs)  # type: ignore[arg-type]

    def no_fetch(*args: object, **kwargs: object) -> object:
        raise AssertionError("proof fetch ran during fingerprint revalidation")

    monkeypatch.setattr(wrkslots._GitVcs, "_ls_remote_inventory", counting)
    monkeypatch.setattr(wrkslots._GitVcs, "_fetched_proof_store", no_fetch)
    with wrkslots._remote_containment_scope():
        first = vcs.remote_heads_containing(repo, "origin", head, authority)
        assert first
        memo = wrkslots._REMOTE_CONTAINMENT_MEMO.get()
        assert memo is not None
        for key, evidence in list(memo.items()):
            memo[key] = wrkslots._RemoteContainmentEvidence(
                evidence.refs, 0.0, evidence.refusal, evidence.extra, evidence.fingerprint
            )
        before = len(calls)
        second = vcs.remote_heads_containing(repo, "origin", head, authority)
    assert second == first
    assert len(calls) - before == 1


@pytest.mark.skipif(sys.platform != "linux", reason="prctl parent-death signal is Linux-only")
def test_parent_death_signal_stops_child(tmp_path: Path) -> None:
    py_dir = Path(wrkslots.__file__).resolve().parent.parent
    child_code = (
        "import sys, time; sys.path.insert(0, {d!r}); "
        "from wrkslots.cli import _install_parent_death_signal; "
        "_install_parent_death_signal(); print('ready', flush=True); time.sleep(60)"
    ).format(d=str(py_dir))
    wrapper = tmp_path / "wrapper.py"
    wrapper.write_text(
        "import subprocess, sys\n"
        f"p = subprocess.Popen([sys.executable, '-c', {child_code!r}], stdout=subprocess.PIPE, text=True)\n"
        "assert p.stdout is not None and p.stdout.readline().strip() == 'ready'\n"
        "print(p.pid, flush=True)\n"
    )
    proc = subprocess.Popen(
        [sys.executable, str(wrapper)], stdout=subprocess.PIPE, text=True
    )
    assert proc.stdout is not None
    child_pid = int(proc.stdout.readline().strip())
    proc.wait(timeout=30)
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        try:
            Path(f"/proc/{child_pid}").stat()
        except FileNotFoundError:
            break
        time.sleep(0.1)
    else:
        subprocess.run(["kill", "-9", str(child_pid)], check=False)
        raise AssertionError("child survived its parent's death")
