# agentctl Phase 1a boundary and follow-up slices

This note records the boundary of the initial liveness patch and the remaining
work needed for a complete supervisor-grade recovery design. The first slice is
deliberately detection-first: it does not restart a harness or replay input.

## Phase 1a implemented here

- `status`, `list`, and `health` distinguish the durable lifecycle from an
  observed runtime state. A positively proved shell fallback is `dead`; a
  detector or transport failure without shell proof is `unknown`. A stale
  expected Muse label cannot suppress an exact-process absence plus stable
  descendant-free idle-shell proof.
- Each name is checked independently. The aggregate result cannot lose later
  sessions because an earlier record is malformed, stale, or has a contended
  lifecycle lock. Lock contention is a non-durable `unknown` row and every
  runtime call is cancellation-bound by the one-shot or overall watch deadline.
- Per-generation observations retain exact reasons, first/current detection
  times, and the last unhealthy/unknown event in `health.json`.
- `health --watch SECONDS --interval SECONDS` polls for a bounded duration and
  exits immediately on a non-healthy result. It does not start, stop, or send.
- Custom Muse startup retries transient incomplete process snapshots within the
  existing startup deadline, including command and malformed-response failures,
  and preserves the last probe diagnostic if the deadline expires. Pane launch,
  process inspection, readiness reads, final verification, and reporting share
  that deadline rather than receiving independent control timeouts.
- Before launching Muse, the record durably captures the canonical executable
  path, executable device/inode, and complete argv. `recover-start` requires the
  saved generation plus an operator-observed PID, matches those facts twice,
  requires a visibly idle composer before publishing `idle`, and can resume
  after an interrupted report/reconciliation step.
- New records identify an owner-selected profile, non-secret environment
  variable names, and whether the runtime is owned or foreign. Secret values
  are never written to the record.
- Headless health stores the runner PID and Linux start time and accepts a dead
  result only from a typed `runner_alive: false` receipt bound to that exact
  saved generation. Transport, decoding, and runtime errors remain `unknown`
  regardless of diagnostic wording. Exact positive liveness additionally
  requires a readable matching start time and non-zombie state; procfs
  ambiguity is typed `null` and maps to `unknown`. When asynchronous start first
  returns without a PID, the first later exact positive worker receipt is
  persisted only under the same private outer generation lock.

## Launch-intent gaps

The new fields are enough to recover the narrow pre-identity Muse failure. They
are not yet a general restart specification:

1. Native Codex and Claude startup delegates executable resolution to Herdr.
   The record retains the harness kind and exact literal arguments, but Herdr
   does not currently return the resolved executable path/image identity.
2. A profile launch retains the profile name, resolved argv, and
   environment variable names. It intentionally does not persist environment
   values. A future restart command must reopen the same private profile,
   validate a non-secret version/digest contract, and refuse if policy changed;
   it must never reconstruct secret values from hashes or logs.
   Existing raw harness arguments, and therefore the new exact launch argv, are
   persisted literally; command-line arguments are not an acceptable secret
   transport. A complete restart design needs named secret-source references
   rather than values.
3. Goal and native-session fields are already durable, but no restart command
   is authorized to replay a goal or prompt. Reconciliation must separate
   reattaching to a proven native session from starting a new conversation.

## Python/Rust port status at this base

This checkout is based on `b899ff9871d6dde7840f9a6f08e466f999e32c4d`.
It is not an all-Rust port:

- Python installs `agentctl = agentctl.cli:main` and
  `herdr-agent = agentctl.legacy_cli:main` from
  `py/agentctl/pyproject.toml`. Its implementation includes
  `cli.py`, `subagents.py`, `sessions.py`, the `foreign/` headless workers,
  `mcp.py`, and the polling `chat*.py` modules.
- Rust builds the `agentctl` and `herdr-agent` binaries from
  `rs/agentctl/src/main.rs` and `legacy_main.rs`. It does contain the shared
  interactive commands (`adopt`, `profiles`, `skill`, and the health/recovery
  commands in this slice) plus `chat_runtime.rs`, `chat_service.rs`, and
  `plugins.rs` for the event-driven subscription-plugin Chat host.
- Rust does not implement the Python headless Herdr/tmux worker stack, the
  Python MCP service, the polling Chat transport/launcher, or the Python
  migrate/reset/repair adapter paths. Wrapper/helper Rust and the subscription
  Chat host are therefore not evidence of full CLI parity. The repository root
  `README.md` describes the Rust artifact as the interactive core plus
  event-driven plugin Chat, which matches the source at this revision.

## Subsequent independently reviewable slices

1. **Reconcile state machine.** Add an explicit `reconcile` operation for
   `starting`, `launch_failed`, `running`, and `stopping`. Every transition must
   require the saved generation and a stable runtime proof. Re-running it after
   interruption must converge without duplicate input.
2. **Reboot recovery.** Record the Linux boot ID for every owned runtime and
   classify pre-reboot process identities as expired. Detect and safely remove
   a bad bare-harness recovery only after proving its pane, process group,
   executable, argv, and absence of a resumable native session. Never infer
   ownership from a process name alone.
