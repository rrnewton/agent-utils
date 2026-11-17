---
title: 'boxed-muse-lane-identity: boxed Muse lanes die at harness start and cannot retire'
status: closed
priority: 2
issue_type: task
labels:
- agentctl
- wrkslots
created_at: 2026-11-15T20:00:00+00:00
updated_at: 2026-11-17T18:35:00+00:00
closed_at: 2026-11-17T18:35:00+00:00
---

# Description

[watermelon-full-preview] A boxed Muse lane cannot start:
`agentctl start <name> --profile muse-... --slot <slot>
--slot-isolation userns` fails at readiness and the record cannot be
retired through the front door (`stop` exits 69, `custom harness
"muse" is not the foreground process`). Live probe 2026-11-15
(throwaway slot and lanes; full table in the #1569 report): the pane
shows the Muse binary print its banner, then `Error setting
effective UID to root: EINVAL: Invalid argument`, and exit. Claude
and Codex die identically under userns (Claude's start even returns
success first: readiness pins the launcher, then the process dies,
and only the slot-owner transfer notices `owner PID is not live`).
The wrkslots userns maps a single uid (`<uid> <uid> 1` in sandbox
`enter_userns`), so no inner uid 0 exists for the harness launcher's
own sandbox entry. Host-side `/proc` observation of boxed processes
works (verified), so identity pinning is collateral: the pinned
process is simply dead. Muse under root isolation is additionally
refused by construction (`RELAY_HARNESSES` covers only claude/codex).
The Eden slot-box entry refusal found alongside (Git sibling rule on
the backing path) is fixed separately under #1569.

# Design

- Primary (mu-boxedlane, in flight): adapter-declared boxed launch
  for Muse: under a userns slot launch, run the runtime beside the
  launcher (`muse.real`) with the harness's own sandbox disabled —
  the slot box is the sandbox — in both Rust and Python agentctl.
  Working set proven live: that binary + disable-sandbox completes
  start/send/read/stop under userns. The userns relay remains the
  fallback if the direct runtime launch cannot be made sound.
- Surface the real error: a harness that dies before readiness must
  fail start with the pane evidence, not `cannot pin custom
  process: No such process` in Rust or a false `running` record in
  Claude's case (readiness re-verify after the composer check; a
  refused slot-owner transfer must fail the start).
- Retire path: `stop` must archive a `launch_failed` (or falsely
  `running`) record whose pinned process is kernel-dead without
  requiring foreground proof (identity-dead proof with the expected
  token, as the dead-native retire already does where reachable).
- Adapter group-leader coverage: claude/muse declare no
  `native_group_leader` (only codex does), so any multi-process
  foreground group fails pinning under userns. Decide per adapter
  and error with the extra foreground processes named, so Claude
  under userns is not the next surprise.

# Acceptance Criteria

- Boxed (userns) Muse, Claude and Codex lanes each complete
  start/readiness/send/read/stop on a throwaway slot, with the exact
  flag sets recorded in Notes.
- A harness dying before readiness fails start naming its pane
  error, and its record retires via stop; regression tests simulate
  the dead-before-ready launch in both implementations.
- Root-isolation Muse is relayed or refused naming supported modes.

# Notes

Probe evidence (2026-11-15, mu-edenimage + mu-boxedlane,
independently reproduced): userns start failures rc75/rc69 with the
EINVAL pane line for all three harnesses; cgroup (limits-only) muse
lane completes the full cycle; the working mount-sandboxed set is
the runtime-beside-launcher launch above. Full matrix in the #1569
boxed-lanes report.

## Resolution

Landed on github/main: aaa67b93 (boxed Muse lanes on the runtime
beside the launcher), 1f3649c7 (retire launch_failed records whose
pinned process is dead), 2fbe7e6d (Muse behind the root slot
relay), and the sealed root-helper stack as tip 0f6905c9
(216e7fc2, 840ea917, 5e3a1bae, 5431086a, 0f6905c9; gated
dde3669d, gate verdict PARTIAL with no failed steps, 7 cross
checks skipped-unverified). Reviews 3/4/5 are closed; review 5
cleared the landing after L1+L2 and left the install-readiness
items I1-I5, which are filed as #1792 boxed-root-helper-fd-mounts.
No sudoers line exists; install remains an owner decision after
#1792 lands and passes its independent install review.

## Security review: the root-launcher spec parser (threat model)

