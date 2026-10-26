"""Read an upstream source that an operator compressed in place, as if it had not been.

A long-lived log mirror gets compressed. The observed case replaced each file ``X`` with ``X.zst``
in one pass -- the vendor mirrors *and* this tool's own content-addressed snapshot objects -- and
every importer then reported the file as missing, because every importer opens ``X``. Nothing about
the records had changed; only their encoding had.

This module covers upstream sources. The archive's own snapshot store is read as plain files and
must be restored (decompressed) if it was compressed; a `.zst` object there reads as missing.

This module is the one place that knows a compressed spelling exists. Callers keep asking for
``X``. :func:`resolve_source` answers with whichever of ``X`` and ``X.zst`` is present, preferring
the plain file when both are, because the plain one is what a later re-fetch writes and it can only
be the same bytes or a longer append. Readers then get the *decompressed* bytes, so the digests the
archive recorded against ``X`` keep verifying against ``X.zst``: compression is not a change of
content, and the append guards must not treat it as one.

Decompression uses the standard library's ``compression.zstd`` where the interpreter has it
(Python 3.14+), then the optional ``zstandard`` package, then the ``zstd`` command. The package
has no runtime dependencies, so the last is the path most hosts take; it is a subprocess per file,
which is cheap next to parsing the file. zstd frames written by the command carry a content
checksum by default, and the command verifies it while decoding, so a torn ``.zst`` fails loudly
rather than yielding a short transcript.

SQLite cannot read a compressed database, so :func:`materialize_source` decompresses one into a
caller-owned directory and returns that path. The caller owns cleanup; Orc ingestion already has a
staging directory for exactly this kind of scratch copy.
"""

from __future__ import annotations

import importlib
import io
import os
import shutil
import subprocess
import tempfile
from pathlib import Path
from types import ModuleType

#: The suffix an in-place compression appends. One codec, because one is what has been observed;
#: a second would be a second entry here and a second branch in :func:`_decompress`.
COMPRESSED_SUFFIX = ".zst"


class CompressedSourceError(OSError):
    """A compressed source exists but could not be decoded."""


def compressed_sibling(path: Path) -> Path:
    """The path an in-place compression of *path* would have produced."""

    return path.with_name(path.name + COMPRESSED_SUFFIX)


def logical_path(path: Path) -> Path:
    """*path* with a compression suffix removed: the name the archive records it under."""

    if path.name.endswith(COMPRESSED_SUFFIX) and len(path.name) > len(COMPRESSED_SUFFIX):
        return path.with_name(path.name[: -len(COMPRESSED_SUFFIX)])
    return path


def is_compressed(path: Path) -> bool:
    """Whether *path* names a compressed spelling rather than a plain file."""

    return path.name.endswith(COMPRESSED_SUFFIX)


def resolve_source(path: Path) -> Path | None:
    """The physical file holding *path*'s bytes, or ``None`` when neither spelling is present.

    A symlink is returned as-is for the caller's own symlink policy to judge; only a missing
    plain entry falls through to the compressed one, so a dangling symlink at ``X`` is not
    silently papered over by an unrelated ``X.zst``.
    """

    if os.path.lexists(path):
        return path
    sibling = compressed_sibling(path)
    if os.path.lexists(sibling):
        return sibling
    return None


def source_exists(path: Path) -> bool:
    """Whether *path* is present as a regular file in either spelling."""

    resolved = resolve_source(path)
    return resolved is not None and resolved.is_file() and not resolved.is_symlink()


def _stdlib_zstd() -> ModuleType | None:
    try:
        return importlib.import_module("compression.zstd")
    except ImportError:
        return None


def _zstandard() -> ModuleType | None:
    try:
        return importlib.import_module("zstandard")
    except ImportError:
        return None


def _decompress(data: bytes, where: Path) -> bytes:
    stdlib = _stdlib_zstd()
    if stdlib is not None:
        try:
            result: bytes = stdlib.decompress(data)
        except Exception as exc:  # the module's own ZstdError; not importable on older Pythons
            raise CompressedSourceError(f"cannot decompress {where}: {exc}") from exc
        return result
    package = _zstandard()
    if package is not None:
        try:
            reader = package.ZstdDecompressor().stream_reader(data)
            with reader:
                result = reader.read()
        except Exception as exc:  # zstandard.ZstdError
            raise CompressedSourceError(f"cannot decompress {where}: {exc}") from exc
        return result
    command = shutil.which("zstd")
    if command is None:
        raise CompressedSourceError(
            f"cannot read {where}: it is zstd-compressed and this interpreter has no zstd "
            "decoder (Python 3.14's compression.zstd, the zstandard package, or the zstd "
            "command)"
        )
    completed = subprocess.run(
        [command, "--decompress", "--stdout", "--quiet"],
        input=data,
        capture_output=True,
        check=False,
    )
    if completed.returncode != 0:
        detail = completed.stderr.decode("utf-8", "replace").strip()
        raise CompressedSourceError(
            f"cannot decompress {where}: zstd exited {completed.returncode}: {detail}"
        )
    return completed.stdout