3. **Per-agent cgroup and exit attribution.** Create a bounded cgroup scope per
   owned generation, retain its stable identity in the record, and collect exit
   status/OOM/signal facts. Define behavior for hosts without delegated cgroup
   control before enabling this by default.
4. **Sixty-second service proof.** Ship example user service and timer units
   around bounded `health --watch 60`, with explicit registry paths, output
   retention, startup ordering, and no restart authority over agents.
5. **Long Muse/Claude prompt correctness.** Replace screen-substring acceptance
   with a bounded identity-bearing submission receipt that cannot confuse
   wrapped, truncated, repeated, or scrolled prompt text. A real long-prompt
   reproduction returned exit 76 (`possibly_submitted`) while the complete
   prompt was still in the Claude composer; one later human Enter submitted it.
   The state machine must distinguish `composer populated but unsubmitted` from
   `possibly submitted`, expose a safe explicit Enter-only recovery, keep truly
   uncertain submissions in quarantine, and test terminal-width and
   retained-history boundaries. The Phase 1a `recover-start` path deliberately
   does not enter this delivery state machine and never supplies that Enter.
6. **Native executable provenance.** Extend Herdr's native start receipt to
   return the exact executable/process generation it launched, then persist it
   with the same strength as the custom Muse path.

Each slice needs Python/Rust differential coverage where the capability is
shared. Edition-specific headless, MCP, polling Chat, and subscription Chat
paths remain separate capabilities rather than evidence of an all-Rust port.

## Post-install live-registry proof of concept

Do not run these commands until the reviewed build is installed. Set the
absolute registry and the actual names first; none of the inspection commands
starts, stops, or sends to an agent.

```sh
REGISTRY=/absolute/path/to/.agentctl
DEAD_NAMES=(first-dead-name second-dead-name third-dead-name)

for NAME in "${DEAD_NAMES[@]}"; do
  agentctl status "$NAME" --registry "$REGISTRY" > "/tmp/${NAME}.status.json" 2> "/tmp/${NAME}.status.err"
  status_rc=$?
  python3 -m json.tool "/tmp/${NAME}.status.json"
  printf '%s status rc=%s\n' "$NAME" "$status_rc"
done
```

Each positively proved shell fallback must print `health: "unhealthy"`,
`runtime_state: "dead"`, an exact `health_reason`, and exit 1. A transient
detector/transport failure must instead print `health: "unknown"` and exit 1.
The durable first/current timestamps are in both the output and
`$REGISTRY/NAME/health.json`.

Check the whole mixed registry in one invocation and capture the command's own
status before inspecting its output:

```sh
agentctl list --registry "$REGISTRY" > /tmp/agentctl-mixed-list.json 2> /tmp/agentctl-mixed-list.err
list_rc=$?
python3 -m json.tool /tmp/agentctl-mixed-list.json
printf 'mixed list rc=%s\n' "$list_rc"
```

The expected result is exit 1 with every valid name still present; a malformed
or stale record must be an `unknown` row and must not hide later healthy/dead
rows.

For the exact failed Muse generation, inspect the saved intent and one fresh
Herdr process snapshot. The PID selection below accepts exactly one foreground
process whose full argv is byte-for-byte equal to the saved argv:

```sh
NAME=failed-muse-name
RECORD="$REGISTRY/$NAME/agent.json"
TOKEN=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["token"])' "$RECORD")
PANE=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["pane_id"])' "$RECORD")
herdr pane process-info --pane "$PANE" > /tmp/agentctl-muse-process.json
PID=$(python3 -c 'import json,sys; r=json.load(open(sys.argv[1], encoding="utf-8")); p=json.load(open(sys.argv[2], encoding="utf-8"))["process_info"]; m=[x["pid"] for x in p["foreground_processes"] if x.get("argv")==r["launch_argv"]]; assert len(m)==1, m; print(m[0])' "$RECORD" /tmp/agentctl-muse-process.json)
herdr pane read "$PANE" --source full --lines 2000 > /tmp/agentctl-muse-before.txt
agentctl recover-start "$NAME" --expected-token "$TOKEN" --expected-pid "$PID" --registry "$REGISTRY" > /tmp/agentctl-muse-recover.json 2> /tmp/agentctl-muse-recover.err
recover_rc=$?
herdr pane read "$PANE" --source full --lines 2000 > /tmp/agentctl-muse-after.txt
cmp --silent /tmp/agentctl-muse-before.txt /tmp/agentctl-muse-after.txt
transcript_cmp_rc=$?
printf 'recover rc=%s transcript cmp rc=%s\n' "$recover_rc" "$transcript_cmp_rc"
python3 -m json.tool /tmp/agentctl-muse-recover.json
```

The expected result is `recover rc=0`, lifecycle `running`, the same saved
generation and exact process identity, and `transcript cmp rc=0`. A nonzero
comparison is a stop condition for investigation, not permission to retry the
prompt. `recover-start` itself never invokes launch or prompt-delivery code; a
second invocation is permitted only to finish an interrupted identity/report
reconciliation for the same token and PID.
