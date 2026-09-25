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
- Headless health stores one boot-, image-, PID-, and Linux-start-time-bound
  runner identity and derives the older PID/start fields only for public
  compatibility output. It accepts a dead result only from a typed
  `runner_alive: false` receipt bound to that exact saved generation.
  Transport, decoding, and runtime errors remain `unknown`
  regardless of diagnostic wording. Exact positive liveness additionally
  requires a readable matching start time and non-zombie state; procfs
  ambiguity is typed `null` and maps to `unknown`. When asynchronous start first
  returns without a PID, the first later exact positive worker receipt is
  persisted only under the same private outer generation lock.
- The project-local configuration is one strict versioned document containing
  launch profiles and one optional default Herdr workspace selector. Starts
  persist only the resolved workspace ID and one normalized launch
  specification; they do not copy profile policy or workspace labels into a
  second authority.
- `relocate` is a fail-closed, token-bound one-pane move state machine. It
  requires Herdr to atomically compare the saved terminal ID while executing
  `pane move`; Herdr 0.8.0 lacks that precondition and therefore this client
  refuses before moving a production pane. Once that Herdr prerequisite is
  available, relocation records the exact old route and intended destination
  in its journal before moving. Recovery finds the same terminal generation at either the old or
  destination route, revalidates the live harness, commits only the derived
  route to the existing session record, and removes the journal. Goal and queue
  state remain name-scoped and are not copied.
- Long-lived Muse sessions may lose their version banner from the viewport.
  Once the exact boot/PID/start/image identity has been re-proved, delivery uses
  Herdr's unwrapped terminal source plus the ruled composer and current footer.
  If the complete prompt cannot be established, Enter remains withheld and the
  delivery stays quarantined for explicit reconciliation. Once an exact prompt
  receives one Enter, a stale composer redraw does not authorize a second
  Enter: the first key may already have been accepted and a repeated key could
  act on a later dialog.

## Canonical fleet snapshot boundary

The state model has one writable owner for each fact:

- `.agentctl/profiles.json` owns reusable project launch policy. The session's
  nested `LaunchSpec` owns the resolved immutable launch intent; flat v1 fields
  are emitted only as a derived compatibility view and are never stored by a
  current writer.
- `runner_identity`, `custom_process_identity`, and `foreign_shell_identity`
  are mutually adapter-specific complete process identities. PID/start fields
  exist only at the legacy decode and public-output boundaries.
- The nested native-session object owns the observed/asserted conversation
  identity. The nested goal object points to one `kind: goal` queue artifact;
  that artifact's phase, text, and ID determine delivery state.
- The session record owns workspace/tab/pane routing. A relocation journal is
  a temporary transaction intent, not another current route; it is deleted
  only after the record, queue binding, and live terminal agree.
- The outer session record owns desired launch/lifecycle state. A headless
  worker's separate runtime row owns only observed runtime state and accepts
  commands from the exact outer generation token. Typed receipts may refresh
  derived route/process observations but cannot rewrite launch intent.
- `health.json` and archived `output.json` are derived observations. Neither is
  consulted as lifecycle, route, launch, or goal authority.

Session-v1/v2 fields and the old headless presentation/permission files are
decode-only migration inputs. Any successful current write emits only the
tagged session-v4/runtime-v3 form and removes superseded sidecars after the
canonical publication succeeds.

`agentctl`'s versioned session record is the authority for launch intent,
runtime identity, route, goal, and queue state. A project integration that maps
those sessions to an external task system must derive its snapshot from one
bounded `agentctl list`; it may add only project fields such as agent name,
display role, and external task ID. It must not copy argv, process identities,
routes, goals, or lifecycle state into another writable binding file.

The bounded migration is:

1. Keep the old periodic snapshot service disabled while its bindings name
   terminals that no longer exist.
2. Configure current agent names and external task IDs in the project-private
   integration, then prove every name resolves to one healthy current
   `agentctl` generation.
3. Generate the external snapshot from that single read. Refuse missing,
   duplicate, unknown, or unhealthy generations rather than retaining old
   routes.
4. Enable periodic publication only after byte-level readback and a restart
   test. The service publishes observations; it never becomes a launch, goal,
   or ownership authority.

External task-system lookup belongs in a project-private plugin because it is
deployment-specific. The normalized session and health snapshot stay in the
open-source core. A current private producer may read schema-2 or schema-3
legacy bindings during migration, but those binding rows are inputs to the
project mapping only: pane IDs, tokens, launch commands, goals, and lifecycle
must come from the bounded agentctl snapshot. The 2026-09-25 incident was stale
project data naming retired `wP` panes while the live fleet was in `wM`/`wK`;
it was not an incompatibility between the producer's schema-2/schema-3 reader.

Until agentctl exposes one atomic multi-session snapshot, a private integration
can take a generation-consistent bounded observation with `list`, native-goal
reads for the selected exact tokens, then a second `list`; it publishes only if
the two name/token sets are identical. This is a bounded compatibility bridge,
not a second writable session registry. Once the atomic snapshot exists, the
plugin should delete that retry loop and consume the single core receipt.

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

This branch was rebased onto agent-utils
`e6a545e93917bd245b2512b0eed99954798349b7`; later rebases must update this
provenance before publication.
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
5. **Native Muse editor receipt.** The current bounded terminal proof now
   handles a scrolled-away version banner and physical wrapping after exact
   process verification, while retaining quarantine and explicit transcript
   reconciliation for ambiguity. A future Herdr/Muse editor-buffer API could
   preserve hard-newline distinctions that a rendered terminal necessarily
   loses; until then failure to establish the complete normalized rendering
   must continue to withhold Enter. `recover-start` does not enter the delivery
   state machine and never supplies that Enter.
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
