"""Kernel evidence about Unix sockets bound to filesystem nodes (#1230 socket-liveness-proof).

A Unix socket node in a directory tree is only a name: whether a live socket is
still bound to it is a fact about the kernel's socket table, which the node's
lstat identity cannot answer (a socket's sockfs inode is another identity
domain).  NETLINK_SOCK_DIAG with UNIX_DIAG_VFS answers it: the kernel lists, per
network namespace, every Unix socket together with the (device, inode) of the
filesystem node it is bound to.

This module is the evidence layer only, and stdlib-only, so it also runs as the
small child that enters another network namespace (``python3 -I sockdiag.py
--netns-fd N``).  It refuses to return a dump it cannot fully authenticate:
the reply must come from the kernel, carry our sequence number and port, end in
NLMSG_DONE, and never report an error, an interrupted dump or truncation.

Two facts about UNIX_DIAG_VFS shape every comparison made with it:

* ``vfs_ino`` is 32 bits wide.  A socket bound to a node reports the low 32 bits
  of the node's inode number, so a mismatch in the low 32 bits proves no socket
  is bound to the node, while a match is only a candidate.
* ``vfs_dev`` is the superblock's kernel dev_t, which differs from st_dev in
  encoding and, on btrfs, in value (stat reports the subvolume's device).  It
  is only ever compared through a calibration socket bound on the same st_dev.

A dump is also not a snapshot.  The kernel resumes each reply chunk at a
position within a hash bucket, so a socket removed meanwhile ahead of that
position makes the next chunk skip a live socket, and unix_diag never sets
NLM_F_DUMP_INTR to say so.  A dump can therefore prove a node LIVE, but never
that no socket is bound to it; the caller proves that with connect() probes.
"""

from __future__ import annotations

import base64
import ctypes
import dataclasses
import errno
import fcntl
import json
import os
import secrets
import socket
import struct
import sys
import time
from collections.abc import Callable, Sequence

NETLINK_SOCK_DIAG = 4
SOCK_DIAG_BY_FAMILY = 20
NLMSG_ERROR = 2
NLMSG_DONE = 3
NLM_F_REQUEST = 0x1
NLM_F_DUMP = 0x300
NLM_F_DUMP_INTR = 0x10
UDIAG_SHOW_NAME = 0x01
UDIAG_SHOW_VFS = 0x02
UDIAG_SHOW_PEER = 0x04
UDIAG_SHOW_UID = 0x40
UNIX_DIAG_NAME = 0
UNIX_DIAG_VFS = 1
UNIX_DIAG_UID = 7
NS_GET_USERNS = 0xB701
CLONE_NEWNET = 0x40000000
CLONE_NEWUSER = 0x10000000

_NLMSGHDR = struct.Struct("=IHHII")
_UNIX_DIAG_REQ = struct.Struct("=BBHIII2I")
_UNIX_DIAG_MSG = struct.Struct("=BBBBI2I")
_NLATTR = struct.Struct("=HH")
_VFS = struct.Struct("=II")

DUMP_RECORD_LIMIT = 4_000_000
"""Records beyond this many refuse: a host listing more is not judged by this proof."""
DUMP_BYTES_LIMIT = 512 * 1024 * 1024
RECV_BUFFER_BYTES = 1 << 20
INODE_MASK = 0xFFFFFFFF


class DumpError(Exception):
    """A socket-table dump that cannot be fully authenticated; the caller refuses."""


@dataclasses.dataclass(frozen=True)
class UnixSocket:
    """One Unix socket as UNIX_DIAG reports it."""

    sockfs_ino: int
    cookie: int
    type: int
    state: int
    vfs: tuple[int, int] | None
    """(low 32 bits of the bound node's inode, kernel dev_t of its superblock)."""
    name: bytes | None
    uid: int | None


@dataclasses.dataclass(frozen=True)
class Dump:
    """Every Unix socket of one network namespace, from one authenticated dump."""

    netns: int
    sockets: tuple[UnixSocket, ...]
    records: int
    bytes: int
    seconds: float


