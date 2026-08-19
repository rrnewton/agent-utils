#!/usr/bin/env python3
"""Mechanically serialize publication to ``rrnewton/agent-utils:main``.

``AGENTS.md`` says routine changes go straight to ``main`` and that writers must
serialize: fetch immediately before the push, fast-forward only, re-fetch and
ancestry-verify afterwards. Until now that was prose, so nothing observed it.

This module is the mechanism. ``publish`` is the sanctioned path: it takes an
exclusive host-wide ``flock``, fetches ``origin/main``, records a **receipt**
naming the fetched value and the holding process, pushes, and then re-fetches
and proves the pushed commit is contained. ``hook-pre-push`` is the same
authority evaluated from the other side: a push to ``refs/heads/main`` is
refused unless a live receipt binds it to that serialized operation.

WHAT BINDS THE CHECK TO THE FACT (proxy binding). The hook does not trust the
token in the environment, which is only a copyable label. It requires all four
of: the token matches the on-disk receipt; the lock is *genuinely still held*
(a fresh non-blocking ``flock`` must FAIL); the recorded holder is a **live
ancestor** of the hook process, so the push is causally inside the serialized
operation rather than merely concurrent with it; and the remote's current value
equals the value the holder actually fetched (compare-and-swap), with the update
a fast-forward. A stale, copied, or borrowed receipt fails at least one of them.

COVERAGE, STATED RATHER THAN IMPLIED. This is a client-side control, and the
honest boundary is:

* COVERED -- ``publish``; and any bare ``git push`` to ``main`` from a checkout
  where ``install-hooks`` has run. The lock path is host-wide per uid, so every
  hooked checkout on one machine contends on one lock. That is the case that
  matters here, because the agent-utils workstreams share a box.
* NOT COVERED -- ``git push --no-verify``; a fresh clone that has not run
  ``install-hooks`` (``core.hooksPath`` lives in ``.git/config``, which is not
  cloned); another host; and the GitHub web UI or REST API.

Server-side closure is deliberately unavailable: ruleset "main history
protection" carries ``deletion``, ``non_fast_forward`` and
``required_linear_history`` with no ``pull_request`` and no
``required_status_checks`` rule, because requiring checks would block the
direct-to-main workflow this repository is built around. So history loss is
blocked on every path while serialization is enforced only on the paths above.
Do not describe this tool as closing the remaining ones.

``pr-exceptions`` checks the other half of the policy -- at most one open PR,
each recording why it is an exception. It is a check command with distinct exit
codes, never a precondition of ``publish``: an unrelated PR-hygiene lapse must
not block a legitimate publication.
"""

from __future__ import annotations

import argparse
import errno
import fcntl
import json
import math
import os
import re
import secrets
import shutil
import stat
import subprocess
import sys
import time
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path

PROG = "agent-utils-main-write"
REPOSITORY = "rrnewton/agent-utils"
MAIN_REF = "refs/heads/main"
ZERO_OID = "0" * 40
_SHA_RE = re.compile(r"^[0-9a-f]{40}$")

TOKEN_ENV = "AGENT_UTILS_MAIN_WRITE_TOKEN"
RECEIPT_ENV = "AGENT_UTILS_MAIN_WRITE_RECEIPT"
LOCK_ENV = "AGENT_UTILS_MAIN_WRITE_LOCK"
NO_PROXY_ENV = "AGENT_UTILS_MAIN_WRITE_NO_PROXY"

# An exception PR must say which of the two AGENTS.md reasons it is claiming.
# The slugs are matched exactly so "explained it somewhere in prose" does not
# read as a recorded reason.
EXCEPTION_TRAILER = "Exception-Reason:"
EXCEPTION_REASONS = frozenset({"high-risk-preland-review", "atomic-consumer-change"})

DEFAULT_NETWORK_TIMEOUT = 180.0
DEFAULT_GIT_TIMEOUT = 60.0


