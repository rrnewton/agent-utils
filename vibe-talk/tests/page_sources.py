"""Where the browser walks get the page they serve, in one place.

`#624 voice-page-modularity`. The walks run real Chromium against a fake server of their own, which
serves the page's files out of a directory: `/` is that directory's `voice.html`, and every other
path is the file of that name below it. They serve what the server serves: the page build
(`web/build.mjs`), whose shell names one minified script and one stylesheet under content-hashed
names, so a minification or bundling fault fails a walk the way it would fail a phone.

`served_root()` is the directory a walk serves unless its `--web-root` names another:
`VIBE_TALK_WEB_ROOT` when that is set (a page build, or a directory of sources served as they are),
else a page build of this checkout made for this process. `variant()` is the page with something
changed on purpose, for a negative control, built the same way. `SOURCE_ROOT` is always the
checked-out sources, and `source_text()` reads one of the page's own numbers from the sources a
served directory was made from.
"""

from __future__ import annotations

import atexit
import functools
import hashlib
import json
import os
import shutil
import subprocess
import tempfile
from collections.abc import Sequence
from pathlib import Path

SOURCE_ROOT = Path(__file__).resolve().parents[1] / "web"
"""The page's source files: `web/` in this checkout."""

ROOT_VARIABLE = "VIBE_TALK_WEB_ROOT"
"""The environment variable naming another directory for the walks to serve."""

BUILD = SOURCE_ROOT / "build.mjs"
"""The page build: `node web/build.mjs --out DIR [--web SOURCES]`."""


def build_page(into: Path, sources: Path = SOURCE_ROOT) -> Path:
    """Build the page whose sources are `sources` into `into`, as the server's build does."""
    result = subprocess.run(
        ["node", str(BUILD), "--out", str(into), "--web", str(sources)],
        capture_output=True, text=True, check=False,
    )
    if result.returncode != 0:
        raise SystemExit(f"the page build of {sources} failed (it needs `npm ci --prefix vibe-talk/contract`):\n"
                         f"{result.stdout}{result.stderr}")
    return into


@functools.cache
def served_root() -> Path:
    """The directory the walks serve the page from: `VIBE_TALK_WEB_ROOT`, else a page build of this
    checkout, made once per process in a directory of its own that goes when the process does."""
    named = os.environ.get(ROOT_VARIABLE, "")
    if named:
        return Path(named).resolve()
    scratch = Path(tempfile.mkdtemp(prefix="vibe-talk-walk-page-"))
    atexit.register(shutil.rmtree, scratch, True)
    return build_page(scratch / "web").resolve()


def _built_from(root: Path) -> Path:
    """The sources the directory `root` serves were made from: `root` itself when it holds sources,
    or, for a page build, this checkout's sources, refused unless they are the ones it was built from
    (by the digests its `assets.json` records)."""
    manifest_path = root / "assets.json"
    if not manifest_path.is_file():
        return root
    recorded = json.loads(manifest_path.read_text()).get("inputs", {})
    inputs = recorded if isinstance(recorded, dict) else {}
    here = {
        "vibe-talk/web/" + source.relative_to(SOURCE_ROOT).as_posix(): hashlib.sha256(source.read_bytes()).hexdigest()[:32]
        for source in SOURCE_ROOT.rglob("*") if source.is_file()
    }
    theirs = {name: digest for name, digest in inputs.items() if name.startswith("vibe-talk/web/")}
    if here != theirs:
        differ = sorted(name for name in here.keys() | theirs.keys() if here.get(name) != theirs.get(name))
        raise SystemExit(f"{root} was not built from {SOURCE_ROOT} (differs in {differ}); "
                         "rebuild it with web/build.mjs")
    return SOURCE_ROOT


def source_text(root: Path, name: str) -> str:
    """The source of the page file `name` that the page served from `root` was made from.

    A walk that waits out one of the page's own numbers reads it here rather than opening
    `root / name` itself, so that the number comes from source text, not a minified copy.
    """
    return (_built_from(root) / name).read_text()


def variant(root: Path, into: Path, patches: Sequence[tuple[str, str, str]]) -> Path:
    """The page `root` serves with `patches` applied to its sources, served from below `into`.

    Each patch is (file, text, replacement), and `text` must occur in that file exactly once, so a
    control that no longer matches the page fails here instead of quietly testing the page unchanged.
    A page build is rebuilt from patched copies of the sources it was made from, exactly as the page
    is built; a directory of sources is patched and served as it is.
    """
    sources = _built_from(root)
    built = sources != root
    copy = into / ("sources" if built else "web")
    shutil.copytree(sources, copy)
    for file, text, replacement in patches:
        body = (copy / file).read_text()
        found = body.count(text)
        if found != 1:
            raise SystemExit(f"the control patches {text!r}, which {sources / file} holds {found} times, not once")
        (copy / file).write_text(body.replace(text, replacement, 1))
    return build_page(into / "web", copy).resolve() if built else copy.resolve()