def decode_bytes(physical: Path, data: bytes) -> bytes:
    """The logical bytes of a file whose physical bytes are *data*."""

    return _decompress(data, physical) if is_compressed(physical) else data


def read_source_bytes(path: Path) -> bytes:
    """The logical bytes of *path*, decompressing a ``.zst`` spelling when that is what exists.

    Raises :class:`FileNotFoundError` naming the plain path when neither spelling exists, so
    callers' existing missing-file handling still reads naturally.
    """

    physical = resolve_source(path)
    if physical is None:
        raise FileNotFoundError(f"no such file (plain or {COMPRESSED_SUFFIX}): {path}")
    return decode_bytes(physical, physical.read_bytes())


def read_first_line(path: Path) -> bytes:
    """The first line of *path*'s logical bytes, including its newline when it has one.

    A compressed file is decoded as a stream and decoding stops at the first newline, so
    discovery's metadata read costs about one line whichever spelling is present.
    """

    physical = resolve_source(path)
    if physical is None:
        raise FileNotFoundError(f"no such file (plain or {COMPRESSED_SUFFIX}): {path}")
    if not is_compressed(physical):
        with physical.open("rb") as handle:
            return handle.readline()
    stdlib = _stdlib_zstd()
    if stdlib is not None:
        try:
            with stdlib.open(physical, "rb") as handle:
                line: bytes = handle.readline()
        except Exception as exc:  # the module's own ZstdError
            raise CompressedSourceError(f"cannot decompress {physical}: {exc}") from exc
        return line
    package = _zstandard()
    if package is not None:
        try:
            with physical.open("rb") as raw, package.ZstdDecompressor().stream_reader(
                raw
            ) as reader:
                line = io.BufferedReader(reader).readline()
        except Exception as exc:  # zstandard.ZstdError
            raise CompressedSourceError(f"cannot decompress {physical}: {exc}") from exc
        return line
    command = shutil.which("zstd")
    if command is None:
        return decode_bytes(physical, physical.read_bytes())  # raises the no-decoder error
    # Stream, and stop at the first newline: discovery reads only a rollout's metadata line, and
    # decoding a few hundred megabytes to find it would make every scan cost the whole corpus.
    with subprocess.Popen(
        [command, "--decompress", "--stdout", "--quiet", str(physical)],
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    ) as process:
        assert process.stdout is not None
        line = process.stdout.readline()
        process.stdout.close()
        process.kill()
    return line


def materialize_source(path: Path, directory: Path) -> Path:
    """A plain-file path holding *path*'s logical bytes, for readers that need a real file.

    A plain source is returned unchanged. A compressed one is decoded into a new file inside
    *directory*, written atomically and owned by the caller, and that path is returned.
    """

    physical = resolve_source(path)
    if physical is None:
        raise FileNotFoundError(f"no such file (plain or {COMPRESSED_SUFFIX}): {path}")
    if not is_compressed(physical):
        return physical
    data = decode_bytes(physical, physical.read_bytes())
    directory.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(
        prefix=".decompressed-", suffix="-" + logical_path(physical).name, dir=directory
    )
    try:
        with os.fdopen(fd, "wb") as handle:
            handle.write(data)
            handle.flush()
            os.fsync(handle.fileno())
    except BaseException:
        Path(temporary).unlink(missing_ok=True)
        raise
    return Path(temporary)


def iter_logical_glob(root: Path, pattern: str) -> list[Path]:
    """Logical paths matching *pattern* below *root*, counting ``.zst`` spellings as their plain name.

    Each logical path appears once, sorted. When both spellings exist the plain one is the
    physical file :func:`resolve_source` will pick.
    """

    found: set[Path] = set(root.rglob(pattern))
    found.update(logical_path(path) for path in root.rglob(pattern + COMPRESSED_SUFFIX))
    return sorted(found)


__all__ = [
    "COMPRESSED_SUFFIX",
    "CompressedSourceError",
    "compressed_sibling",
    "decode_bytes",
    "is_compressed",
    "iter_logical_glob",
    "logical_path",
    "materialize_source",
    "read_first_line",
    "read_source_bytes",
    "resolve_source",
    "source_exists",
]