class WriteRefused(RuntimeError):
    """A serialization or ancestry precondition was not observably satisfied."""


class WriteUnverifiable(RuntimeError):
    """The tool could not look. NEVER report this as a satisfied condition."""


# --------------------------------------------------------------------------- #
# git plumbing                                                                 #
# --------------------------------------------------------------------------- #


def _run(
    command: Sequence[str], *, check: bool = True, timeout: float = DEFAULT_GIT_TIMEOUT
) -> subprocess.CompletedProcess[str]:
    try:
        return subprocess.run(
            list(command),
            check=check,
            capture_output=True,
            text=True,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired as error:
        raise WriteUnverifiable(
            f"command timed out after {timeout:.0f}s: {' '.join(command)}"
        ) from error
    except subprocess.CalledProcessError as error:
        detail = (error.stderr or error.stdout or "").strip()
        raise WriteUnverifiable(f"command failed: {' '.join(command)}: {detail}") from error
    except OSError as error:
        raise WriteUnverifiable(f"command failed: {' '.join(command)}: {error}") from error


def _git(root: Path, *args: str, check: bool = True) -> str:
    return _run(("git", "-C", str(root), *args), check=check).stdout.strip()


def repo_root(start: Path | None = None) -> Path:
    base = start if start is not None else Path.cwd()
    result = _run(("git", "-C", str(base), "rev-parse", "--show-toplevel"))
    return Path(result.stdout.strip()).resolve()


def _origin_url(root: Path) -> str:
    result = _run(("git", "-C", str(root), "remote", "get-url", "origin"), check=False)
    return result.stdout.strip()


def _network_git(root: Path, *args: str) -> str:
    """Run a networked git command, adding ``with-proxy`` only when it is needed.

    A local or ``file://`` origin (the test harness, and any mirror) must not be
    routed through the egress proxy, which would fail closed for no reason.
    """
    url = _origin_url(root)
    local = (not url) or url.startswith("/") or url.startswith("file://")
    direct = os.environ.get(NO_PROXY_ENV) == "1" or local
    prefix: tuple[str, ...] = ()
    if not direct and shutil.which("with-proxy") is not None:
        prefix = ("with-proxy",)
    return _run(
        (*prefix, "git", "-C", str(root), *args), timeout=DEFAULT_NETWORK_TIMEOUT
    ).stdout.strip()


def _rev_parse(root: Path, rev: str) -> str:
    sha = _git(root, "rev-parse", "--verify", f"{rev}^{{commit}}")
    if not _SHA_RE.match(sha):
        raise WriteUnverifiable(f"{rev!r} did not resolve to a commit SHA")
    return sha


def _is_ancestor(root: Path, older: str, newer: str) -> bool:
    result = _run(
        ("git", "-C", str(root), "merge-base", "--is-ancestor", older, newer), check=False
    )
    if result.returncode not in (0, 1):
        detail = (result.stderr or result.stdout or "").strip()
        raise WriteUnverifiable(f"cannot compare {older} with {newer}: {detail}")
    return result.returncode == 0


def fetch_main(root: Path) -> str:
    """Fetch ``origin/main`` and return its freshly observed value."""
    _network_git(root, "fetch", "origin", "refs/heads/main:refs/remotes/origin/main")
    return _rev_parse(root, "refs/remotes/origin/main")


# --------------------------------------------------------------------------- #
# receipt + lock                                                               #
# --------------------------------------------------------------------------- #


def lock_path() -> Path:
    """One lock per uid per host, so every hooked checkout contends on it."""
    override = os.environ.get(LOCK_ENV)
    if override:
        return Path(os.path.abspath(override))
    return Path("/tmp") / f"agent-utils-main-write-{os.getuid()}" / "writer.lock"


def receipt_path(lock: Path) -> Path:
    return lock.with_name(lock.name + ".receipt")


def _private_parent_fd(path: Path, *, create: bool) -> int | None:
    """Open the state directory without following its final component.

    The default lives directly below sticky ``/tmp``.  A hostile uid may win
    the name race, but it cannot make an object pass the owner and mode checks.
    Overrides receive the same validation and therefore must name a file in an
    already-private directory (or one whose immediate parent can be created).
    """
    parent = path.parent
    if create:
        try:
            os.mkdir(parent, 0o700)
        except FileExistsError:
            pass
        except OSError as error:
            raise WriteRefused(
                f"cannot create private writer-state directory {parent}: {error}"
            ) from error
    flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC
    try:
        fd = os.open(parent, flags)
    except FileNotFoundError:
        if not create:
            return None
        raise WriteRefused(f"writer-state directory {parent} disappeared") from None
    except OSError as error:
        raise WriteRefused(
            f"cannot safely open writer-state directory {parent}: {error}"
        ) from error
    metadata = os.fstat(fd)
    mode = stat.S_IMODE(metadata.st_mode)
    if (
        not stat.S_ISDIR(metadata.st_mode)
        or metadata.st_uid != os.geteuid()
        or mode != 0o700
    ):
        os.close(fd)
        raise WriteRefused(
            f"writer-state directory {parent} must be an owned mode-0700 directory"
        )
    return fd


def _open_private_file_at(
    parent_fd: int, name: str, *, create: bool, writable: bool
) -> int | None:
    """Open and validate one private regular state file relative to ``parent_fd``."""
    if not name or name in {".", ".."} or Path(name).name != name:
        raise WriteRefused(f"invalid writer-state filename {name!r}")
    flags = (os.O_RDWR if writable else os.O_RDONLY) | os.O_NOFOLLOW | os.O_CLOEXEC
    flags |= os.O_NONBLOCK
    if create:
        flags |= os.O_CREAT
    try:
        fd = os.open(name, flags, 0o600, dir_fd=parent_fd)
    except FileNotFoundError:
        if not create:
            return None
        raise WriteRefused(f"writer-state file {name} disappeared") from None
    except OSError as error:
        raise WriteRefused(f"cannot safely open writer-state file {name}: {error}") from error
    metadata = os.fstat(fd)
    mode = stat.S_IMODE(metadata.st_mode)
    if (
        not stat.S_ISREG(metadata.st_mode)
        or metadata.st_uid != os.geteuid()
        or metadata.st_nlink != 1
        or mode != 0o600
    ):
        os.close(fd)
        raise WriteRefused(
            f"writer-state file {name} must be an owned, single-link mode-0600 regular file"
        )
    return fd


def _read_private_receipt(path: Path) -> str:
    parent_fd = _private_parent_fd(path, create=False)
    if parent_fd is None:
        raise WriteRefused(f"writer receipt {path} is unreadable")
    fd = -1
    try:
        opened = _open_private_file_at(parent_fd, path.name, create=False, writable=False)
        if opened is None:
            raise WriteRefused(f"writer receipt {path} is unreadable")
        fd = opened
        chunks: list[bytes] = []
        remaining = 16_385
        while remaining:
            chunk = os.read(fd, min(remaining, 4096))
            if not chunk:
                break
            chunks.append(chunk)
            remaining -= len(chunk)
        payload = b"".join(chunks)
        if len(payload) > 16_384:
            raise WriteRefused(f"writer receipt {path} is unreasonably large")
        try:
            return payload.decode("utf-8")
        except UnicodeError as error:
            raise WriteRefused(f"writer receipt {path} is not UTF-8") from error
    finally:
        if fd >= 0:
            os.close(fd)
        os.close(parent_fd)


def _write_private_receipt(path: Path, payload: str) -> None:
    """Atomically replace a receipt without ever opening the old path."""
    parent_fd = _private_parent_fd(path, create=True)
    assert parent_fd is not None
    temp_name = f".{path.name}.{os.getpid()}.{secrets.token_hex(8)}.tmp"
    fd = -1
    installed = False
    try:
        flags = (
            os.O_WRONLY
            | os.O_CREAT
            | os.O_EXCL
            | os.O_NOFOLLOW
            | os.O_CLOEXEC
            | os.O_NONBLOCK
        )
        try:
            fd = os.open(temp_name, flags, 0o600, dir_fd=parent_fd)
        except OSError as error:
            raise WriteRefused(f"cannot create private writer receipt: {error}") from error
        metadata = os.fstat(fd)
        if (
            not stat.S_ISREG(metadata.st_mode)
            or metadata.st_uid != os.geteuid()
            or metadata.st_nlink != 1
            or stat.S_IMODE(metadata.st_mode) != 0o600
        ):
            raise WriteRefused("new writer receipt is not a private owned regular file")
        encoded = payload.encode("utf-8")
        view = memoryview(encoded)
        while view:
            written = os.write(fd, view)
            if written <= 0:  # pragma: no cover - os.write either progresses or raises
                raise WriteRefused("short write while creating writer receipt")
            view = view[written:]
        os.fsync(fd)
        os.close(fd)
        fd = -1
        os.replace(temp_name, path.name, src_dir_fd=parent_fd, dst_dir_fd=parent_fd)
        installed = True
        os.fsync(parent_fd)
    except OSError as error:
        raise WriteRefused(f"cannot install private writer receipt {path}: {error}") from error
    finally:
        if fd >= 0:
            os.close(fd)
        if not installed:
            try:
                os.unlink(temp_name, dir_fd=parent_fd)
            except FileNotFoundError:
                pass
        os.close(parent_fd)


@dataclass(frozen=True)
class Receipt:
    """What the holder actually observed, carried with the value it authorizes."""

    token: str
    pid: int
    expect: str
    started: float

    def to_json(self) -> str:
        return json.dumps(
            {
                "token": self.token,
                "pid": self.pid,
                "expect": self.expect,
                "started": self.started,
            },
            sort_keys=True,
        )

    @staticmethod
    def from_text(text: str) -> "Receipt":
        try:
            raw: object = json.loads(text)
        except json.JSONDecodeError as error:
            raise WriteRefused("writer receipt is not readable JSON") from error
        if not isinstance(raw, dict):
            raise WriteRefused("writer receipt is not an object")
        token = raw.get("token")
        pid = raw.get("pid")
        expect = raw.get("expect")
        started = raw.get("started")
        if not isinstance(token, str) or not token:
            raise WriteRefused("writer receipt has no token")
        if isinstance(pid, bool) or not isinstance(pid, int) or pid <= 0:
            raise WriteRefused("writer receipt has no holder pid")
        if not isinstance(expect, str) or not _SHA_RE.match(expect):
            raise WriteRefused("writer receipt has no fetched-origin SHA")
        if (
            isinstance(started, bool)
            or not isinstance(started, (int, float))
            or not math.isfinite(float(started))
        ):
            raise WriteRefused("writer receipt has no start time")
        return Receipt(token=token, pid=pid, expect=expect, started=float(started))


def _process_is_live(pid: int) -> bool:
    return Path(f"/proc/{pid}").is_dir()


def _parent_pid(pid: int) -> int | None:
    try:
        text = Path(f"/proc/{pid}/status").read_text()
    except OSError:
        return None
    for line in text.splitlines():
        if line.startswith("PPid:"):
            try:
                return int(line.split(":", 1)[1].strip())
            except ValueError:
                return None
    return None


def _is_live_ancestor(candidate: int, of_pid: int) -> bool:
    """True iff ``candidate`` is a live ancestor of ``of_pid``.

    This is the binding that a copied token cannot forge: a push authorized by
    this receipt must be running *inside* the serialized operation, not merely
    at the same time as one.
    """
    if candidate <= 0 or not _process_is_live(candidate):
        return False
    seen: set[int] = set()
    walker = _parent_pid(of_pid)
    while walker is not None and walker > 0 and walker not in seen:
        if walker == candidate:
            return True
        seen.add(walker)
        walker = _parent_pid(walker)
    return False


def lock_is_held(lock: Path) -> bool:
    """True iff some open file description currently holds the flock.

    Probing by trying to take it is the only honest answer: a recorded pid or a
    'locked' marker file would be a proxy for a lock rather than the lock.
    """
    parent_fd = _private_parent_fd(lock, create=False)
    if parent_fd is None:
        return False
    fd = -1
    try:
        opened = _open_private_file_at(parent_fd, lock.name, create=False, writable=True)
        if opened is None:
            return False
        fd = opened
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError as error:
            if error.errno not in (errno.EACCES, errno.EAGAIN):
                raise WriteRefused(f"cannot probe writer lock {lock}: {error}") from error
            return True
        fcntl.flock(fd, fcntl.LOCK_UN)
        return False
    finally:
        if fd >= 0:
            os.close(fd)
        os.close(parent_fd)


class WriterLock:
    """Exclusive, non-blocking host-wide writer lock.

    Contention fails immediately and names the holder. A bounded queue would
    hide the fact that authority was not acquired, which is the state a second
    writer most needs to see.
    """

    def __init__(self, lock: Path):
        self._path = lock
        self._handle: object = None
        self._fd = -1
        self._parent_fd = -1

    def __enter__(self) -> "WriterLock":
        parent_fd = _private_parent_fd(self._path, create=True)
        assert parent_fd is not None
        try:
            opened = _open_private_file_at(
                parent_fd, self._path.name, create=True, writable=True
            )
            assert opened is not None
            try:
                handle = os.fdopen(opened, "r+")
            except BaseException:
                os.close(opened)
                raise
        except BaseException:
            os.close(parent_fd)
            raise
        try:
            fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError as error:
            handle.close()
            os.close(parent_fd)
            raise WriteRefused(
                f"another agent-utils main writer owns {self._path}; "
                "wait for it to finish rather than pushing around it"
            ) from error
        self._handle = handle
        self._fd = handle.fileno()
        self._parent_fd = parent_fd
        return self

    def __exit__(self, *_exc: object) -> None:
        if self._fd >= 0:
            try:
                fcntl.flock(self._fd, fcntl.LOCK_UN)
            except OSError:
                pass
        handle = self._handle
        if handle is not None and hasattr(handle, "close"):
            close = getattr(handle, "close")
            if callable(close):
                close()
        self._handle = None
        self._fd = -1
        if self._parent_fd >= 0:
            os.close(self._parent_fd)
        self._parent_fd = -1


def verify_receipt(lock: Path, *, expected_remote: str, hook_pid: int) -> Receipt:
    """Dereference the writer authority. Every failure here is a refusal."""
    token = os.environ.get(TOKEN_ENV, "")
    recorded = os.environ.get(RECEIPT_ENV, "")
    if not token or not recorded:
        raise WriteRefused(
            "push to main carries no serialized-writer receipt; publish with "
            f"`python3 scripts/main_write.py publish` (never --no-verify)"
        )
    path = Path(recorded)
    expected_path = receipt_path(lock)
    if os.path.abspath(path) != os.path.abspath(expected_path):
        raise WriteRefused(f"writer receipt {path} does not name {receipt_path(lock)}")
    receipt = Receipt.from_text(_read_private_receipt(path))
    if not secrets.compare_digest(receipt.token, token):
        raise WriteRefused("writer receipt token does not match this operation")
    if not lock_is_held(lock):
        raise WriteRefused(
            "writer lock is not held; the receipt is stale, so this push is not serialized"
        )
    if not _is_live_ancestor(receipt.pid, hook_pid):
        raise WriteRefused(
            f"receipt holder pid {receipt.pid} is not a live ancestor of this push; "
            "a borrowed receipt does not authorize an unrelated writer"
        )
    if receipt.expect != expected_remote:
        raise WriteRefused(
            f"remote main moved after the fetch (remote={expected_remote} "
            f"fetched-origin={receipt.expect})"
        )
    return receipt


# --------------------------------------------------------------------------- #
# subcommands                                                                  #
# --------------------------------------------------------------------------- #


def cmd_publish(args: argparse.Namespace) -> int:
    root = repo_root(Path(args.root) if args.root else None)
    lock = lock_path()
    with WriterLock(lock) as _held:
        expect = fetch_main(root)
        rev = _rev_parse(root, str(args.rev))
        if rev == expect:
            print(f"{PROG}: already-published={rev} (nothing to publish)")
            return 0
        if not _is_ancestor(root, expect, rev):
            raise WriteRefused(
                f"{rev} is not based on freshly fetched origin/main {expect}; "
                "update the branch instead of forcing a non-fast-forward"
            )
        receipt = Receipt(
            token=secrets.token_hex(16), pid=os.getpid(), expect=expect, started=time.time()
        )
        target = receipt_path(lock)
        _write_private_receipt(target, receipt.to_json())
        os.environ[TOKEN_ENV] = receipt.token
        os.environ[RECEIPT_ENV] = str(target)
        try:
            _network_git(root, "push", "origin", f"{rev}:{MAIN_REF}")
            published = fetch_main(root)
        finally:
            os.environ.pop(TOKEN_ENV, None)
            os.environ.pop(RECEIPT_ENV, None)
        if not _is_ancestor(root, rev, published):
            raise WriteRefused(
                f"freshly fetched main {published} does not contain published {rev}"
            )
        print(
            f"AGENT_UTILS_MAIN_WRITE published={rev} fetched-main={published} "
            f"ancestry=1/1 base={expect}"
        )
    return 0


def cmd_hook_pre_push(args: argparse.Namespace) -> int:
    """Evaluate the writer authority for each ref git is about to push."""
    root = repo_root(Path(args.root) if args.root else None)
    lock = lock_path()
    hook_pid = os.getpid()
    checked = 0
    allowed_non_main = 0
    for line_number, line in enumerate(sys.stdin.read().splitlines(), start=1):
        fields = line.split()
        if len(fields) != 4:
            raise WriteRefused(
                f"malformed pre-push record on line {line_number}: expected 4 fields, "
                f"found {len(fields)}"
            )
        _local_ref, local_sha, remote_ref, remote_sha = fields
        if not _SHA_RE.match(local_sha) or not _SHA_RE.match(remote_sha):
            raise WriteRefused(
                f"malformed pre-push record on line {line_number}: "
                "object IDs must be 40 lowercase hexadecimal characters"
            )
        checked_ref = _run(
            ("git", "-C", str(root), "check-ref-format", remote_ref), check=False
        )
        if checked_ref.returncode != 0:
            raise WriteRefused(
                f"malformed pre-push record on line {line_number}: "
                f"invalid remote ref {remote_ref!r}"
            )
        if remote_ref != MAIN_REF:
            # Feature branches and PR heads are ordinary work. Refusing them
            # would make the guard worse than the gap it closes.
            allowed_non_main += 1
            continue
        checked += 1
        if local_sha == ZERO_OID:
            raise WriteRefused("refusing to delete main")
        if remote_sha == ZERO_OID:
            raise WriteRefused("refusing to create a missing remote main")
        verify_receipt(lock, expected_remote=remote_sha, hook_pid=hook_pid)
        if not _is_ancestor(root, remote_sha, local_sha):
            raise WriteRefused(
                f"main update {remote_sha}..{local_sha} is not a fast-forward"
            )
    print(
        f"{PROG}: pre-push ok main_refs={checked} other_refs={allowed_non_main}",
        file=sys.stderr,
    )
    return 0


def _open_pull_requests(repository: str) -> list[dict[str, object]]:
    if shutil.which("gh") is None:
        raise WriteUnverifiable("`gh` is not available, so PR exceptions cannot be read")
    prefix: tuple[str, ...] = ()
    if os.environ.get(NO_PROXY_ENV) != "1" and shutil.which("with-proxy") is not None:
        prefix = ("with-proxy",)
    result = _run(
        (
            *prefix,
            "gh",
            "pr",
            "list",
            "--repo",
            repository,
            "--state",
            "open",
            "--json",
            "number,title,body,isDraft",
        ),
        timeout=DEFAULT_NETWORK_TIMEOUT,
    )
    try:
        raw: object = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise WriteUnverifiable(f"gh returned malformed PR JSON: {error.msg}") from error
    if not isinstance(raw, list):
        raise WriteUnverifiable("gh returned a non-list PR payload")
    out: list[dict[str, object]] = []
    for index, item in enumerate(raw):
        if not isinstance(item, dict):
            raise WriteUnverifiable(
                f"gh returned a non-object PR payload item at index {index}"
            )
        out.append(item)
    return out


def recorded_exception_reason(body: str) -> str | None:
    """The exact reason slug this PR records, or ``None`` if it records none."""
    for line in body.splitlines():
        stripped = line.strip()
        if not stripped.startswith(EXCEPTION_TRAILER):
            continue
        value = stripped[len(EXCEPTION_TRAILER) :].strip()
        if value in EXCEPTION_REASONS:
            return value
    return None


def cmd_pr_exceptions(args: argparse.Namespace) -> int:
    pulls = _open_pull_requests(str(args.repository))
    findings: list[str] = []
    detail: list[dict[str, object]] = []
    for pull in pulls:
        number = pull.get("number")
        body = pull.get("body")
        reason = recorded_exception_reason(body if isinstance(body, str) else "")
        detail.append({"number": number, "reason": reason})
        if reason is None:
            findings.append(
                f"PR #{number} records no `{EXCEPTION_TRAILER} <reason>` line "
                f"(allowed: {', '.join(sorted(EXCEPTION_REASONS))})"
            )
    if len(pulls) > 1:
        findings.append(
            f"{len(pulls)} PRs are open; AGENTS.md allows at most one exceptional PR at a time"
        )
    if args.json:
        print(
            json.dumps(
                {
                    "repository": args.repository,
                    "open_pull_requests": len(pulls),
                    "pull_requests": detail,
                    "violations": findings,
                    "satisfied": not findings,
                },
                indent=2,
                sort_keys=True,
            )
        )
    else:
        for finding in findings:
            print(f"{PROG}: pr-exceptions: {finding}", file=sys.stderr)
        if not findings:
            print(f"{PROG}: pr-exceptions ok open={len(pulls)}")
    return 1 if findings else 0


def cmd_status(args: argparse.Namespace) -> int:
    root = repo_root(Path(args.root) if args.root else None)
    lock = lock_path()
    branch = _git(root, "rev-parse", "--abbrev-ref", "HEAD")
    dirty = [line for line in _git(root, "status", "--porcelain").splitlines() if line]
    if args.no_fetch:
        expect = _rev_parse(root, "refs/remotes/origin/main")
    else:
        expect = fetch_main(root)
    head = _rev_parse(root, "HEAD")
    unlanded = int(_git(root, "rev-list", "--count", f"{expect}..{head}"))
    behind = int(_git(root, "rev-list", "--count", f"{head}..{expect}"))
    held = lock_is_held(lock)
    clear = not dirty and unlanded == 0 and not held
    payload: dict[str, object] = {
        "repository": REPOSITORY,
        "root": str(root),
        "branch": branch,
        "head": head,
        "fetched_origin_main": expect,
        "dirty_paths": len(dirty),
        "unlanded_commits": unlanded,
        "behind_commits": behind,
        "writer_lock": str(lock),
        "writer_lock_held": held,
        "hooks_path": _git(root, "config", "--get", "core.hooksPath", check=False),
        "queue_clear": clear,
    }
    if args.json:
        print(json.dumps(payload, indent=2, sort_keys=True))
    else:
        for key, value in payload.items():
            print(f"{key}={value}")
    return 0 if clear else 1


_HOOK_BODY = """#!/bin/sh
# Installed by scripts/main_write.py install-hooks. Refuses an unserialized
# push to refs/heads/main; every other ref passes straight through.
exec python3 "$(git rev-parse --show-toplevel)/scripts/main_write.py" hook-pre-push "$@"
"""


def cmd_install_hooks(args: argparse.Namespace) -> int:
    root = repo_root(Path(args.root) if args.root else None)
    hooks = root / ".githooks"
    hook = hooks / "pre-push"
    if args.check:
        try:
            body = hook.read_text()
        except OSError as error:
            raise WriteRefused(f"cannot read tracked pre-push hook {hook}: {error}") from error
        if body != _HOOK_BODY:
            raise WriteRefused(
                "tracked .githooks/pre-push has drifted from install-hooks output"
            )
        print(f"{PROG}: tracked hook body matches install-hooks output")
        return 0

    hooks.mkdir(parents=True, exist_ok=True)
    hook.write_text(_HOOK_BODY)
    hook.chmod(0o755)
    _git(root, "config", "core.hooksPath", ".githooks")
    print(f"{PROG}: installed {hook} and set core.hooksPath=.githooks")
    print(
        f"{PROG}: NOT covered by this install -- `git push --no-verify`, a clone "
        "that has not run install-hooks, another host, and the GitHub web/REST path."
    )
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog=PROG, description="Serialize agent-utils publication to main."
    )
    parser.add_argument("--root", default=None, help="repository to operate on")
    sub = parser.add_subparsers(dest="command", required=True)

    publish = sub.add_parser("publish", help="serialized fast-forward publish to main", description="serialized fast-forward publish to main")
    publish.add_argument("rev", nargs="?", default="HEAD")
    publish.set_defaults(handler=cmd_publish)

    hook = sub.add_parser("hook-pre-push", help="pre-push hook entry point", description="pre-push hook entry point")
    hook.add_argument("hook_args", nargs="*")
    hook.set_defaults(handler=cmd_hook_pre_push)

    status = sub.add_parser("status", help="report the serialize-queue predicate", description="report the serialize-queue predicate")
    status.add_argument("--json", action="store_true")
    status.add_argument("--no-fetch", action="store_true")
    status.set_defaults(handler=cmd_status)

    exceptions = sub.add_parser("pr-exceptions", help="check the PR-exception invariant", description="check the PR-exception invariant")
    exceptions.add_argument("--repository", default=REPOSITORY)
    exceptions.add_argument("--json", action="store_true")
    exceptions.set_defaults(handler=cmd_pr_exceptions)

    install = sub.add_parser("install-hooks", help="install the pre-push guard", description="install the pre-push guard")
    install.add_argument(
        "--check",
        action="store_true",
        help="verify the tracked hook body without changing the checkout",
    )
    install.set_defaults(handler=cmd_install_hooks)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(list(argv) if argv is not None else None)
    handler = getattr(args, "handler")
    if not callable(handler):  # pragma: no cover - argparse guarantees this
        parser.error("no handler bound")
    try:
        result = handler(args)
    except WriteRefused as error:
        print(f"{PROG}: REFUSED: {error}", file=sys.stderr)
        return 1
    except WriteUnverifiable as error:
        print(f"{PROG}: UNVERIFIABLE: {error}", file=sys.stderr)
        return 2
    return int(result) if isinstance(result, int) else 0


if __name__ == "__main__":
    raise SystemExit(main())
