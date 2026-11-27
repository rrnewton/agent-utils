"""The content-keyed host cache of validation adapter executables (#2340).

A temporary Git repository stands in for the workspace: crate ``b`` (the package built)
depends on crate ``a``, has a dev-only dependency on crate ``c``, and its build reads
``py/extra.txt`` from outside every crate, as wrkvizd embeds files from ``py/wrkviz/``. A fake
build writes the executable plus the dep-info and build-script output Cargo would.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import time
from collections.abc import Iterator
from pathlib import Path

import pytest

from tests import adapter_cache
from tests.adapter_cache import Spec


def _git(repo: Path, *args: str) -> None:
    subprocess.run(["git", "-C", str(repo), *args], check=True, capture_output=True)


def _commit(repo: Path, message: str = "change") -> None:
    _git(repo, "add", "-A")
    _git(repo, "commit", "-q", "-m", message)


@pytest.fixture
def repo(tmp_path: Path) -> Path:
    root = tmp_path / "repo"
    files = {
        "rs/Cargo.toml": "[workspace]\n",
        "rs/Cargo.lock": "# lock\n",
        "rs/a/Cargo.toml": "[package]\nname = 'a'\n",
        "rs/a/src/lib.rs": "pub fn a() {}\n",
        "rs/b/Cargo.toml": "[package]\nname = 'b'\n",
        "rs/b/src/main.rs": "fn main() {}\n",
        "rs/c/Cargo.toml": "[package]\nname = 'c'\n",
        "rs/c/src/lib.rs": "pub fn c() {}\n",
        "py/extra.txt": "embedded\n",
        ".gitignore": "rs/target/\n",
    }
    for name, text in files.items():
        (root / name).parent.mkdir(parents=True, exist_ok=True)
        (root / name).write_text(text)
    _git(root, "init", "-q")
    _git(root, "config", "user.email", "test@example.invalid")
    _git(root, "config", "user.name", "test")
    _commit(root, "initial")
    return root


def _metadata(repo: Path) -> dict[str, object]:
    def package(name: str, source: str | None = None) -> dict[str, object]:
        return {"id": f"{name}-id", "name": name, "source": source,
                "manifest_path": str(repo / "rs" / name / "Cargo.toml")}

    return {
        "packages": [package("a"), package("b"), package("c"),
                     package("serde", "registry+https://example.invalid")],
        "resolve": {"nodes": [
            {"id": "b-id", "deps": [
                {"pkg": "a-id", "dep_kinds": [{"kind": None}]},
                {"pkg": "c-id", "dep_kinds": [{"kind": "dev"}]},
                {"pkg": "serde-id", "dep_kinds": [{"kind": None}]},
            ]},
            {"id": "a-id", "deps": []},
            {"id": "c-id", "deps": []},
            {"id": "serde-id", "deps": []},
        ]},
    }


@pytest.fixture(autouse=True)
def _isolated(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> Iterator[None]:
    monkeypatch.setattr(adapter_cache, "cargo_metadata", _metadata)
    monkeypatch.setattr(adapter_cache, "toolchain", lambda repo: "rustc 1.0\ncargo 1.0\n")
    admission = tmp_path / "admission"
    for folder in ("queue", "runs", "deferred"):
        (admission / folder).mkdir(parents=True)
    monkeypatch.setenv("GATE_ADMISSION_DIR", str(admission))
    monkeypatch.setattr(adapter_cache, "EVENTS", [])
    yield


class FakeBuild:
    """Writes ``b`` (bytes vary per build) with the dep-info and build output Cargo would."""

    def __init__(self, repo: Path) -> None:
        self.repo = repo
        self.target = repo / "rs" / "target" / "validation-adapters" / "b"
        self.count = 0

    def __call__(self) -> dict[str, Path]:
        self.count += 1
        release = self.target / "release"
        release.mkdir(parents=True, exist_ok=True)
        executable = release / "b"
        executable.write_text(f"#!/bin/sh\necho build {self.count}\n")
        executable.chmod(0o755)
        deps = [self.repo / "rs/b/src/main.rs", self.repo / "rs/a/src/lib.rs",
                self.repo / "py/extra.txt", Path("/outside/registry/serde/lib.rs"),
                self.target / "release/build/b-1/out/generated.rs"]
        (release / "b.d").write_text(f"{executable}: " + " ".join(map(str, deps)) + "\n")
        output = release / "build" / "b-1" / "output"
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text("cargo:rerun-if-changed=build.rs\ncargo:rerun-if-env-changed=MY_FLAG\n")
        return {"b": executable}


def _spec(**env: str) -> Spec:
    return Spec(name="b", packages=("b",), binaries=("b",),
                cargo_args=("build", "--release", "--locked", "-p", "b"), env=env)


def _fetch(repo: Path, root: Path, build: FakeBuild, spec: Spec | None = None) -> dict[str, Path]:
    return adapter_cache.fetch_or_build(spec or _spec(), build, build.target, repo=repo, root=root)


def _entries(root: Path) -> list[Path]:
    return sorted(path for path in (root / "b").iterdir() if path.is_dir())


def test_a_clean_tree_hit_runs_no_build(repo: Path, tmp_path: Path) -> None:
    root, build = tmp_path / "cache", FakeBuild(repo)
    built = _fetch(repo, root, build)
    assert build.count == 1 and built["b"].parent == build.target / "release"
    [entry] = _entries(root)
    hit = _fetch(repo, root, build)
    assert build.count == 1
    # The hit is the build's own path in this worktree, never the entry in the host cache.
    assert hit["b"] == built["b"] == build.target / "release" / "b"
    assert hit["b"].read_text() == (entry / "b").read_text()
    assert not os.access(entry / "b", os.W_OK)
    assert adapter_cache.EVENTS[-1].startswith("b hit ")
    assert "cache " in adapter_cache.summary() and "b hit" in adapter_cache.summary()


class WalkingBuild(FakeBuild):
    """An executable that finds ``py/extra.txt`` by walking up from its own path, as wrkslotsd
    finds ``py/wrkslots/__main__.py``; ``embed`` also writes the build worktree's path into it,
    as ``env!("CARGO_MANIFEST_DIR")`` does in wrkvizd."""

    def __init__(self, repo: Path, *, embed: bool = False) -> None:
        super().__init__(repo)
        self.embed = embed

    def __call__(self) -> dict[str, Path]:
        built = super().__call__()
        lines = [
            "#!/bin/sh",
            'd=$(dirname "$0")',
            'while [ "$d" != / ]; do',
            '  [ -f "$d/py/extra.txt" ] && { echo "$d"; exit 0; }',
            '  d=$(dirname "$d")',
            "done",
            "exit 1",
        ]
        if self.embed:
            lines.insert(1, f"# built in {self.repo.resolve()}")
        built["b"].write_text("\n".join(lines) + "\n")
        return built


def _clone(repo: Path, tmp_path: Path, name: str) -> Path:
    other = tmp_path / name
    subprocess.run(["git", "clone", "-q", str(repo), str(other)], check=True, capture_output=True)
    return other


def _found_from(executable: Path) -> Path:
    result = subprocess.run([str(executable)], check=True, capture_output=True, text=True)
    return Path(result.stdout.strip())


def test_a_hit_runs_where_a_build_would_so_self_located_files_resolve(
    repo: Path, tmp_path: Path,
) -> None:
    root, build = tmp_path / "cache", WalkingBuild(repo)
    _fetch(repo, root, build)
    shutil.rmtree(build.target)  # a fresh worktree at the same commit has no target yet
    hit = _fetch(repo, root, build)
    assert build.count == 1 and adapter_cache.EVENTS[-1].startswith("b hit ")
    assert hit["b"] == build.target / "release" / "b"
    assert build.target in hit["b"].parents and os.access(hit["b"], os.X_OK)
    assert _found_from(hit["b"]) == repo


def test_a_hit_in_another_worktree_runs_from_that_worktree(repo: Path, tmp_path: Path) -> None:
    root, build = tmp_path / "cache", WalkingBuild(repo)
    _fetch(repo, root, build)
    other = _clone(repo, tmp_path, "other")
    other_build = WalkingBuild(other)
    hit = _fetch(other, root, other_build)
    assert build.count == 1 and other_build.count == 0  # the same inputs: one build serves both
    assert hit["b"] == other_build.target / "release" / "b"
    assert _found_from(hit["b"]) == other


def test_an_executable_holding_its_worktree_path_is_reused_only_there(
    repo: Path, tmp_path: Path,
) -> None:
    root, build = tmp_path / "cache", WalkingBuild(repo, embed=True)
    _fetch(repo, root, build)
    assert '"b"' in (root / "b" / "index.json").read_text()  # learned as worktree-bound
    other = _clone(repo, tmp_path, "other")
    other_build = WalkingBuild(other, embed=True)
    _fetch(other, root, other_build)
    assert other_build.count == 1  # Cargo would rebuild it there, so the cache does too
    assert len(_entries(root)) == 2
    _fetch(repo, root, build)
    _fetch(other, root, other_build)
    assert build.count == 1 and other_build.count == 1


def test_the_worktree_path_scan_is_conservative(repo: Path, tmp_path: Path) -> None:
    holder, plain = tmp_path / "holder", tmp_path / "plain"
    holder.write_bytes(b"\0" * (1 << 20) + os.fsencode(str(repo.resolve())) + b"/rs/x\0")
    plain.write_bytes(b"\0" * (1 << 20) + b"/some/other/path\0")
    assert adapter_cache._embeds_worktree(holder, repo)
    assert not adapter_cache._embeds_worktree(plain, repo)
    # A path split across the scan's 1 MiB read blocks is still found.
    split = tmp_path / "split"
    needle = os.fsencode(str(repo.resolve()))
    split.write_bytes(b"\0" * ((1 << 20) - len(needle) // 2) + needle + b"\0")
    assert adapter_cache._embeds_worktree(split, repo)
    # What it cannot read counts as bound to its worktree, never as shareable.
    assert adapter_cache._embeds_worktree(tmp_path / "missing", repo)
    assert adapter_cache._embeds_worktree(tmp_path, repo)  # a directory: unreadable as a file


def test_a_hit_replaces_a_different_file_at_the_place(repo: Path, tmp_path: Path) -> None:
    root, build = tmp_path / "cache", FakeBuild(repo)
    built = _fetch(repo, root, build)
    [entry] = _entries(root)
    built["b"].write_text("#!/bin/sh\necho stale\n")
    hit = _fetch(repo, root, build)
    assert build.count == 1 and hit["b"] == built["b"]
    assert hit["b"].read_text() == (entry / "b").read_text()
    assert not [path for path in hit["b"].parent.iterdir() if path.name.startswith(".b.")]


def test_an_entry_without_a_place_is_rebuilt(repo: Path, tmp_path: Path) -> None:
    root, build = tmp_path / "cache", FakeBuild(repo)
    _fetch(repo, root, build)
    [entry] = _entries(root)
    manifest = json.loads((entry / "manifest.json").read_text())
    manifest["paths"] = {"b": "../../escape/b"}
    (entry / "manifest.json").write_text(json.dumps(manifest))
    built = _fetch(repo, root, build)
    assert build.count == 2 and built["b"] == build.target / "release" / "b"
    assert not (build.target.parent / "escape").exists()


def test_a_build_outside_its_target_is_not_published(repo: Path, tmp_path: Path) -> None:
    root, build = tmp_path / "cache", FakeBuild(repo)
    adapter_cache.fetch_or_build(_spec(), build, repo / "rs" / "target" / "elsewhere",
                                 repo=repo, root=root)
    assert build.count == 1 and not list(root.glob("b/*/manifest.json"))
    assert "not published" in adapter_cache.EVENTS[-1]


def test_the_build_learns_extra_inputs_and_declared_env(repo: Path, tmp_path: Path) -> None:
    root, build = tmp_path / "cache", FakeBuild(repo)
    _fetch(repo, root, build)
    index = (root / "b" / "index.json").read_text()
    assert '"py/extra.txt"' in index and '"MY_FLAG"' in index
    # Inside a crate, inside the target, or outside the repository: not an extra input.
    assert "main.rs" not in index and "generated.rs" not in index and "outside" not in index


@pytest.mark.parametrize("path", ["rs/a/src/lib.rs", "rs/b/src/main.rs", "rs/Cargo.lock"])
def test_a_crate_or_lock_change_misses(repo: Path, tmp_path: Path, path: str) -> None:
    root, build = tmp_path / "cache", FakeBuild(repo)
    _fetch(repo, root, build)
    (repo / path).write_text("changed\n")
    _commit(repo)
    _fetch(repo, root, build)
    assert build.count == 2
    assert len(_entries(root)) == 2


def test_an_extra_input_change_misses(repo: Path, tmp_path: Path) -> None:
    root, build = tmp_path / "cache", FakeBuild(repo)
    _fetch(repo, root, build)
    (repo / "py/extra.txt").write_text("a different page\n")
    _commit(repo)
    _fetch(repo, root, build)
    assert build.count == 2


def test_a_declared_env_change_misses(repo: Path, tmp_path: Path) -> None:
    root, build = tmp_path / "cache", FakeBuild(repo)
    _fetch(repo, root, build, _spec(MY_FLAG="1"))
    _fetch(repo, root, build, _spec(MY_FLAG="1"))
    assert build.count == 1
    _fetch(repo, root, build, _spec(MY_FLAG="2"))
    assert build.count == 2


def test_a_dev_only_dependency_or_unrelated_change_still_hits(repo: Path, tmp_path: Path) -> None:
    root, build = tmp_path / "cache", FakeBuild(repo)
    _fetch(repo, root, build)
    (repo / "rs/c/src/lib.rs").write_text("pub fn c2() {}\n")
    (repo / "README.md").write_text("docs\n")
    _commit(repo)
    _fetch(repo, root, build)
    assert build.count == 1


@pytest.mark.parametrize("edit", ["modified", "untracked", "extra"])
def test_a_dirty_tree_builds_and_publishes_nothing(
    repo: Path, tmp_path: Path, edit: str,
) -> None:
    root, build = tmp_path / "cache", FakeBuild(repo)
    _fetch(repo, root, build)  # learns py/extra.txt as an extra input
    [published] = _entries(root)
    if edit == "modified":
        (repo / "rs/a/src/lib.rs").write_text("pub fn dirty() {}\n")
    elif edit == "untracked":
        (repo / "rs/b/src/new.rs").write_text("// new\n")
    else:
        (repo / "py/extra.txt").write_text("uncommitted\n")
    _fetch(repo, root, build)
    _fetch(repo, root, build)
    assert build.count == 3
    assert _entries(root) == [published]
    assert "uncommitted inputs, no key" in adapter_cache.EVENTS[-1]


def test_an_incomplete_entry_is_never_used(repo: Path, tmp_path: Path) -> None:
    root, build = tmp_path / "cache", FakeBuild(repo)
    _fetch(repo, root, build)
    [entry] = _entries(root)
    (entry / "manifest.json").unlink()
    _fetch(repo, root, build)
    assert build.count == 2
    assert (entry / "manifest.json").is_file()  # replaced by a complete entry


def test_an_altered_executable_is_never_run(repo: Path, tmp_path: Path) -> None:
    root, build = tmp_path / "cache", FakeBuild(repo)
    _fetch(repo, root, build)
    [entry] = _entries(root)
    (entry / "b").chmod(0o755)
    (entry / "b").write_text("#!/bin/sh\necho tampered\n")
    ticket = Path(os.environ["GATE_ADMISSION_DIR"]) / "runs" / "1-ticket.json"
    ticket.write_text("{}")
    built = _fetch(repo, root, build)
    assert build.count == 2
    assert built["b"].parent == build.target / "release"
    hit = _fetch(repo, root, build)
    assert build.count == 2 and "tampered" not in hit["b"].read_text()
    # Moved aside, not deleted, while a ticket exists; pruned once none does.
    [aside] = [path for path in (root / "b").iterdir() if path.name.startswith(".corrupt-")]
    ticket.unlink()
    assert aside in adapter_cache.prune(root)


def test_two_publishers_of_one_key_leave_one_complete_entry(
    repo: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    root, first, second = tmp_path / "cache", FakeBuild(repo), FakeBuild(repo)
    second.target = repo / "rs" / "target" / "other"
    copy = shutil.copyfile
    raced: list[bool] = []

    def copy_after_a_rival(source: Path, destination: Path) -> Path:
        if not raced:  # the rival process publishes the same key while this one copies
            raced.append(True)
            _fetch(repo, root, second)
        return copy(source, destination)

    monkeypatch.setattr(shutil, "copyfile", copy_after_a_rival)
    _fetch(repo, root, first)
    assert raced and second.count == 1
    [entry] = _entries(root)
    assert not [path for path in (root / "b").iterdir() if path.name.startswith(".tmp-")]
    monkeypatch.setattr(shutil, "copyfile", copy)
    hit = _fetch(repo, root, first)
    assert first.count == 1 and hit["b"] == first.target / "release" / "b"
    assert "build 1" in (entry / "b").read_text()  # the rival's bytes, verified against its manifest
    assert hit["b"].read_text() == (entry / "b").read_text()


def _entry(root: Path, name: str, size: int, age_days: float, now: float) -> Path:
    entry = root / "b" / name
    entry.mkdir(parents=True)
    (entry / "b").write_bytes(b"x" * size)
    stamp = now - age_days * 86400
    os.utime(entry, (stamp, stamp))
    return entry


def test_prune_removes_nothing_while_a_ticket_exists(tmp_path: Path) -> None:
    root, now = tmp_path / "cache", time.time()
    old = _entry(root, "old", 10, 30, now)
    admission = Path(os.environ["GATE_ADMISSION_DIR"])
    for folder in ("queue", "runs", "deferred"):
        ticket = admission / folder / "1-ticket.json"
        ticket.write_text("{}")
        assert adapter_cache.prune(root, now=now) == []
        assert old.is_dir()
        ticket.unlink()
    assert adapter_cache.prune(root, now=now) == [old]


def test_prune_enforces_age_count_and_size(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    root, now = tmp_path / "cache", time.time()
    monkeypatch.setenv("AGENT_UTILS_ADAPTER_CACHE_MAX_AGE_DAYS", "7")
    monkeypatch.setenv("AGENT_UTILS_ADAPTER_CACHE_MAX_ENTRIES", "3")
    monkeypatch.setenv("AGENT_UTILS_ADAPTER_CACHE_MAX_BYTES", "250")
    stale = _entry(root, "stale", 10, 8, now)
    surplus = _entry(root, "surplus", 10, 4, now)
    oldest_kept = _entry(root, "kept-1", 100, 3, now)
    newer = [_entry(root, f"kept-{n}", 100, 3 - n * 0.5, now) for n in (2, 3)]
    abandoned = _entry(root, ".tmp-abandoned", 10, 1, now)
    in_progress = _entry(root, ".tmp-in-progress", 10, 0, now)
    removed = adapter_cache.prune(root, now=now)
    assert stale in removed and surplus in removed and abandoned in removed
    assert in_progress.is_dir()  # another publisher may still be copying into it
    # Over 250 bytes, the oldest of the three that fit the count goes too.
    assert oldest_kept in removed
    assert all(entry.is_dir() for entry in newer)


def test_at_the_size_cap_with_a_ticket_nothing_is_published(
    repo: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    root, build = tmp_path / "cache", FakeBuild(repo)
    monkeypatch.setenv("AGENT_UTILS_ADAPTER_CACHE_MAX_BYTES", "64")
    (Path(os.environ["GATE_ADMISSION_DIR"]) / "runs" / "1-ticket.json").write_text("{}")
    _fetch(repo, root, build)
    _fetch(repo, root, build)
    assert build.count == 2
    assert _entries(root) == []
    assert "not published" in adapter_cache.EVENTS[-1]


def test_below_the_free_space_floor_nothing_is_published(
    repo: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    root, build = tmp_path / "cache", FakeBuild(repo)
    monkeypatch.setenv("AGENT_UTILS_ADAPTER_CACHE_MIN_FREE_BYTES", str(2**62))
    _fetch(repo, root, build)
    assert _entries(root) == []


def test_crate_dirs_follow_workspace_normal_and_build_dependencies(repo: Path) -> None:
    assert adapter_cache.crate_dirs(_metadata(repo), repo, ["b"]) == ["rs/a", "rs/b"]
    with pytest.raises(ValueError, match="not workspace packages"):
        adapter_cache.crate_dirs(_metadata(repo), repo, ["serde"])


def test_dep_info_parsing_keeps_escaped_spaces(tmp_path: Path) -> None:
    info = tmp_path / "b.d"
    info.write_text("/t/b: /src/a\\ b.rs \\\n /src/c.rs\n\n/t/b.d: /src/ignored\n")
    assert adapter_cache._dep_info_paths(info) == [
        Path("/src/a b.rs"), Path("/src/c.rs"), Path("/src/ignored"),
    ]


def test_a_cache_failure_falls_back_to_building(
    repo: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    def broken(repo: Path) -> dict[str, object]:
        raise subprocess.CalledProcessError(101, ["cargo", "metadata"])

    monkeypatch.setattr(adapter_cache, "cargo_metadata", broken)
    build = FakeBuild(repo)
    assert _fetch(repo, tmp_path / "cache", build)["b"].is_file()
    assert build.count == 1 and "no key" in adapter_cache.EVENTS[-1]
