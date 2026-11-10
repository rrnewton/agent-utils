---
title: 'socket-liveness-proof: prove a filesystem socket node is dead before destructive recovery reclaims its tree'
status: closed
priority: 2
issue_type: feature
assignee: cl-fence
created_at: 2026-11-10T17:46:18+00:00
updated_at: 2026-11-10T21:00:00+00:00
closed_at: 2026-11-10T21:00:00+00:00
---

# Description

[opus 5.5] Destructive wrkslots recovery (`recover-ownerless-validate-batch` and the census behind remove) refuses every selected tree that contains a filesystem socket node: `_capture_lsof_process_path_census` reports "selected tree contains a filesystem socket node; socket-object liveness is unproven". A consumer's validation cleanup therefore never reclaims stale checkouts that hold dead sockets. The refusal is correct today, because a socket path's lstat device/inode and the sockfs inode numbers in /proc/PID/fd are different identity domains, so no numeric comparison can prove a socket has no owner. Bypassing it in the consumer would evade the shared destructive authority, so the proof belongs here.

Reported by a consumer coordinator with a design lead (not a tested proof). The existing alias controls in py/wrkslots/tests/test_lifecycle.py (relative bindings, hardlinks, an unlinked original, an original name rebound to another socket) must all be kept.

# Design

DESIGN FIRST, reviewed by the lead before any code. The design must settle:

- Evidence: a bounded NETLINK_SOCK_DIAG dump with UNIX_DIAG_VFS, giving the kernel's association between Unix socket identity and VFS (device, inode). Authenticate the kernel sender, request sequence and network namespace; parse the complete multipart response through DONE; refuse on errors, truncation, interrupted or incomplete dumps. Validate the device encoding and the interface's 32-bit VFS inode width; never truncate an inode and call it a nonmatch. Keep socket cookies where available.
- Composition with the existing generation-bound fd and network-namespace census: any matching owner or bound table entry refuses; an inaccessible namespace, unknown family, missing mapping, orphaned originating namespace, PID reuse or unreadable evidence also refuses. Keep the private-tree fence and the final same-UID closure model; no timestamp exemptions.
- Only after negative evidence, a bounded no-payload connect(). Success, timeout, a permission error, an unexpected errno or an unknown type refuses. ECONNREFUSED counts only with the same unchanged socket identity; ENOENT needs an explicit disappearance and re-census outcome. Check parent and path identity before and after; refuse replacements. Receipts distinguish a passive inventory from an active probe.
- A dry-run real-data adapter: normal-user reads only, deletes nothing, never constructs sudo or calls the root-owned census path; reports paths, distinct identities, dead/live/unknown counts and named reasons, with every visibility gap REFUSED.

# Acceptance Criteria

- Lead-approved design in Notes.
- Tests first: dead stale nodes, live listeners, live non-listening owners despite ECONNREFUSED, disappearance and rebinding, unreadable evidence, inode/device collisions, incomplete dumps, namespace churn; all existing alias controls kept; mutants that remove the listener, owner, visibility or identity checks, or swap the ENOENT/ECONNREFUSED meanings, are killed.
- Python first; a Rust parity note for the Rust edition's owner.
- Activation note: a consumer that loads its own pinned copy activates the change only when its pin moves.

# Notes

Owner: cl-fence (devbig030) after #1201 batch-grouped-fencing, which works in the same census and fence code (lead, 2026-11-10).

## Design as approved and built (lead, 2026-11-10)

[opus 5.5] The lead approved the design in /tmp/dev-agent-utils-handoffs/1230-design.md with three answers: a calibration socket inside the sealed tree is acceptable (a distinct `.wrkslots-calib.<pid>.<start>.<tag>` name, removed before the verdict with the root identity re-checked, and stale ones cleaned up after a crash); the probe always runs in destructive recovery and never in the dry run without `--probe`; the command is `socket-census`. Amendment on namespaces: dump our own namespace in-process, enter others from a child, and treat an unenterable namespace as able to prove LIVE by a bound path but never DEAD.