Attacker: the invoking user, with full control of the spec and
their own files, trying to turn the root helper into root. The
helper (`py/wrkslots/sandbox.py`) is the only root code path; the
spec arrives on stdin, so no sudoers wildcard exists. DECISION
(2026-11-16, after the second review): root mode IS sealed with
PR_SET_NO_NEW_PRIVS. Harness launchers need a setuid step, so they
fail honestly in a sealed root box (probed: the launcher dies
`Error setting effective UID to root: EPERM`, exactly as in a user
namespace); userns remains their mount-sandboxed route, and root
mode serves non-setuid boxed tools until an unsealed redesign
passes review. One line per review finding (both rounds):

- F1 root unlink/lookup of a user path: CLOSED — the spec travels
  on stdin (`root_helper` fixed argv, `_exec_root_helper` dup2);
  the helper takes no path argument and unlinks nothing as root.
- F2 root reads of user symlinks + ptrace window: CLOSED — the
  drop (keeping only CAP_SYS_ADMIN) happens before stdin is read;
  registry reads are post-drop, O_NOFOLLOW, regular-file only;
  dumpable is 0 from entry to exec and never raised (unit-tested).
  Registration is documented as an accident guard, not a boundary.
- F3 bypassable denylist: CLOSED by TRUSTED ALLOWLIST — allowed
  roots come only from trusted data (passwd home or a user-owned
  home, the registry row's checkout paths compared exactly for
  slot/project, the control directory, the per-slot state
  directory, the box `/tmp`, the user's runtime spec and credential
  directories), with ownership checked along every ancestor.
  Spec-claimed aliases and cwd are not roots; an alias qualifies
  only as the same directory (device+inode) or an ancestor of the
  home. Every source and target resolves symlink-free via openat2
  (fail closed when unavailable), checks run post-drop and again
  against the built view, and the finished mount points are
  verified before exec. The system-prefix denylist remains as belt
  and braces, and masks keep only that refusal.
- F4 NO_NEW_PRIVS contradiction: DECIDED the other way — the seal
  is restored (see the decision above); the allowlist is defense
  in depth, not the only boundary.
- F5 stale installed copy / `-B` mismatch: CLOSED — always `-I -B`;
  the launcher reads the installed copy's `_HELPER_VERSION` and
  refuses on mismatch with this file's version.
- F6 flag-clearing remount: CLOSED — the nosuid/nodev remount
  preserves existing ro/noexec flags and touches only binds made.
- F7 groups/tmp_size/interpreter: CLOSED — groups are the spec's
  claimed set intersected with the caller's own (via sudo's parent
  /proc status, falling back to the group database, equality
  refused otherwise); `tmp_size` is strictly validated before it
  reaches a tmpfs option string; the install step requires a
  root-owned interpreter with a root-owned sys.path under `-I`.

Held from the review: identity only from SUDO_UID/SUDO_GID; root
invoker refused; private propagation (nothing reaches the host
mount table); all capabilities dropped before exec; fixed fs types
(tmpfs or bind); no ptrace surface while CAP_SYS_ADMIN is held.

Residual risks: (1) a large Python module still parses user JSON —
now as the user, post-drop, under a closed schema (unknown keys
refuse); root surface before the drop is env reading plus the
uid/gid drop itself. (2) Same-uid races between check and mount
remain theoretically open; the attacker is the invoking user, who
already owns every file involved, and mounts are fd-pinned for
sources. (3) Installed-copy version skew fails closed.

Operational note: a box's private mount namespace pins host binds
that existed when it started; a long-lived boxed process therefore
blocks host cleanup of those binds until it exits. Boxed test
processes must be retired promptly, never left idle.

Tests: `py/wrkslots/tests/test_sandbox_root_spec.py` (unregistered
and forged claims refused; closed schema; tmp_size; spec mount
policy refused; paths outside trusted roots and symlink-smuggled
targets refused; seal and dumpable pinned). Live: `wrkslots run
<slot> --isolation root -- /bin/true` and `/usr/bin/id` pass with
all checks; a sealed root Muse relay lane fails honestly at
startup (the launcher's EPERM), as decided. The narrow sudoers line
ships only after this design passes an independent adversarial
review; install (owner, per host; nothing installed by the agent):
`sudo install -d -o root -g root -m 0755 /usr/local/lib/wrkslots`
and `sudo install -o root -g root -m 0755 py/wrkslots/sandbox.py
/usr/local/lib/wrkslots/sandbox.py`, plus one wildcard-free
sudoers line naming the invoking user, the host's resolved
python realpath, `-I -B`, the installed helper, and the literal
`enter-root` (spec on stdin; one line serves every project).
