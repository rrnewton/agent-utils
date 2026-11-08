"""The copied frozen-record parser imports cleanly from exactly its copied files.

wrkslots runs the consuming project's frozen-record parser from a private copy
of ``_FROZEN_VALIDATION_PARSER_FILES``. The fixture parser in test_lifecycle
only mirrors the import graph it was written against. When the producer's
reverie_node.py started importing reverie_e2e at top level (2026-11-02), every
frozen validation checkout became unremovable ("canonical frozen validation
record parser refused the handle: ... ModuleNotFoundError: No module named
'reverie_e2e'", 2026-11-08), and no agent-utils test noticed. The producer
fixed it by importing reverie_e2e lazily (2026-11-08), so the list is
unchanged. This test copies the listed files from the real producer's HEAD,
which is what production reads, and imports the parser entry point from that
copy alone. Any module the copied set imports at load time but the list omits
fails it, whichever side drifts.
"""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

import pytest

from wrkslots import cli

PRODUCER_ENV = "WRKSLOTS_FROZEN_PARSER_PRODUCER"


def _git(directory: Path, *args: str) -> subprocess.CompletedProcess[bytes]:
    return subprocess.run(
        ["git", "-C", str(directory), *args], capture_output=True, check=False
    )


def _tracks_parser(directory: Path) -> bool:
    entry = f"HEAD:{cli._FROZEN_VALIDATION_PARSER_ENTRY}"
    return _git(directory, "cat-file", "-e", entry).returncode == 0


def _producer() -> Path | None:
    """The consuming project that owns the parser modules, if one encloses us."""

    configured = os.environ.get(PRODUCER_ENV)
    if configured:
        return Path(configured)
    directory = Path(__file__).resolve().parent
    while True:
        result = _git(directory, "rev-parse", "--show-superproject-working-tree")
        superproject = result.stdout.decode().strip()
        if result.returncode != 0 or not superproject:
            return None
        directory = Path(superproject)
        if _tracks_parser(directory):
            return directory


def test_copied_parser_set_imports_without_the_producer_tree(tmp_path: Path) -> None:
    producer = _producer()
    if producer is None:
        pytest.skip(
            f"no enclosing project tracks {cli._FROZEN_VALIDATION_PARSER_ENTRY}; "
            f"set {PRODUCER_ENV} to its checkout"
        )
    assert _tracks_parser(producer), f"{producer} does not track the parser entry point"
    for relative in cli._FROZEN_VALIDATION_PARSER_FILES:
        blob = _git(producer, "cat-file", "blob", f"HEAD:{relative}")
        assert blob.returncode == 0, blob.stderr.decode()
        destination = tmp_path / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(blob.stdout)

    # The same interpreter, isolation flags and bootstrap as
    # _canonical_frozen_validation_record; --help imports every module the
    # entry point needs and then exits without reading a record.
    result = subprocess.run(
        [
            str(Path("/usr/bin/python3").resolve(strict=True)),
            "-I",
            "-B",
            "-c",
            cli._FROZEN_VALIDATION_PARSER_BOOTSTRAP,
            str(tmp_path / cli._FROZEN_VALIDATION_PARSER_ENTRY),
            "--help",
        ],
        capture_output=True,
        text=True,
        timeout=60,
        cwd=tmp_path,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    assert "usage:" in result.stdout
