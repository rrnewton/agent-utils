"""Where the browser walks get the page they serve, in one place.

`#624 voice-page-modularity`. The walks run real Chromium against a fake server of their own, which
serves the page's files out of a directory: `/` is that directory's `voice.html`, and every other
path is the file of that name below it. Each walk used to name `web/` itself, so changing what the
walks serve (the build the refactor introduces, whose shell names hashed, minified assets) meant
editing every one of them. Now they ask here.

`served_root()` is the directory a walk serves unless its `--web-root` names another:
`VIBE_TALK_WEB_ROOT` when that is set, else the checked-out `web/`. `SOURCE_ROOT` is always the
checked-out sources, for a walk that reads one of the page's own numbers out of them.
"""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path

SOURCE_ROOT = Path(__file__).resolve().parents[1] / "web"
"""The page's source files: `web/` in this checkout."""

ROOT_VARIABLE = "VIBE_TALK_WEB_ROOT"
"""The environment variable naming another directory for the walks to serve."""


def served_root() -> Path:
    """The directory the walks serve the page from: `VIBE_TALK_WEB_ROOT`, else `SOURCE_ROOT`."""
    named = os.environ.get(ROOT_VARIABLE, "")
    return Path(named).resolve() if named else SOURCE_ROOT


def source_text(root: Path, name: str) -> str:
    """The source of the page file `name` that the page served from `root` was made from.

    A walk that waits out one of the page's own numbers reads it here rather than opening
    `root / name` itself, so that the number keeps coming from source text however the served
    directory is produced. A directory of sources (web/, or another checkout's) is read as it is.
    A page build (`web/build.mjs`, which writes `assets.json` beside its minified files) was made
    from this checkout's sources, and its `inputs` say so by digest: `name` is read from
    `SOURCE_ROOT`, and a build those sources did not make is refused rather than read wrongly.
    """
    manifest_path = root / "assets.json"
    if not manifest_path.is_file():
        return (root / name).read_text()
    inputs = json.loads(manifest_path.read_text()).get("inputs", {})
    source = SOURCE_ROOT / name
    built_from = inputs.get(f"vibe-talk/web/{name}") if isinstance(inputs, dict) else None
    if built_from != hashlib.sha256(source.read_bytes()).hexdigest()[:32]:
        raise SystemExit(f"{root} was not built from {source}; rebuild it with web/build.mjs")
    return source.read_text()