Built in `py/wrkslots/sockdiag.py` (the stdlib-only evidence layer, which is also the namespace child) and `py/wrkslots/cli.py` (`_prove_socket_nodes` and its parts, `_require_dead_socket_nodes`, the `socket-census` command). `_capture_lsof_process_path_census` now proves every socket node DEAD instead of refusing; the census then goes on unchanged, and its bound-name matching resolves names through the dumps' VFS identities.

Refinements found while building, all reported to the lead:

- **A unix_diag dump is not a snapshot.** The kernel resumes each 32 KB reply chunk by its position in a hash bucket and never sets NLM_F_DUMP_INTR. On a host with about 100k Unix sockets, a live held socket was missing from about one dump in six. A dump therefore proves LIVE only.
- **The dual-type probe** closes that gap. connect() finds a pathname socket by inode in one table for every namespace (unix_find_socket_byinode) and answers EPROTOTYPE for a socket of another type. So a stream probe and a seqpacket probe both refused means no socket of any type is bound to the node. Checked for a bound non-listening stream socket, seqpacket, dgram, a listener, and a socket held only in another namespace. Bind always creates a new inode, so nothing can bind to an existing node later.
- **The lead approved amendment (b)** on these conditions, all implemented and tested:
  - A visibility gap (an unenterable netns, or a non-dumpable process left unread) stops blocking DEAD once both probes are refused, before and after the second dump.
  - The node's identity (dev, ino, nlink, mode, ctime) and the parent's (dev, ino, ctime) must be unchanged throughout.
  - Any non-refused answer is final.
  - The gap is recorded as "gap overridden by dual-type probe: ..." in the verdict and under the finish journal's `x.socket_proof`. That record is audit-only, so it raises no minimum version.
  - A self-test in an unprivileged user and network namespace must give exactly ECONNREFUSED (stream) and EPROTOTYPE (seqpacket) while the socket is held, and ECONNREFUSED to both once it is released. Otherwise the override is off and gaps stay UNKNOWN. The result is cached per boot ID, per process.
  - Its temporary node is in a fresh private temp directory rather than the slot, because the slot is sealed during recovery; the lookup it tests does not depend on the filesystem.