class Transport:
    """The netlink exchange, replaceable by tests with a scripted peer."""

    def __init__(self, timeout: float) -> None:
        self.sock = socket.socket(
            socket.AF_NETLINK, socket.SOCK_RAW | socket.SOCK_CLOEXEC, NETLINK_SOCK_DIAG
        )
        self.sock.bind((0, 0))
        self.sock.settimeout(max(0.001, timeout))

    def port(self) -> int:
        """The netlink port the kernel assigned to this socket."""
        return int(self.sock.getsockname()[0])

    def send(self, data: bytes) -> None:
        """Send one request to the kernel."""
        self.sock.sendto(data, (0, 0))

    def recv(self) -> tuple[bytes, int, tuple[int, int]]:
        """One datagram: its bytes, the recvmsg flags, and the sender's (port, groups)."""
        data, _ancillary, flags, address = self.sock.recvmsg(
            RECV_BUFFER_BYTES, 0, socket.MSG_TRUNC
        )
        return data, flags, (int(address[0]), int(address[1]))

    def close(self) -> None:
        """Close the netlink socket."""
        self.sock.close()


def setns(fd: int, nstype: int) -> None:
    """setns(2) through libc: os.setns needs Python 3.12, and this runs on 3.10 too."""

    libc = ctypes.CDLL(None, use_errno=True)
    if libc.setns(ctypes.c_int(fd), ctypes.c_int(nstype)) != 0:
        number = ctypes.get_errno()
        raise OSError(number, os.strerror(number))


def own_netns() -> int:
    """The nsfs inode of the calling thread's network namespace."""

    return os.stat("/proc/thread-self/ns/net").st_ino


def unix_request(seq: int) -> bytes:
    """A SOCK_DIAG_BY_FAMILY dump request for every Unix socket, with names and VFS identities."""

    payload = _UNIX_DIAG_REQ.pack(
        socket.AF_UNIX,
        0,
        0,
        0xFFFFFFFF,
        0,
        UDIAG_SHOW_NAME | UDIAG_SHOW_VFS | UDIAG_SHOW_PEER | UDIAG_SHOW_UID,
        0xFFFFFFFF,
        0xFFFFFFFF,
    )
    return _NLMSGHDR.pack(
        _NLMSGHDR.size + len(payload), SOCK_DIAG_BY_FAMILY, NLM_F_REQUEST | NLM_F_DUMP, seq, 0
    ) + payload


def _parse_record(data: bytes, start: int, end: int) -> UnixSocket:
    if end - start < _UNIX_DIAG_MSG.size:
        raise DumpError("unix_diag record shorter than its header")
    family, kind, state, _pad, ino, cookie0, cookie1 = _UNIX_DIAG_MSG.unpack_from(data, start)
    if family != socket.AF_UNIX:
        raise DumpError(f"unix_diag record of family {family}")
    vfs: tuple[int, int] | None = None
    name: bytes | None = None
    uid: int | None = None
    offset = start + _UNIX_DIAG_MSG.size
    while offset < end:
        if end - offset < _NLATTR.size:
            raise DumpError("truncated unix_diag attribute header")
        length, attribute = _NLATTR.unpack_from(data, offset)
        if length < _NLATTR.size or offset + length > end:
            raise DumpError("unix_diag attribute overruns its record")
        body = data[offset + _NLATTR.size : offset + length]
        attribute &= 0x3FFF
        if attribute == UNIX_DIAG_VFS:
            if len(body) != _VFS.size or vfs is not None:
                raise DumpError("malformed UNIX_DIAG_VFS attribute")
            vfs_ino, vfs_dev = _VFS.unpack(body)
            vfs = (vfs_ino, vfs_dev)
        elif attribute == UNIX_DIAG_NAME:
            if name is not None:
                raise DumpError("repeated UNIX_DIAG_NAME attribute")
            name = bytes(body)
        elif attribute == UNIX_DIAG_UID:
            if len(body) != 4:
                raise DumpError("malformed UNIX_DIAG_UID attribute")
            uid = struct.unpack("=I", body)[0]
        offset += (length + 3) & ~3
    return UnixSocket(ino, cookie0 | (cookie1 << 32), kind, state, vfs, name, uid)


