"""Deterministic precompression for browser-facing timeline files."""

from __future__ import annotations

import gzip
import io
import os
import shutil
import tempfile
from pathlib import Path

GZIP_COMPRESSION_LEVEL = 6
GZIP_MINIMUM_BYTES = 1024


def gzip_sidecar_path(path: Path) -> Path:
    """Return the conventional precompressed sidecar path for *path*."""

    return path.with_name(path.name + ".gz")


def deterministic_gzip(data: bytes) -> bytes:
    """Return a reproducible gzip-6 stream with no filename or wall-clock timestamp."""

    output = io.BytesIO()
    with gzip.GzipFile(
        filename="",
        mode="wb",
        compresslevel=GZIP_COMPRESSION_LEVEL,
        fileobj=output,
        mtime=0,
    ) as compressed:
        compressed.write(data)
    return output.getvalue()


def _files_equal(left: Path, right: Path) -> bool:
    if not left.is_file() or left.stat().st_size != right.stat().st_size:
        return False
    with left.open("rb") as left_handle, right.open("rb") as right_handle:
        while True:
            left_chunk = left_handle.read(1024 * 1024)
            right_chunk = right_handle.read(1024 * 1024)
            if left_chunk != right_chunk:
                return False
            if not left_chunk:
                return True


def _write_gzip_if_changed(source: Path, sidecar: Path) -> bool:
    sidecar.parent.mkdir(parents=True, exist_ok=True)
    fd, raw_tmp = tempfile.mkstemp(prefix=f".{sidecar.name}.", dir=sidecar.parent)
    tmp = Path(raw_tmp)
    try:
        with os.fdopen(fd, "wb") as output:
            with gzip.GzipFile(
                filename="",
                mode="wb",
                compresslevel=GZIP_COMPRESSION_LEVEL,
                fileobj=output,
                mtime=0,
            ) as compressed:
                with source.open("rb") as source_handle:
                    shutil.copyfileobj(source_handle, compressed, length=1024 * 1024)
            output.flush()
            os.fsync(output.fileno())
        if _files_equal(sidecar, tmp):
            return False
        os.replace(tmp, sidecar)
        return True
    finally:
        if tmp.exists():
            tmp.unlink()


def sync_gzip_sidecar(path: Path, *, minimum_bytes: int = GZIP_MINIMUM_BYTES) -> bool:
    """Create, refresh, or remove the deterministic ``.gz`` companion for *path*.

    Tiny files remain identity-only because their header and inode overhead outweighs the transfer
    saving. A missing source removes an old sidecar so sparse-summary rebuilds cannot expose stale
    content.
    """

    if minimum_bytes < 0:
        raise ValueError("minimum_bytes must be non-negative")
    sidecar = gzip_sidecar_path(path)
    if sidecar.is_symlink():
        raise ValueError(f"refusing unsafe gzip sidecar: {sidecar}")
    if not path.is_file() or path.stat().st_size < minimum_bytes:
        if not sidecar.exists():
            return False
        if not sidecar.is_file():
            raise ValueError(f"refusing unsafe gzip sidecar: {sidecar}")
        sidecar.unlink()
        return True
    if path.is_symlink():
        raise ValueError(f"refusing to compress symlinked static asset: {path}")
    return _write_gzip_if_changed(path, sidecar)


__all__ = [
    "GZIP_COMPRESSION_LEVEL",
    "GZIP_MINIMUM_BYTES",
    "deterministic_gzip",
    "gzip_sidecar_path",
    "sync_gzip_sidecar",
]