- **Inodes are compared by their low 32 bits** (UNIX_DIAG_VFS width), never by truncating the node's inode. Many st_ino values on btrfs exceed 2^32.
- **Device matching.** A dumped device equal to the node's kernel-encoded st_dev, or to its mount's superblock device in mountinfo, proves LIVE without calibration, since LIVE refuses anyway. Calibration (memoized per device for the whole proof) is needed only to rule a candidate out.
- **Namespace churn.** A new namespace is dumped in further rounds (at most 4) instead of refusing outright. A namespace created after the first read cannot hold a socket bound to an existing node, because a socket's namespace is fixed when it is created.
- **The namespace status checks** use /proc/thread-self/ns/net. SIOCGSKNS needs CAP_NET_ADMIN.
- **Non-dumpable same-UID processes** (systemd --user, sd-pam, gnome-keyring, a setuid helper) hide their namespace and descriptors. Destructive recovery reads them with one batched root `find -P` and drops a batch on any diagnostic. The dry run reports them as REFUSED gaps.
- **Held-socket closure.** A Unix socket held both before and after the dumps but absent from every dump (re-checked against one extra dump) is a gap. Without it, the orphaned-namespace control would have looked dead in the dry run.
- **Doctor stays read-only** (#1172). It reports a stale calibration directory as `stale-socket-calibration`; recovery and `socket-census --probe` remove it.

Cost on devbig030 (1,100 same-UID processes, 190k descriptors, about 100k Unix sockets in the host namespace): one namespace scan takes 0.74 s; the host-namespace dump takes 0.35 s; the 10 other namespaces are dumped by parallel children. A passive census takes 2.5-4.5 s; `--probe` takes about twice that.

Tests (all in `py/wrkslots/tests/test_proc_status.py`, plus the updated alias controls in `test_lifecycle.py`):

- Parser refusals: no DONE, DUMP_INTR, NLMSG_ERROR, wrong seq, port or sender, MSG_TRUNC, malformed or overrunning attributes, DONE with an error, data after DONE, record limit.
- Verdict rules: a low-32 candidate, calibration skipped or failed, gaps.
- An unenterable namespace proving LIVE by name only.
- Every bound socket type; a dead node; unlink, rebind, a file replacement, parent replacement, and ctime changes on the node and the parent.
- A stale node DEAD only with the probe; a bound stream socket LIVE with and without dump evidence; a socket held only in flight in SCM_RIGHTS.
- Disappearance rerun; churn rounds and the churn limit; PID reuse; orphaned held sockets.
- Calibration against a real socket; stale calibration cleanup.
- The privileged namespace reader.
- The PrivateNetwork-style case: a socket held only inside an unenterable user+net namespace stays NOT DEAD while held and becomes DEAD (gap overridden) after release.
- The failed-self-test fallback.
- The journal record bound.
- `socket-census` running no sudo, writing nothing passively, and refusing `--project-root`.

Every existing socket alias control is kept. They now refuse as LIVE. The orphaned-namespace control is caught by the dual-type probe.

Mutants killed: single-type probe; EPROTOTYPE not LIVE; success treated as dead; ENOENT/ECONNREFUSED swapped; low32 treated as a nonmatch; calibration skipped; override without the self-test; gaps ignored; dump LIVE disabled. One survivor is equivalent: dropping the pre-connect identity check is still caught by the post-connect check, which runs before the answer is used.

## Rust parity note (for the other edition's owner)

The other edition's census has no socket-node proof and keeps refusing socket nodes, which is safe. Parity needs:

- the DEAD rule above (dual-type probe before and after a second dump, identity with ctime, the self-test gate for overriding gaps);
- a netlink parser checked against `sockdiag.parse_dump` by a byte-level differential on recorded dumps;
- the probe outcome table;
- the `x.socket_proof` journal record, read as an ignorable extension.

## Activation

A consumer that loads its own pinned copy gets this only when its pin moves past the landing commit. Until then its cleanup keeps refusing socket-node trees, which is safe.

## Follow-up: the self-test's directory (lead, 2026-11-10)

[opus 5.5] Landed in 3447358a, the self-test used `tempfile.TemporaryDirectory`, which honours TMPDIR, so a caller whose TMPDIR was inside a slot would have put the self-test's node in that slot's tree. The lead approved landing first, with these follow-up points:
1. TMPDIR is ignored: the directory goes under /run/user/<uid> when that is a directory this user owns, else under /tmp.
2. The self-test does not run, and counts as failed so the override stays off, when that parent resolves inside any census target.
3. The directory is named `wrkslots-probe-selftest.<pid>.<start-ticks>.<random>`, made 0700, and removed on every path. Each run first sweeps stale ones: owned by this user, creator gone, holding only `s`.
4. Tests cover removal after a pass, a rigged failure, a holder that never starts and a holder that dies, the 0700 mode, a TMPDIR inside a target being ignored, the in-target refusal, and the sweep.

## Resolution

Delivered in 3447358a and 63b81733 as described above.

Cost measured on a consumer's live validate checkouts (2026-11-10 16:48 ET, devbig030, load average about 90). The run was one passive `socket-census` pass over all 31 idle checkouts, under nice -n 19 and ionice -c3:
- 0 socket nodes in any of them, so no namespace scan, dump or probe ran, and nothing was refused.
- 2.96 s wall, 0.97 s user and 1.93 s system CPU, 72 MB RSS.
- 0 /proc entries read. The whole cost was the tree walk: about 398k entries in 42.8k directories.

When a tree does hold a socket node, one passive census on the same host took 2.4 s. It read about 382k /proc links and 378k /proc directory entries over about 10k PID directories (two namespace scans), and made 3 dumps; the host's dump lists about 100k Unix sockets in about 0.35 s. `--probe`, which recovery uses, roughly doubles that and adds the once-per-boot self-test.