def parse_dump(
    chunks: Sequence[tuple[bytes, int, tuple[int, int]]], *, seq: int, port: int
) -> tuple[list[UnixSocket], int]:
    """Parse a complete multipart reply; refuse anything short of a clean NLMSG_DONE."""

    sockets: list[UnixSocket] = []
    total = 0
    done = False
    for data, flags, sender in chunks:
        if done:
            raise DumpError("data after NLMSG_DONE")
        if sender != (0, 0):
            raise DumpError(f"netlink reply from {sender}, not the kernel")
        if flags & socket.MSG_TRUNC:
            raise DumpError("netlink reply truncated (MSG_TRUNC)")
        total += len(data)
        if total > DUMP_BYTES_LIMIT:
            raise DumpError("socket-table dump exceeds its byte limit")
        offset = 0
        while offset < len(data):
            if len(data) - offset < _NLMSGHDR.size:
                raise DumpError("truncated netlink message header")
            length, kind, msg_flags, msg_seq, msg_port = _NLMSGHDR.unpack_from(data, offset)
            if length < _NLMSGHDR.size or offset + length > len(data):
                raise DumpError("netlink message overruns its datagram")
            if msg_seq != seq:
                raise DumpError(f"netlink reply sequence {msg_seq}, not {seq}")
            if msg_port != port:
                raise DumpError(f"netlink reply for port {msg_port}, not {port}")
            if msg_flags & NLM_F_DUMP_INTR:
                raise DumpError("socket-table dump was interrupted (NLM_F_DUMP_INTR)")
            body_start = offset + _NLMSGHDR.size
            if kind == NLMSG_DONE:
                if length >= _NLMSGHDR.size + 4:
                    status = struct.unpack_from("=i", data, body_start)[0]
                    if status != 0:
                        raise DumpError(f"socket-table dump ended with error {-status}")
                done = True
                if offset + ((length + 3) & ~3) < len(data):
                    raise DumpError("data after NLMSG_DONE")
                break
            if kind == NLMSG_ERROR:
                status = (
                    struct.unpack_from("=i", data, body_start)[0]
                    if length >= _NLMSGHDR.size + 4
                    else 0
                )
                raise DumpError(f"socket-table dump refused with error {-status}")
            if kind != SOCK_DIAG_BY_FAMILY:
                raise DumpError(f"unexpected netlink message type {kind}")
            sockets.append(_parse_record(data, body_start, offset + length))
            if len(sockets) > DUMP_RECORD_LIMIT:
                raise DumpError("socket-table dump exceeds its record limit")
            offset += (length + 3) & ~3
    if not done:
        raise DumpError("socket-table dump ended without NLMSG_DONE")
    return sockets, total


def raw_dump(
    *, deadline: float, transport_factory: Callable[[float], Transport] | None = None
) -> tuple[list[tuple[bytes, int, tuple[int, int]]], int, int]:
    """Send one unix_diag dump request and collect the reply up to NLMSG_DONE."""

    factory = transport_factory or Transport
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        raise DumpError("socket-table dump has no time left")
    transport = factory(remaining)
    try:
        port = transport.port()
        seq = secrets.randbelow(0x7FFFFFFF) + 1
        transport.send(unix_request(seq))
        chunks: list[tuple[bytes, int, tuple[int, int]]] = []
        while True:
            if time.monotonic() > deadline:
                raise DumpError("socket-table dump exceeded its time limit")
            try:
                chunk = transport.recv()
            except TimeoutError as exc:
                raise DumpError("socket-table dump timed out before NLMSG_DONE") from exc
            except OSError as exc:
                raise DumpError(f"socket-table dump failed: {exc}") from exc
            chunks.append(chunk)
            if _ends_dump(chunk[0]):
                return chunks, seq, port
    finally:
        transport.close()


def _ends_dump(data: bytes) -> bool:
    """Whether a datagram holds NLMSG_DONE or NLMSG_ERROR (parse_dump judges it)."""

    offset = 0
    while len(data) - offset >= _NLMSGHDR.size:
        length, kind = _NLMSGHDR.unpack_from(data, offset)[:2]
        if kind in (NLMSG_DONE, NLMSG_ERROR):
            return True
        if length < _NLMSGHDR.size:
            return True  # malformed: let parse_dump refuse it
        offset += (length + 3) & ~3
    return False


def dump_own_namespace(
    *, deadline: float, transport_factory: Callable[[float], Transport] | None = None
) -> Dump:
    """Dump the calling thread's network namespace, refusing if it changed meanwhile."""

    started = time.monotonic()
    before = own_netns()
    chunks, seq, port = raw_dump(deadline=deadline, transport_factory=transport_factory)
    if own_netns() != before:
        raise DumpError("network namespace changed during the socket-table dump")
    sockets, total = parse_dump(chunks, seq=seq, port=port)
    return Dump(before, tuple(sockets), len(sockets), total, time.monotonic() - started)


def enter_and_dump(netns_fd: int, *, deadline: float) -> dict[str, object]:
    """In a fresh child: join the namespace's owning user namespace, then it, and dump.

    Joining a network namespace needs CAP_SYS_ADMIN both in its owning user
    namespace and in the caller's: an unprivileged caller has both only by first
    joining a user namespace it owns.  Any failure is reported, never retried.
    """

    target = os.fstat(netns_fd).st_ino
    if target != own_netns():
        try:
            owner = fcntl.ioctl(netns_fd, NS_GET_USERNS)
        except OSError as exc:
            return {"error": f"cannot find the namespace's owning user namespace: {exc}"}
        try:
            if os.fstat(owner).st_ino != os.stat("/proc/self/ns/user").st_ino:
                setns(owner, CLONE_NEWUSER)
            setns(netns_fd, CLONE_NEWNET)
        except OSError as exc:
            return {
                "error": f"cannot enter network namespace {target}: {exc}",
                "errno": exc.errno,
            }
        finally:
            os.close(owner)
    if own_netns() != target:
        return {"error": f"entered network namespace {own_netns()}, not {target}"}
    started = time.monotonic()
    chunks, seq, port = raw_dump(deadline=deadline)
    if own_netns() != target:
        return {"error": "network namespace changed during the socket-table dump"}
    return {
        "netns": target,
        "seq": seq,
        "port": port,
        "seconds": time.monotonic() - started,
        "chunks": [
            [base64.b64encode(data).decode("ascii"), flags, list(sender)]
            for data, flags, sender in chunks
        ],
    }


def dump_from_wire(wire: dict[str, object], *, expected_netns: int) -> Dump:
    """Parse and authenticate a child's raw dump of ``expected_netns``."""

    if "error" in wire:
        raise DumpError(str(wire["error"]))
    if wire.get("netns") != expected_netns:
        raise DumpError(f"child dumped namespace {wire.get('netns')}, not {expected_netns}")
    seq, port = wire.get("seq"), wire.get("port")
    raw_chunks = wire.get("chunks")
    if not isinstance(seq, int) or not isinstance(port, int) or not isinstance(raw_chunks, list):
        raise DumpError("child returned a malformed dump")
    chunks: list[tuple[bytes, int, tuple[int, int]]] = []
    for item in raw_chunks:
        if not (isinstance(item, list) and len(item) == 3 and isinstance(item[0], str)
                and isinstance(item[1], int) and isinstance(item[2], list) and len(item[2]) == 2):
            raise DumpError("child returned a malformed dump chunk")
        chunks.append((base64.b64decode(item[0], validate=True), item[1],
                       (int(item[2][0]), int(item[2][1]))))
    sockets, total = parse_dump(chunks, seq=seq, port=port)
    seconds = wire.get("seconds")
    return Dump(expected_netns, tuple(sockets), len(sockets), total,
                float(seconds) if isinstance(seconds, (int, float)) else 0.0)


def low32(inode: int) -> int:
    """The part of an inode number UNIX_DIAG_VFS carries."""

    return inode & INODE_MASK


def _child_main(argv: Sequence[str]) -> int:
    if len(argv) != 4 or argv[0] != "--netns-fd" or argv[2] != "--deadline-seconds":
        print(json.dumps({"error": "usage: sockdiag.py --netns-fd N --deadline-seconds S"}))
        return 2
    deadline = time.monotonic() + float(argv[3])
    try:
        wire = enter_and_dump(int(argv[1]), deadline=deadline)
    except DumpError as exc:
        wire = {"error": str(exc)}
    except OSError as exc:
        wire = {"error": f"socket-table dump failed: {exc}", "errno": exc.errno}
    print(json.dumps(wire))
    return 0 if "error" not in wire else 3


if __name__ == "__main__":
    raise SystemExit(_child_main(sys.argv[1:]))
