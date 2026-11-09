//! Command-line interface for rebuilding and reading the observer index.

use std::path::PathBuf;

use clap::{CommandFactory, Parser, Subcommand};
use serde::Serialize;

use crate::index::{read_decision, read_pressure_plan, rebuild_policy_index};
use crate::{read_index, rebuild_index, ReplaySummary};

#[derive(Parser)]
#[command(
    name = "wrkslotsd",
    version,
    about = "Validate slot event history and maintain a disposable read-only observer index",
    long_about = "Validate a hash-linked EVENTS.<machine> directory and materialize a disposable SQLite observer index. Replay covers path-independent record semantics; the shadow-policy JSON is not the originating .wrkslots.yml registry, and configuration-coupled paths, layout defaults, landed refs, Git registrations, and nested-repository discovery remain with Python. Optional typed configuration and caller-supplied captured evidence produce fail-closed, digest-bound diagnostic decisions. This slice does not independently recheck /proc, boot ID, the systemd invocation link, cgroup existence, or TaskGraph claim ownership, so it cannot emit actionable eligibility. Explain and plan reconstruct and self-consistency-check decisions and reject evidence that has aged out; the disposable database is not authenticated against a coordinated same-UID rewrite, live EVENTS, or repositories absent from its inputs. Initial publication requires a filesystem that supports renameat2(RENAME_NOREPLACE). This migration slice has no daemon, socket, lease, cleanup, rescue, deletion, apply, or repository mutation capability.",
    after_help = "Examples:\n  wrkslotsd rebuild --events-dir /srv/control/EVENTS.node-a --index /var/tmp/slot-observer.sqlite\n  wrkslotsd rebuild --events-dir /srv/control/EVENTS.node-a --index /var/tmp/slot-observer.sqlite --config policy.json --evidence evidence.json\n  wrkslotsd explain --index /var/tmp/slot-observer.sqlite --slot worker-a\n  wrkslotsd plan --index /var/tmp/slot-observer.sqlite --target-bytes 107374182400"
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    command: Command,
}

// Split into flattened groups (#1003): as one 57-variant enum, clap's
// derived `augment_subcommands` allocated a single ~2.9 MiB stack frame
// in the debug profile and overflowed the default 2 MiB test-thread
// stack. Each group's builder is its own smaller function; the
// flattened surface (names, order, arguments) is unchanged.
#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum Command {
    #[command(flatten)]
    Writers(Writers),
    #[command(flatten)]
    Readers(Readers),
    #[command(flatten)]
    Recovery(Recovery),
    #[command(flatten)]
    Lifecycle(Lifecycle),
}

#[derive(Subcommand)]
pub(crate) enum Writers {
    /// Remove and archive one slot (writer; fixture registries only until cutover)
    Remove {
        /// Active slot to remove
        slot: String,
        /// Live coordinator PID authorizing the removal
        #[arg(long, value_name = "PID")]
        coordinator_pid: i64,
        /// Current slot generation; refuses stale callers
        #[arg(long, value_name = "N")]
        expected_generation: i64,
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
    /// Strictly replay an event log and atomically replace the derived index
    Rebuild {
        /// Real EVENTS.<machine> directory to validate; it is never modified
        #[arg(long, value_name = "DIR")]
        events_dir: PathBuf,
        /// Disposable SQLite index path outside the event directory
        #[arg(long, value_name = "FILE")]
        index: PathBuf,
        /// Shadow-policy JSON (not .wrkslots.yml); requires --evidence
        #[arg(long, value_name = "FILE", requires = "evidence")]
        config: Option<PathBuf>,
        /// Caller-supplied captured census JSON; requires --config
        #[arg(long, value_name = "FILE", requires = "config")]
        evidence: Option<PathBuf>,
    },
    /// Read and cross-check summary counters from an existing derived index
    Status {
        /// Existing disposable SQLite index
        #[arg(long, value_name = "FILE")]
        index: PathBuf,
    },
    /// Explain one reconstructed shadow decision; never changes source or index state
    Explain {
        /// Existing disposable SQLite index
        #[arg(long, value_name = "FILE")]
        index: PathBuf,
        /// Active or pending-operation slot name
        #[arg(long)]
        slot: String,
    },
    /// Validate a project's .wrkslots.yml and print the normalized configuration
    ValidateConfig {
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
    /// Validate one ACTIVE.<machine>.json shard and print the normalized state
    ParseState {
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
    /// Validate one ARCHIVED.<machine>.json shard and print it with records as stored
    ParseArchive {
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
    /// Validate one slot's generation-bound handoff sidecar and print it (or null)
    ParseHandoff {
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
        /// Active slot whose current-generation sidecar should be read
        #[arg(long, value_name = "NAME")]
        slot: String,
    },
    /// Validate one slot's hold record and print it as stored (or null)
    ParseHold {
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
        /// Slot whose hold record should be read
        #[arg(long, value_name = "NAME")]
        slot: String,
    },
    /// Shape-validate one recovery journal in the control directory and print it as stored
    ParseJournal {
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
        /// Journal file name inside the control directory
        #[arg(long, value_name = "NAME")]
        name: String,
    },
    /// Validate the ACTIVE and ARCHIVED shards as a pair and print both
    ParseStates {
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
    /// Validate every machine's ACTIVE and ARCHIVED shards as one registry and print each pair
    ParseGlobal {
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
    },
    /// Load every active record's hold across all machines and print one entry per record
    ParseHolds {
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
    },
    /// List the outstanding recovery-journal file names for the registry
    ParseJournals {
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
    },
    /// Print the Python `status` command's envelope (JSON) or human form
    StatusText {
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
        /// Read every ACTIVE machine shard instead of only the selected machine
        #[arg(long)]
        all_machines: bool,
        /// Limit output to this slot name
        #[arg(long, value_name = "SLOT")]
        slot: Option<String>,
        /// Output format: json or human
        #[arg(long, value_name = "FORMAT", default_value = "human", value_parser = ["human", "json"])]
        format: String,
    },
}

#[derive(Subcommand)]
pub(crate) enum Readers {
    /// Print the Python `doctor` command's envelope (JSON) or human form
    DoctorText {
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
        /// Read every ACTIVE machine shard instead of only the selected machine
        #[arg(long)]
        all_machines: bool,
        /// Output format: json or human
        #[arg(long, value_name = "FORMAT", default_value = "human", value_parser = ["human", "json"])]
        format: String,
    },
    /// Print the Python `audit` command's payload (JSON) or human form
    AuditText {
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
        /// Resumable cache-accounting state path (default: a project-keyed file below XDG_CACHE_HOME or ~/.cache)
        #[arg(long, value_name = "PATH")]
        cache_census_state: Option<String>,
        /// Maximum cache traversal work units for this invocation
        #[arg(long, value_name = "ENTRIES", default_value_t = 100_000)]
        cache_work_limit: i64,
        /// Cache traversal wall budget in seconds for this invocation
        #[arg(long, value_name = "SECONDS", default_value_t = 5.0)]
        cache_wall_seconds: f64,
        /// Total cache-glob planning time in seconds across every subject (audit plans outside the state lock after a short-lock snapshot)
        #[arg(long, value_name = "SECONDS", default_value_t = 30.0)]
        cache_plan_total_seconds: f64,
        /// Exit non-zero on attention (1) or unknown (2) slots
        #[arg(long)]
        gate: bool,
        /// Audit every machine's ACTIVE shard instead of only the local machine's
        #[arg(long)]
        all_machines: bool,
        /// Output format: json or human
        #[arg(long, value_name = "FORMAT", default_value = "human", value_parser = ["human", "json"])]
        format: String,
    },
    /// Print an exec-only shell command line that enters a slot's box (Python `shell-command`)
    #[allow(clippy::enum_variant_names)]
    ShellCommand {
        /// Registered slot name (omitted with --box)
        slot: Option<String>,
        /// Slot type (default: agent)
        #[arg(long, value_name = "TYPE", default_value = "agent", value_parser = ["agent", "validate"])]
        slot_type: String,
        /// Enter a coordinator box instead of a slot's box; no SLOT
        #[arg(long = "box")]
        box_mode: bool,
        /// With --box: the box name (default: coordinator)
        #[arg(long, value_name = "NAME", default_value = "coordinator")]
        name: String,
        /// With --box: worktrees or project (default: configuration sandbox scope)
        #[arg(long, value_name = "SCOPE", value_parser = ["worktrees", "project"])]
        writable: Option<String>,
        /// With --box: the box's working directory (default: the project root)
        #[arg(long, value_name = "PATH")]
        cwd: Option<PathBuf>,
        /// Passed to run (default: configuration sandbox isolation, else userns)
        #[arg(long, value_name = "MODE", value_parser = ["userns", "cgroup", "root"])]
        isolation: Option<String>,
        /// Interactive shell (default: $SHELL, else /bin/bash)
        #[arg(long, value_name = "PATH")]
        shell: Option<String>,
        /// Output format: text prints the command line; json prints {command, slot_path, isolation}
        #[arg(long, value_name = "FORMAT", default_value = "text", value_parser = ["text", "json"])]
        format: String,
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
        /// Program to run boxed instead of the interactive shell, after a literal --
        #[arg(
            trailing_var_arg = true,
            allow_hyphen_values = true,
            value_name = "COMMAND"
        )]
        command: Vec<String>,
    },
    /// Diagnose every active record (owner process, heartbeat, hold) and print one status object per record
    ParseStatus {
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
    },
    /// Audit every registered and on-disk slot (verdicts, caches, disk, gate summary)
    ParseAudit {
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
        /// Resumable cache-accounting state path (default: a project-keyed file below XDG_CACHE_HOME or ~/.cache)
        #[arg(long, value_name = "PATH")]
        cache_census_state: Option<String>,
        /// Maximum cache traversal work units for this invocation
        #[arg(long, value_name = "ENTRIES", default_value_t = 100_000)]
        cache_work_limit: i64,
        /// Cache traversal wall budget in seconds for this invocation
        #[arg(long, value_name = "SECONDS", default_value_t = 5.0)]
        cache_wall_seconds: f64,
        /// Total cache-glob planning time in seconds across every subject (audit plans outside the state lock after a short-lock snapshot)
        #[arg(long, value_name = "SECONDS", default_value_t = 30.0)]
        cache_plan_total_seconds: f64,
        /// Audit every machine's ACTIVE shard instead of only the local machine's
        #[arg(long)]
        all_machines: bool,
    },
    /// Diagnose unpublished or stale-after-rebase HEADs for every checkout on the selected machine
    Unpushed {
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
        /// Only diagnose this slot
        #[arg(long, value_name = "SLOT")]
        slot: Option<String>,
    },
    /// Diagnose unpublished or stale-after-rebase HEADs, printing the Python `unpushed` output
    UnpushedText {
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
        /// Only diagnose this slot
        #[arg(long, value_name = "SLOT")]
        slot: Option<String>,
        /// Output format: human prints one row per checkout; json prints the payload
        #[arg(long, value_name = "FORMAT", default_value = "human", value_parser = ["human", "json"])]
        format: String,
    },
    /// Diagnose the registry (tolerant status records plus operational findings)
    ParseDoctor {
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
        /// Diagnose every machine instead of only the selected machine
        #[arg(long)]
        all_machines: bool,
    },
    /// Diagnose filesystem drift between active rows and managed slot directories
    ParseStorage {
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
    },
    /// Diagnose Git drift between active rows and worktree registrations
    ParseStorageGit {
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
    },
    /// Protect one active slot from cleanup and removal (writer; fixture registries only until cutover)
    Hold {
        /// Active slot name
        slot: String,
        /// Why the slot is protected
        #[arg(long, value_name = "TEXT")]
        reason: String,
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
    /// Release a slot hold (writer; fixture registries only until cutover)
    Unhold {
        /// Active slot name
        slot: String,
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
    /// Refresh one slot's heartbeat for its exact owner (writer; fixture registries only until cutover)
    Heartbeat {
        /// Active slot name
        slot: String,
        /// Recorded agent name
        #[arg(long, value_name = "NAME")]
        agent: String,
        /// Recorded live owner PID
        #[arg(long, value_name = "PID")]
        owner_pid: i64,
        /// Current slot generation
        #[arg(long, value_name = "N")]
        expected_generation: i64,
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
    /// Publish one generation-bound handoff sidecar (writer; fixture registries only until cutover)
    WriteHandoff {
        /// Active agent slot name
        slot: String,
        /// Recorded agent name
        #[arg(long, value_name = "NAME")]
        agent: String,
        /// Recorded live owner PID
        #[arg(long, value_name = "PID")]
        owner_pid: i64,
        /// Current slot generation
        #[arg(long, value_name = "N")]
        expected_generation: i64,
        /// External handoff file (outside every Git working tree)
        #[arg(long, value_name = "FILE")]
        from_file: PathBuf,
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
    /// Give one slot generation back while its owner runs (writer; fixture registries only until cutover)
    Release {
        /// Active agent slot name
        slot: String,
        /// Recorded agent name
        #[arg(long, value_name = "NAME")]
        agent: String,
        /// Exact live owner PID
        #[arg(long, value_name = "PID")]
        owner_pid: i64,
        /// Current slot generation
        #[arg(long, value_name = "N")]
        expected_generation: i64,
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
}

#[derive(Subcommand)]
pub(crate) enum Recovery {
    /// Record one coordinator read of a slot's handoff (writer; fixture registries only until cutover)
    ReadHandoff {
        /// Active agent slot name
        slot: String,
        /// Live current coordinator PID recorded with the read event
        #[arg(long, value_name = "PID")]
        coordinator_pid: i64,
        /// Record coordinator authorization as optional read provenance
        #[arg(long)]
        coordinator_authorized: bool,
        /// Deprecated compatibility flag that always refuses
        #[arg(long)]
        adopt_legacy_change: bool,
        /// Exact current generation; accepted only with the deprecated adoption flag
        #[arg(long, value_name = "N")]
        expected_generation: Option<i64>,
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
    /// Bind an unowned slot to its live owner (writer; fixture registries only until cutover)
    Adopt {
        /// Active slot awaiting an owner
        slot: String,
        /// Agent name already recorded on the slot
        #[arg(long, value_name = "NAME")]
        agent: String,
        /// Live owner PID to bind
        #[arg(long, value_name = "PID")]
        owner_pid: i64,
        /// Current slot generation; refuses stale callers
        #[arg(long, value_name = "N")]
        expected_generation: i64,
        /// Transient systemd scope containing --owner-pid (both scope options together)
        #[arg(long, value_name = "UNIT.scope")]
        task_scope_unit: Option<String>,
        /// Lowercase 32-hex systemd InvocationID for --task-scope-unit
        #[arg(long, value_name = "HEX32")]
        task_scope_invocation_id: Option<String>,
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
    /// Hand a slot with a provably dead owner to its live successor (writer; fixture registries only until cutover)
    TransferOwner {
        /// Active slot whose owner died
        slot: String,
        /// Agent name already recorded on the slot (with --reopen: live agent to record)
        #[arg(long, value_name = "NAME")]
        agent: String,
        /// Live successor PID to bind
        #[arg(long, value_name = "PID")]
        owner_pid: i64,
        /// Current slot generation; refuses stale callers
        #[arg(long, value_name = "N")]
        expected_generation: i64,
        /// Coordinator-authorized rebind of a released, unremoved generation; starts a new generation
        #[arg(long)]
        reopen: bool,
        /// Live coordinator PID recorded with a --reopen rebind
        #[arg(long, value_name = "PID")]
        coordinator_pid: Option<i64>,
        /// Record that the coordinator authorized this --reopen rebind (required with --reopen)
        #[arg(long)]
        coordinator_authorized: bool,
        /// Why the coordinator rebinds a released generation; recorded in the event log (required with --reopen)
        #[arg(long, value_name = "TEXT")]
        reason: Option<String>,
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
    /// Record coordinator recovery evidence for a slot with no recorded owner (writer; fixture registries only until cutover)
    RecoverUnboundOwner {
        /// Active slot with no usable owner lease
        slot: String,
        /// Recorded live coordinator PID
        #[arg(long, value_name = "PID")]
        coordinator_pid: i64,
        /// Current slot generation
        #[arg(long, value_name = "N")]
        expected_generation: i64,
        /// Why owner evidence cannot be recovered
        #[arg(long, value_name = "TEXT")]
        recovery_note: String,
        /// Validation evidence (repeatable)
        #[arg(long, value_name = "TEXT", required = true)]
        validation: Vec<String>,
        /// Known limitation to retain (repeatable)
        #[arg(long, value_name = "TEXT")]
        limitation: Vec<String>,
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
    /// Recover one exact absent agent row (writer; fixture registries only until cutover)
    RecoverAbsentAgentRow {
        /// Slot from SLOT=GENERATION=SHA256
        slot: String,
        /// Current slot generation
        #[arg(long, value_name = "N")]
        expected_generation: i64,
        /// Exact source ACTIVE record SHA-256
        #[arg(long, value_name = "SHA256")]
        record_sha256: String,
        /// Apply instead of planning the plan
        #[arg(long = "apply")]
        apply_mode: bool,
        /// The caller asserts it is the recorded coordinator
        #[arg(long)]
        coordinator_authorized: bool,
        /// Stable coordinator process PID (required with --apply)
        #[arg(long, value_name = "PID")]
        coordinator_pid: Option<i64>,
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
    /// Recover an explicit bounded batch of absent agent rows (writer; fixture registries only until cutover)
    RecoverAbsentAgentRows {
        /// One SLOT=GENERATION=SHA256 row (repeatable)
        #[arg(long, value_name = "ROW")]
        row: Vec<String>,
        /// UTF-8 file with one row per line
        #[arg(long, value_name = "FILE")]
        input: Option<PathBuf>,
        /// Apply instead of planning each plan
        #[arg(long = "apply")]
        apply_mode: bool,
        /// The caller asserts it is the recorded coordinator
        #[arg(long)]
        coordinator_authorized: bool,
        /// Stable coordinator process PID (required with --apply)
        #[arg(long, value_name = "PID")]
        coordinator_pid: Option<i64>,
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
    /// Recover absent validation rows named by a JSON input file (writer; fixture registries only until cutover)
    RecoverAbsentValidateRows {
        /// JSON input file with the exact rows to recover
        #[arg(long, value_name = "FILE")]
        input: PathBuf,
        /// Apply instead of planning
        #[arg(long = "apply")]
        apply_mode: bool,
        /// The caller asserts it is the recorded coordinator
        #[arg(long)]
        coordinator_authorized: bool,
        /// Stable coordinator process PID (required with --apply)
        #[arg(long, value_name = "PID")]
        coordinator_pid: Option<i64>,
        /// Output format: text or json
        #[arg(long, value_name = "FORMAT", default_value = "text")]
        format: String,
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
    /// Salvage and remove one exact unregistered agent worktree (writer; fixture registries only until cutover)
    RecoverOwnerlessAgentWorktree {
        /// Direct child of the managed agent directory
        path: String,
        /// Source repository that registered the worktree
        #[arg(long, value_name = "PATH", required = true)]
        repository: String,
        /// Exact worktree HEAD
        #[arg(long, value_name = "SHA", required = true)]
        head: String,
        /// Branch the worktree has checked out
        #[arg(long, value_name = "BRANCH", conflicts_with = "detached")]
        branch: Option<String>,
        /// The worktree has a detached HEAD contained in a remote branch
        #[arg(long)]
        detached: bool,
        /// Remote name recorded for the worktree
        #[arg(long, value_name = "REMOTE", required = true)]
        remote: String,
        /// SHA-256 of the remote's single fetch URL
        #[arg(long, value_name = "SHA256", required = true)]
        remote_url_sha256: String,
        /// Exact HANDOFF.md digest after the coordinator has read it
        #[arg(long, value_name = "SHA256")]
        handoff_sha256: Option<String>,
        /// Apply instead of planning
        #[arg(long = "apply")]
        apply_mode: bool,
        /// The caller asserts it is the recorded coordinator
        #[arg(long)]
        coordinator_authorized: bool,
        /// Stable coordinator process PID (required with --apply)
        #[arg(long, value_name = "PID")]
        coordinator_pid: Option<i64>,
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
    /// Retire one exact unregistered standalone clone (writer; fixture registries only until cutover)
    RecoverOwnerlessAgentClone {
        /// Direct child of the managed agent directory
        path: String,
        /// Remote for one Git directory inside the clone, as KEY=URL (repeatable)
        #[arg(long = "remote-url", value_name = "KEY=URL")]
        remote_urls: Vec<String>,
        /// Exact HANDOFF digest after the coordinator has read it (repeatable)
        #[arg(long = "handoff-sha256", value_name = "SHA256")]
        handoff_sha256s: Vec<String>,
        /// Delete the gitignored files the plan lists
        #[arg(long)]
        discard_ignored: bool,
        /// Apply instead of planning
        #[arg(long = "apply")]
        apply_mode: bool,
        /// The caller asserts it is the recorded coordinator
        #[arg(long)]
        coordinator_authorized: bool,
        /// Stable coordinator process PID (required with --apply)
        #[arg(long, value_name = "PID")]
        coordinator_pid: Option<i64>,
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
    /// Relocate the exact ownerless agent cache tree (writer; fixture registries only until cutover)
    RecoverOwnerlessAgentCache {
        /// Apply instead of planning
        #[arg(long = "apply")]
        apply_mode: bool,
        /// The caller asserts it is the recorded coordinator
        #[arg(long)]
        coordinator_authorized: bool,
        /// Stable coordinator process PID (required with --apply)
        #[arg(long, value_name = "PID")]
        coordinator_pid: Option<i64>,
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
    /// Recover one bounded batch of private ownerless validation checkouts (writer; fixture registries only until cutover)
    RecoverOwnerlessValidateBatch {
        /// Project-relative private ownerless validation checkout (repeatable)
        #[arg(long = "checkout", value_name = "PATH")]
        checkouts: Vec<String>,
        /// Absolute frozen validation checkout (repeatable)
        #[arg(long = "frozen-validate-checkout", value_name = "PATH")]
        frozen_validate_checkouts: Vec<String>,
        /// Aligned terminal validation record (repeatable)
        #[arg(long = "completed-record", value_name = "PATH")]
        completed_records: Vec<String>,
        /// Aligned source repository (repeatable)
        #[arg(long = "repository", value_name = "PATH")]
        repositories: Vec<String>,
        /// Proof manifest aligned with each checkout (repeatable)
        #[arg(long = "validation-proof-manifest", value_name = "PATH")]
        validation_proof_manifests: Vec<String>,
        /// The caller asserts it is the recorded coordinator
        #[arg(long)]
        coordinator_authorized: bool,
        /// Stable coordinator process PID
        #[arg(long, value_name = "PID")]
        coordinator_pid: Option<i64>,
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
    /// Classify retained validation checkouts without removing them (writer; fixture registries only until cutover)
    ClassifyOwnerlessValidateBatch {
        /// Project-relative private ownerless validation checkout (repeatable)
        #[arg(long = "checkout", value_name = "PATH")]
        checkouts: Vec<String>,
        /// Absolute frozen validation checkout (repeatable)
        #[arg(long = "frozen-validate-checkout", value_name = "PATH")]
        frozen_validate_checkouts: Vec<String>,
        /// Aligned terminal validation record (repeatable)
        #[arg(long = "completed-record", value_name = "PATH")]
        completed_records: Vec<String>,
        /// Aligned source repository (repeatable)
        #[arg(long = "repository", value_name = "PATH")]
        repositories: Vec<String>,
        /// Proof manifest aligned with each checkout (repeatable)
        #[arg(long = "validation-proof-manifest", value_name = "PATH")]
        validation_proof_manifests: Vec<String>,
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
    /// Recover an explicit bounded batch of absent agent rows, printing the Python output (writer; fixture registries only until cutover)
    RecoverAbsentAgentRowsText {
        /// One SLOT=GENERATION=SHA256 row (repeatable)
        #[arg(long, value_name = "ROW")]
        row: Vec<String>,
        /// UTF-8 file with one row per line
        #[arg(long, value_name = "FILE")]
        input: Option<PathBuf>,
        /// Apply instead of planning each plan
        #[arg(long = "apply")]
        apply_mode: bool,
        /// The caller asserts it is the recorded coordinator
        #[arg(long)]
        coordinator_authorized: bool,
        /// Stable coordinator process PID (required with --apply)
        #[arg(long, value_name = "PID")]
        coordinator_pid: Option<i64>,
        /// Output format: human prints the outcome lines; json prints the payload
        #[arg(long, value_name = "FORMAT", default_value = "human", value_parser = ["human", "json"])]
        format: String,
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
    /// Recover absent validation rows named by a JSON input file, printing the Python output (writer; fixture registries only until cutover)
    RecoverAbsentValidateRowsText {
        /// JSON input file with the exact rows to recover
        #[arg(long, value_name = "FILE")]
        input: PathBuf,
        /// Apply instead of planning
        #[arg(long = "apply")]
        apply_mode: bool,
        /// The caller asserts it is the recorded coordinator
        #[arg(long)]
        coordinator_authorized: bool,
        /// Stable coordinator process PID (required with --apply)
        #[arg(long, value_name = "PID")]
        coordinator_pid: Option<i64>,
        /// Output format: human prints the ROW lines; json prints the payload
        #[arg(long, value_name = "FORMAT", default_value = "human", value_parser = ["human", "json"])]
        format: String,
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
    /// Recover one bounded batch of private ownerless validation checkouts, printing the Python output (writer; fixture registries only until cutover)
    RecoverOwnerlessValidateBatchText {
        /// Project-relative private ownerless validation checkout (repeatable)
        #[arg(long = "checkout", value_name = "PATH")]
        checkouts: Vec<String>,
        /// Absolute frozen validation checkout (repeatable)
        #[arg(long = "frozen-validate-checkout", value_name = "PATH")]
        frozen_validate_checkouts: Vec<String>,
        /// Aligned terminal validation record (repeatable)
        #[arg(long = "completed-record", value_name = "PATH")]
        completed_records: Vec<String>,
        /// Aligned source repository (repeatable)
        #[arg(long = "repository", value_name = "PATH")]
        repositories: Vec<String>,
        /// Proof manifest aligned with each checkout (repeatable)
        #[arg(long = "validation-proof-manifest", value_name = "PATH")]
        validation_proof_manifests: Vec<String>,
        /// The caller asserts it is the recorded coordinator
        #[arg(long)]
        coordinator_authorized: bool,
        /// Stable coordinator process PID
        #[arg(long, value_name = "PID")]
        coordinator_pid: Option<i64>,
        /// Output format: human prints the retained lines; json prints the payload
        #[arg(long, value_name = "FORMAT", default_value = "human", value_parser = ["human", "json"])]
        format: String,
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
}

#[derive(Subcommand)]
pub(crate) enum Lifecycle {
    /// Classify retained validation checkouts without removing them, printing the Python output (writer; fixture registries only until cutover)
    ClassifyOwnerlessValidateBatchText {
        /// Project-relative private ownerless validation checkout (repeatable)
        #[arg(long = "checkout", value_name = "PATH")]
        checkouts: Vec<String>,
        /// Absolute frozen validation checkout (repeatable)
        #[arg(long = "frozen-validate-checkout", value_name = "PATH")]
        frozen_validate_checkouts: Vec<String>,
        /// Aligned terminal validation record (repeatable)
        #[arg(long = "completed-record", value_name = "PATH")]
        completed_records: Vec<String>,
        /// Aligned source repository (repeatable)
        #[arg(long = "repository", value_name = "PATH")]
        repositories: Vec<String>,
        /// Proof manifest aligned with each checkout (repeatable)
        #[arg(long = "validation-proof-manifest", value_name = "PATH")]
        validation_proof_manifests: Vec<String>,
        /// Output format: human prints the state lines; json prints the payload
        #[arg(long, value_name = "FORMAT", default_value = "human", value_parser = ["human", "json"])]
        format: String,
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
    /// Set, clear, or show the per-host retire freeze (#780)
    Freeze {
        /// Action: set, clear, or status
        #[arg(value_parser = ["set", "clear", "status"])]
        action: String,
        /// Freeze reason words (required for set)
        reason: Vec<String>,
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
    /// Classify outstanding create journals without locks or writes, printing the Python output
    ClassifyCreateJournals {
        /// Requested slot name the journals must be unrelated to (not needed with --all-kinds)
        slot: Option<String>,
        /// Requested agent name the journals must be unrelated to (not needed with --all-kinds)
        #[arg(long, value_name = "NAME")]
        agent: Option<String>,
        /// Report every lifecycle journal (kind, slot, agent, owner PID/state, age, blocks) instead of classifying creates (#1098)
        #[arg(long)]
        all_kinds: bool,
        /// Slot type: agent or validate
        #[arg(long, value_name = "TYPE", default_value = "agent")]
        slot_type: String,
        /// Output format: human prints the journal lines; json prints the payload
        #[arg(long, value_name = "FORMAT", default_value = "human", value_parser = ["human", "json"])]
        format: String,
        /// Classify every machine's journals instead of only the local machine's
        #[arg(long)]
        all_machines: bool,
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
    /// Report or reap stray Git worktree registrations (#775)
    ReapStrayWorktrees {
        /// Additional source repository to census (repeatable)
        #[arg(long, value_name = "PATH")]
        repository: Vec<String>,
        /// Preserve a rescue ref and remove each reapable registration
        #[arg(long = "apply")]
        apply_mode: bool,
        /// Output format: human prints the stray lines; json prints the payload
        #[arg(long, value_name = "FORMAT", default_value = "human", value_parser = ["human", "json"])]
        format: String,
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
    /// Report or reclaim slot-state homes whose slot is already archived (#796)
    ReclaimOrphanState {
        /// Reclaim each reapable orphan home (default: report only)
        #[arg(long = "apply")]
        apply_mode: bool,
        /// Output format: human prints the orphan lines; json prints the payload
        #[arg(long, value_name = "FORMAT", default_value = "human", value_parser = ["human", "json"])]
        format: String,
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
    /// Resume a journal or start bounded ownerless validation cleanup (writer; fixture registries only until cutover)
    Recover {
        /// Live current coordinator PID recorded with recovery
        #[arg(long, value_name = "PID")]
        coordinator_pid: Option<i64>,
        /// Select one exact interrupted create or finish
        #[arg(long, value_name = "SLOT")]
        slot: Option<String>,
        /// Required when starting ownerless validation cleanup
        #[arg(long)]
        coordinator_authorized: bool,
        /// Discard an incomplete temp write only when its prior durable file is valid
        #[arg(long)]
        discard_partial: bool,
        /// Discard a stale image copy of an interrupted image-backed removal
        #[arg(long)]
        discard_stale_image_copy: bool,
        /// Discard an interrupted import-existing journal
        #[arg(long)]
        abort_import: bool,
        /// Compatibility spelling for an ownerless validate-fresh worktree
        #[arg(long, value_name = "PATH")]
        legacy_validate_checkout: Option<String>,
        /// Project-relative unregistered validation Git worktree
        #[arg(long, value_name = "PATH")]
        ownerless_validate_checkout: Option<String>,
        /// Project-relative unregistered validate-cargo directory
        #[arg(long, value_name = "PATH")]
        ownerless_validate_cargo_home: Option<String>,
        /// Absolute frozen validation checkout
        #[arg(long, value_name = "PATH")]
        frozen_validate_checkout: Option<String>,
        /// Project-relative terminal validation record
        #[arg(long, value_name = "PATH")]
        completed_record: Option<String>,
        /// Project-relative source repository
        #[arg(long, value_name = "PATH")]
        repository: Option<String>,
        /// Why no retained run record exists
        #[arg(long, value_name = "TEXT")]
        recovery_note: Option<String>,
        /// Canonical validation proof manifest
        #[arg(long, value_name = "PATH")]
        validation_proof_manifest: Option<String>,
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
    /// Create linked worktrees and register a slot (writer; fixture registries only until cutover)
    Create {
        /// New slot name
        slot: String,
        /// Slot type: agent or validate
        #[arg(long, value_name = "TYPE")]
        slot_type: String,
        /// Confirm the coordinator assigned this slot
        #[arg(long)]
        coordinator_authorized: bool,
        /// Registered agent name
        #[arg(long, value_name = "NAME")]
        agent: String,
        /// Task or work-item identifier
        #[arg(long, value_name = "TASK")]
        task: String,
        /// Short human-readable reason for the slot
        #[arg(long, value_name = "TEXT")]
        purpose: String,
        /// Live owner PID to bind (omit only when the owner will adopt)
        #[arg(long, value_name = "PID")]
        owner_pid: Option<i64>,
        /// Live assigned coordinator PID
        #[arg(long, value_name = "PID")]
        coordinator_pid: i64,
        /// Source repository as NAME=PATH (repeat once per checkout)
        #[arg(long, value_name = "NAME=PATH")]
        repo: Vec<String>,
        /// Configured Git remote as NAME=REMOTE
        #[arg(long, value_name = "NAME=REMOTE")]
        remote: Vec<String>,
        /// Expected fetch URL as NAME=URL
        #[arg(long, value_name = "NAME=URL")]
        remote_url: Vec<String>,
        /// New local branch as NAME=BRANCH (once per repo)
        #[arg(long, value_name = "NAME=BRANCH")]
        branch: Vec<String>,
        /// Start ref as NAME=REF
        #[arg(long, value_name = "NAME=REF")]
        start: Vec<String>,
        /// Allow create below the provisioning floor, never below emergency
        #[arg(long)]
        override_disk_floor: bool,
        /// Transient systemd scope containing --owner-pid (both scope options together)
        #[arg(long, value_name = "UNIT.scope")]
        task_scope_unit: Option<String>,
        /// Lowercase 32-hex systemd InvocationID for --task-scope-unit
        #[arg(long, value_name = "HEX32")]
        task_scope_invocation_id: Option<String>,
        /// Storage representation: config, worktree, or image
        #[arg(long, value_name = "KIND", default_value = "config")]
        representation: String,
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
    /// Create linked worktrees and register a slot, printing the Python `create` output (writer; fixture registries only until cutover)
    CreateText {
        /// New slot name
        slot: String,
        /// Slot type: agent or validate
        #[arg(long, value_name = "TYPE")]
        slot_type: String,
        /// Confirm the coordinator assigned this slot
        #[arg(long)]
        coordinator_authorized: bool,
        /// Registered agent name
        #[arg(long, value_name = "NAME")]
        agent: String,
        /// Task or work-item identifier
        #[arg(long, value_name = "TASK")]
        task: String,
        /// Short human-readable reason for the slot
        #[arg(long, value_name = "TEXT")]
        purpose: String,
        /// Live owner PID to bind (omit only when the owner will adopt)
        #[arg(long, value_name = "PID")]
        owner_pid: Option<i64>,
        /// Live assigned coordinator PID
        #[arg(long, value_name = "PID")]
        coordinator_pid: i64,
        /// Source repository as NAME=PATH (repeat once per checkout)
        #[arg(long, value_name = "NAME=PATH")]
        repo: Vec<String>,
        /// Configured Git remote as NAME=REMOTE
        #[arg(long, value_name = "NAME=REMOTE")]
        remote: Vec<String>,
        /// Expected fetch URL as NAME=URL
        #[arg(long, value_name = "NAME=URL")]
        remote_url: Vec<String>,
        /// New local branch as NAME=BRANCH (once per repo)
        #[arg(long, value_name = "NAME=BRANCH")]
        branch: Vec<String>,
        /// Start ref as NAME=REF
        #[arg(long, value_name = "NAME=REF")]
        start: Vec<String>,
        /// Allow create below the provisioning floor, never below emergency
        #[arg(long)]
        override_disk_floor: bool,
        /// Transient systemd scope containing --owner-pid (both scope options together)
        #[arg(long, value_name = "UNIT.scope")]
        task_scope_unit: Option<String>,
        /// Lowercase 32-hex systemd InvocationID for --task-scope-unit
        #[arg(long, value_name = "HEX32")]
        task_scope_invocation_id: Option<String>,
        /// Storage representation: config, worktree, or image
        #[arg(long, value_name = "KIND", default_value = "config")]
        representation: String,
        /// Output format: human prints the created lines; json prints the payload
        #[arg(long, value_name = "FORMAT", default_value = "human", value_parser = ["human", "json"])]
        format: String,
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
    /// Record a slot's finish handoff in its active record (writer; fixture registries only until cutover)
    Finish {
        /// Active slot whose work is finished
        slot: String,
        /// Recorded agent name
        #[arg(long, value_name = "NAME")]
        agent: String,
        /// Recorded live owner PID
        #[arg(long, value_name = "PID")]
        owner_pid: i64,
        /// Current slot generation; refuses stale callers
        #[arg(long, value_name = "N")]
        expected_generation: i64,
        /// Command and result evidence (repeatable, at least one)
        #[arg(long, value_name = "TEXT", required = true)]
        validation: Vec<String>,
        /// Known limitation to retain (repeatable)
        #[arg(long, value_name = "TEXT")]
        limitation: Vec<String>,
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
    /// Bootstrap the selected machine's shard pair and event log (writer; fixture registries only until cutover)
    EnsureStates {
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
    /// Parse a .wrkslots.yml document (the strict subset or JSON) and print it as canonical JSON
    ParseConfig {
        /// Configuration document to parse; it is never modified
        #[arg(long, value_name = "FILE")]
        config: PathBuf,
    },
    /// Print the normal end-to-end workflow (Python `quickstart`)
    Quickstart,
    /// Inspect handoff-read slots awaiting safe retirement (Python `retirement-queue`)
    RetirementQueue {
        /// Output format: human or json
        #[arg(long, value_name = "FORMAT", default_value = "human", value_parser = ["human", "json"])]
        format: String,
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
    /// Register already-created live worktrees (writer; fixture registries only until cutover)
    Register {
        /// Existing managed slot name
        slot: String,
        /// Type recorded at registration: agent or validate
        #[arg(long, value_name = "TYPE")]
        slot_type: Option<String>,
        /// Confirm that the coordinator authorized this registration
        #[arg(long)]
        coordinator_authorized: bool,
        /// Registered agent name
        #[arg(long, value_name = "AGENT")]
        agent: String,
        /// Task or work-item identifier
        #[arg(long, value_name = "TASK")]
        task: String,
        /// Short human-readable reason for the slot
        #[arg(long, value_name = "TEXT")]
        purpose: String,
        /// Live owner PID to bind
        #[arg(long, value_name = "PID")]
        owner_pid: i64,
        /// Live coordinator PID to bind
        #[arg(long, value_name = "PID")]
        coordinator_pid: i64,
        /// Confirm that the named worktrees are currently owned and in use
        #[arg(long)]
        verified_live: bool,
        /// Source repository as NAME=PATH (repeat once per checkout)
        #[arg(long, value_name = "NAME=PATH")]
        repo: Vec<String>,
        /// Configured Git remote as NAME=REMOTE
        #[arg(long, value_name = "NAME=REMOTE")]
        remote: Vec<String>,
        /// Expected fetch URL as NAME=URL
        #[arg(long, value_name = "NAME=URL")]
        remote_url: Vec<String>,
        /// Transient systemd scope containing --owner-pid (both scope options together)
        #[arg(long, value_name = "UNIT.scope")]
        task_scope_unit: Option<String>,
        /// Lowercase 32-hex systemd InvocationID for --task-scope-unit
        #[arg(long, value_name = "HEX32")]
        task_scope_invocation_id: Option<String>,
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
    /// Void a handoff-read retirement entry and rebind the slot to its live agent (writer; fixture registries only until cutover)
    SupersedeHandoff {
        /// Active slot whose handoff-read entry is superseded
        slot: String,
        /// Live agent name to record on the slot
        #[arg(long, value_name = "AGENT")]
        agent: String,
        /// Live new-owner PID to bind
        #[arg(long, value_name = "PID")]
        owner_pid: i64,
        /// Current slot generation; refuses stale callers
        #[arg(long, value_name = "N")]
        expected_generation: i64,
        /// Live coordinator PID recorded with the supersession
        #[arg(long, value_name = "PID")]
        coordinator_pid: i64,
        /// Record that the coordinator authorized this supersession (required)
        #[arg(long)]
        coordinator_authorized: bool,
        /// Why the lane supersedes the handoff; recorded in the event log
        #[arg(long, value_name = "TEXT")]
        reason: String,
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
    /// Convert or regenerate .wrkslots.yml (writer; fixture registries only until cutover)
    Config {
        #[command(subcommand)]
        action: Option<ConfigCommand>,
    },
    /// Partition reconstructed decisions into a bounded, non-executing pressure report
    Plan {
        /// Existing disposable SQLite index
        #[arg(long, value_name = "FILE")]
        index: PathBuf,
        /// Future eligibility target; currently zero rows can pass the rollout gates
        #[arg(long, default_value_t = 0)]
        target_bytes: u64,
        /// Optional stricter future-candidate count; indexed configuration remains the ceiling
        #[arg(long)]
        limit: Option<usize>,
    },
}

#[derive(Subcommand)]
pub(crate) enum ConfigCommand {
    /// Rewrite the configuration as literate YAML or as JSON
    Convert {
        /// Format to write: yaml or json
        #[arg(long = "to", value_name = "FORMAT", value_parser = ["yaml", "json"])]
        target: String,
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Machine shard override (else WRKSLOTS_MACHINE, else the configured machine)
        #[arg(long, value_name = "NAME")]
        machine: Option<String>,
    },
}

#[derive(Serialize)]
struct CliSummary<'a> {
    machine: &'a str,
    replay_count: u64,
    tip_sha256: &'a str,
    // Revisions legitimately span all of u64. JSON strings prevent consumers
    // backed by binary64 numbers from silently rounding them.
    active_revision: String,
    active_count: u64,
    archive_revision: String,
    archive_count: u64,
}

impl<'a> From<&'a ReplaySummary> for CliSummary<'a> {
    fn from(summary: &'a ReplaySummary) -> Self {
        Self {
            machine: &summary.machine,
            replay_count: summary.replay_count,
            tip_sha256: &summary.tip_sha256,
            active_revision: summary.active_revision.to_string(),
            active_count: summary.active_count,
            archive_revision: summary.archive_revision.to_string(),
            archive_count: summary.archive_count,
        }
    }
}

const QUICKSTART: &str = r##"wrkslots manages durable agent and validation worktree slots.

1. Initialize the project once. The running command is project-owned and is called with the agent
   name. It must return 0 for dead, 1 for alive, or 2 when it cannot determine the answer. init
   writes .wrkslots.yml as literate YAML with a comment above every key (older JSON projects
   still load; `wrkslots config convert --to yaml` converts them).

     wrkslots init . --worktrees-dir worktrees/slots --liveness-command tools/agent-liveness.py

2. Create a new linked worktree and branch from an existing source repository. The repository's
   configured origin is used by default; add --remote product=upstream to choose another remote,
   and add --remote-url product=URL when the caller must verify its fetch URL.

     wrkslots create slot01 \
       --slot-type agent --coordinator-authorized \
       --agent codex-1 --task task-123 --purpose "fix parser" \
       --coordinator-pid "$COORDINATOR_PID" --owner-pid "$OWNER_PID" \
       --repo product=product --branch product=codex/fix-parser

3. The exact recorded owner refreshes its heartbeat while working.

     wrkslots heartbeat slot01 \
       --agent codex-1 --owner-pid "$OWNER_PID" --expected-generation 1

4. Before exiting, the owner may record a clean, published handoff. This retains the slot.

     wrkslots write-handoff slot01 \
       --agent codex-1 --owner-pid "$OWNER_PID" --expected-generation 1 \
       --from-file /path/to/HANDOFF.md

     wrkslots finish slot01 \
       --agent codex-1 --owner-pid "$OWNER_PID" --expected-generation 1 \
       --validation "make test: pass"

   An owner that is done with the slot but keeps running gives it back after writing the
   handoff. The release is bound to that owner process, the generation, the current heartbeat,
   and the handoff digest. It stops this generation's heartbeat, and removal then no longer waits
   for the owner to exit or for the time-to-live. Leave the slot first: removal still refuses
   while any process, the owner included, uses it.

     wrkslots release slot01 \
       --agent codex-1 --owner-pid "$OWNER_PID" --expected-generation 1

5. Reading a handoff prints its exact contents and enqueues that generation without removing it.

     wrkslots read-handoff slot01 \
       --coordinator-pid "$CURRENT_COORDINATOR_PID"

6. After the registered running command and exact process identity prove the owner dead, and the
   recorded time-to-live expires without renewal, any later coordinator may remove the slot. A
   slot its owner released needs only the handoff read from step 5.
   Agent slots preserve dirty and unpushed work first; validate slots skip salvage. Salvage never
   stages a HANDOFF* file, and it pushes only to remotes listed in salvage_push_remotes. Work on
   any other remote is kept in a self-contained Git bundle under <control>/wrkslots-salvage/ (or
   under --salvage-archive-root), and removal begins only after the bundle and receipt pass
   readback checks. If a push to an allowed remote refuses and an operator explicitly approves
   durable local custody, pass an existing absolute directory outside the project.

     wrkslots remove slot01 \
       --coordinator-pid "$CURRENT_COORDINATOR_PID" --expected-generation 1

   Add `--salvage-archive-root "$HOME/temp/agent_checkouts"` to that remove command to enable
   this fallback for a refused push.

If a create or removal is interrupted, preserve all paths and run:

     wrkslots recover --coordinator-pid "$CURRENT_COORDINATOR_PID"

Box a command or agent to a slot (plain or image-backed alike): per-slot limits, a fresh /tmp,
the real $HOME read-only under a per-slot private layer, and only the slot, its Git directories,
the project's outputs (ai_docs, experiments), and shared harness state writable. `init` writes
every default into the configuration's sandbox section; inspect it with `sandbox show-config`.

     wrkslots run slot01 -- make test
     wrkslots run slot01 --isolation root -- HARNESS ...   # setuid helpers work inside
     wrkslots shell-command slot01 --format json            # for agent launchers
     wrkslots sandbox write-defaults                        # older projects: add missing keys

Box a (sub)coordinator that creates slots and launches subagents into them: the same view,
but every slot, the registry, and the slots' Git directories writable (--writable project: the
whole project root), with no slot. Image slots it creates are mounted on the host.

     wrkslots box --isolation root --cwd worktrees -- HARNESS ...
     wrkslots shell-command --box --cwd worktrees --format json   # for agent launchers

Use `wrkslots COMMAND --help` for the exact effects and inputs of one command.
"##;

/// Words after the first literal `--` in the process arguments, for
/// handlers that must know which side of `--` a word was on (the
/// Python CLI keeps the same raw arguments for this).
fn after_separator() -> Vec<String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().position(|arg| arg == "--") {
        Some(index) => args[index + 1..].to_vec(),
        None => Vec::new(),
    }
}

/// Python command spellings whose wrkslotsd command has another name.
const PY_RENAMED: &[(&str, &str)] = &[
    ("status", "status-text"),
    ("doctor", "doctor-text"),
    ("audit", "audit-text"),
    ("unpushed", "unpushed-text"),
    ("create", "create-text"),
    (
        "recover-absent-agent-rows",
        "recover-absent-agent-rows-text",
    ),
    (
        "recover-absent-validate-rows",
        "recover-absent-validate-rows-text",
    ),
    (
        "recover-ownerless-validate-batch",
        "recover-ownerless-validate-batch-text",
    ),
    (
        "classify-ownerless-validate-batch",
        "classify-ownerless-validate-batch-text",
    ),
];

/// Python spellings the Rust edition already serves under the same name.
const PY_IDENTITY: &[&str] = &[
    "adopt",
    "classify-create-journals",
    "config",
    "finish",
    "freeze",
    "heartbeat",
    "hold",
    "quickstart",
    "read-handoff",
    "reap-stray-worktrees",
    "reclaim-orphan-state",
    "recover",
    "recover-absent-agent-row",
    "recover-ownerless-agent-cache",
    "recover-ownerless-agent-clone",
    "recover-ownerless-agent-worktree",
    "recover-unbound-owner",
    "register",
    "release",
    "remove",
    "retirement-queue",
    "shell-command",
    "supersede-handoff",
    "transfer-owner",
    "unhold",
    "write-handoff",
];

/// Python commands with no Rust port yet; compat mode refuses them by name.
const PY_ONLY: &[&str] = &[
    "box",
    "clean-caches",
    "image",
    "import-existing",
    "init",
    "limits",
    "migrate",
    "relocate-repository",
    "remove-agent-batch",
    "remove-validate-batch",
    "retire-pending",
    "run",
    "sandbox",
    "worktrees-image",
];

type PyCompatTranslation = Option<(Vec<String>, Option<String>)>;

fn py_flag_present(head: &[String], flag: &str) -> bool {
    head.iter()
        .any(|arg| arg == flag || arg.starts_with(&format!("{flag}=")))
}

/// Whether this invocation speaks the Python CLI: routed as the
/// wrkslots Rust edition, leading Python globals, or a spelling no
/// working native call can mean.
fn py_compat_triggered(rest: &[String], leading_globals: bool, routed: bool) -> bool {
    if routed || leading_globals {
        return true;
    }
    let Some(cmd) = rest.first().map(String::as_str) else {
        return false;
    };
    match cmd {
        "doctor" | "audit" => true,
        "status" => !py_flag_present(rest, "--index"),
        "create"
        | "unpushed"
        | "recover-absent-agent-rows"
        | "recover-absent-validate-rows"
        | "recover-ownerless-validate-batch"
        | "classify-ownerless-validate-batch" => py_flag_present(rest, "--format"),
        _ => false,
    }
}

/// Translate exact Python-CLI argv to wrkslotsd argv (#562 P10).
/// `Ok(None)` keeps the native wrkslotsd surface unchanged; a covered
/// Python spelling returns the translated argv plus any `--wait-lock`
/// value for `WRKSLOTS_WAIT_LOCK`; unported spellings refuse by name.
fn translate_python_argv(args: Vec<String>) -> Result<PyCompatTranslation, String> {
    let routed = std::env::var_os("WRKSLOTS_PYCOMPAT").is_some();
    translate_python_argv_routed(args, routed)
}

fn translate_python_argv_routed(
    args: Vec<String>,
    routed: bool,
) -> Result<PyCompatTranslation, String> {
    let mut project_root: Option<String> = None;
    let mut machine: Option<String> = None;
    let mut wait_lock: Option<String> = None;
    let mut allow_existing = false;
    let mut userguide = false;
    let mut i = 1usize;
    while i < args.len() {
        let token = args[i].clone();
        let (slot, inline): (&str, Option<String>) =
            if let Some(v) = token.strip_prefix("--project-root=") {
                ("root", Some(v.to_string()))
            } else if token == "--project-root" {
                ("root", None)
            } else if let Some(v) = token.strip_prefix("--machine=") {
                ("machine", Some(v.to_string()))
            } else if token == "--machine" {
                ("machine", None)
            } else if let Some(v) = token.strip_prefix("--wait-lock=") {
                ("wait", Some(v.to_string()))
            } else if token == "--wait-lock" {
                ("wait", None)
            } else if token == "--allow-existing-unregistered-worktrees" {
                allow_existing = true;
                i += 1;
                continue;
            } else if token == "--userguide" {
                userguide = true;
                i += 1;
                continue;
            } else {
                break;
            };
        let value = match inline {
            Some(value) => {
                i += 1;
                value
            }
            None => {
                if i + 1 >= args.len() {
                    return Ok(None);
                }
                let value = args[i + 1].clone();
                i += 2;
                value
            }
        };
        match slot {
            "root" => project_root = Some(value),
            "machine" => machine = Some(value),
            _ => wait_lock = Some(value),
        }
    }
    let rest: Vec<String> = args[i..].to_vec();
    if !py_compat_triggered(&rest, i > 1, routed) {
        return Ok(None);
    }
    if userguide {
        return Err("--userguide stays with the Python edition in this slice".to_string());
    }
    if allow_existing {
        return Err(
            "--allow-existing-unregistered-worktrees stays with the Python edition in this slice"
                .to_string(),
        );
    }
    let Some(cmd) = rest.first().cloned() else {
        return Ok(None);
    };
    if PY_ONLY.contains(&cmd.as_str()) {
        return Err(format!(
            "'{cmd}' stays with the Python edition in this slice"
        ));
    }
    let renamed = PY_RENAMED
        .iter()
        .find(|(py, _)| *py == cmd)
        .map(|(_, rust)| (*rust).to_string());
    if renamed.is_none() && !PY_IDENTITY.contains(&cmd.as_str()) {
        return Ok(None);
    }
    let tail: Vec<String> = rest[1..].to_vec();
    let split = tail
        .iter()
        .position(|arg| arg == "--")
        .unwrap_or(tail.len());
    let (head, separator_tail) = tail.split_at(split);
    if cmd == "status" && py_flag_present(head, "--verbose") {
        return Err("status --verbose stays with the Python edition in this slice".to_string());
    }
    for flag in ["--image-ceiling-gib", "--image-state-ceiling-gib"] {
        if cmd == "create" && py_flag_present(head, flag) {
            return Err(format!(
                "create {flag} stays with the Python edition in this slice"
            ));
        }
    }
    let mut out: Vec<String> = vec![args[0].clone(), renamed.unwrap_or(cmd.clone())];
    out.extend(head.iter().cloned());
    if cmd != "quickstart" {
        if let Some(root) = project_root {
            out.push("--root".to_string());
            out.push(root);
        }
        if let Some(machine) = machine {
            out.push("--machine".to_string());
            out.push(machine);
        }
    }
    out.extend(separator_tail.iter().cloned());
    Ok(Some((out, wait_lock)))
}

/// Parse process arguments, print one JSON summary, and return a process code.
pub fn run() -> u8 {
    let argv: Vec<String> = std::env::args().collect();
    let command = match translate_python_argv(argv) {
        Ok(Some((translated, wait_lock))) => {
            if let Some(wait) = wait_lock {
                std::env::set_var("WRKSLOTS_WAIT_LOCK", wait);
            }
            match Cli::try_parse_from(translated) {
                Ok(cli) => cli.command,
                Err(error) => error.exit(),
            }
        }
        Ok(None) => Cli::parse().command,
        Err(error) => {
            eprintln!("wrkslotsd: {error}");
            return 1;
        }
    };
    let result: Result<serde_json::Value, crate::ObserverError> = match command {
        Command::Writers(inner) => match inner {
            Writers::Rebuild {
                events_dir,
                index,
                config,
                evidence,
            } => match (config, evidence) {
                (Some(config), Some(evidence)) => {
                    rebuild_policy_index(&events_dir, &index, &config, &evidence)
                }
                (None, None) => rebuild_index(&events_dir, &index),
                _ => unreachable!("clap enforces paired policy inputs"),
            }
            .and_then(|summary| encode_value(&CliSummary::from(&summary))),
            Writers::Status { index } => {
                read_index(&index).and_then(|summary| encode_value(&CliSummary::from(&summary)))
            }
            Writers::Explain { index, slot } => {
                read_decision(&index, &slot).and_then(|decision| encode_value(&decision))
            }
            Writers::ValidateConfig { root, machine } => {
                match crate::config_validate::load_config(root.as_deref(), machine.as_deref()) {
                    Ok(value) => {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&value).unwrap_or_default()
                        );
                        return 0;
                    }
                    Err(error) => {
                        eprintln!("wrkslotsd: {error}");
                        return 1;
                    }
                }
            }
            Writers::ParseState { root, machine } => {
                match crate::active_state::parse_state(root.as_deref(), machine.as_deref()) {
                    Ok(value) => {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&value).unwrap_or_default()
                        );
                        return 0;
                    }
                    Err(error) => {
                        eprintln!("wrkslotsd: {error}");
                        return 1;
                    }
                }
            }
            Writers::ParseArchive { root, machine } => {
                match crate::archive_state::parse_archive(root.as_deref(), machine.as_deref()) {
                    Ok(value) => {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&value).unwrap_or_default()
                        );
                        return 0;
                    }
                    Err(error) => {
                        eprintln!("wrkslotsd: {error}");
                        return 1;
                    }
                }
            }
            Writers::ParseHandoff {
                root,
                machine,
                slot,
            } => {
                match crate::handoff_state::parse_handoff(
                    root.as_deref(),
                    machine.as_deref(),
                    &slot,
                ) {
                    Ok(value) => {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&value).unwrap_or_default()
                        );
                        return 0;
                    }
                    Err(error) => {
                        eprintln!("wrkslotsd: {error}");
                        return 1;
                    }
                }
            }
            Writers::ParseHold {
                root,
                machine,
                slot,
            } => match crate::hold_state::parse_hold(root.as_deref(), machine.as_deref(), &slot) {
                Ok(value) => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&value).unwrap_or_default()
                    );
                    return 0;
                }
                Err(error) => {
                    eprintln!("wrkslotsd: {error}");
                    return 1;
                }
            },
            Writers::ParseJournal {
                root,
                machine,
                name,
            } => {
                match crate::journal_state::parse_journal(
                    root.as_deref(),
                    machine.as_deref(),
                    &name,
                ) {
                    Ok(value) => {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&value).unwrap_or_default()
                        );
                        return 0;
                    }
                    Err(error) => {
                        eprintln!("wrkslotsd: {error}");
                        return 1;
                    }
                }
            }
            Writers::ParseStates { root, machine } => {
                match crate::pair_state::parse_states(root.as_deref(), machine.as_deref()) {
                    Ok(value) => {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&value).unwrap_or_default()
                        );
                        return 0;
                    }
                    Err(error) => {
                        eprintln!("wrkslotsd: {error}");
                        return 1;
                    }
                }
            }
            Writers::ParseGlobal { root } => {
                match crate::global_state::parse_global(root.as_deref()) {
                    Ok(value) => {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&value).unwrap_or_default()
                        );
                        return 0;
                    }
                    Err(error) => {
                        eprintln!("wrkslotsd: {error}");
                        return 1;
                    }
                }
            }
            Writers::ParseHolds { root } => {
                match crate::record_holds::parse_holds(root.as_deref()) {
                    Ok(value) => {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&value).unwrap_or_default()
                        );
                        return 0;
                    }
                    Err(error) => {
                        eprintln!("wrkslotsd: {error}");
                        return 1;
                    }
                }
            }
            Writers::ParseJournals { root } => {
                match crate::journals_state::parse_journals(root.as_deref()) {
                    Ok(value) => {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&value).unwrap_or_default()
                        );
                        return 0;
                    }
                    Err(error) => {
                        eprintln!("wrkslotsd: {error}");
                        return 1;
                    }
                }
            }
            Writers::StatusText {
                root,
                machine,
                all_machines,
                slot,
                format,
            } => match crate::status_state::status_cli(
                root.as_deref(),
                machine.as_deref(),
                all_machines,
                slot.as_deref(),
                format == "human",
            ) {
                Ok(text) => {
                    print!("{text}");
                    return 0;
                }
                Err(error) => {
                    eprintln!("wrkslotsd: {error}");
                    return 1;
                }
            },
            Writers::Remove {
                slot,
                coordinator_pid,
                expected_generation,
                root,
                machine,
            } => {
                match crate::remove_write::remove_slot(
                    root.as_deref(),
                    machine.as_deref(),
                    &slot,
                    coordinator_pid,
                    expected_generation,
                ) {
                    Ok(value) => {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&value).unwrap_or_default()
                        );
                        return 0;
                    }
                    Err(error) => {
                        eprintln!("wrkslotsd: {error}");
                        return 1;
                    }
                }
            }
        },
        Command::Readers(inner) => match inner {
            Readers::DoctorText {
                root,
                machine,
                all_machines,
                format,
            } => match crate::doctor_state::doctor_cli(
                root.as_deref(),
                machine.as_deref(),
                all_machines,
                format == "human",
            ) {
                Ok(text) => {
                    print!("{text}");
                    return 0;
                }
                Err(error) => {
                    eprintln!("wrkslotsd: {error}");
                    return 1;
                }
            },
            Readers::AuditText {
                root,
                machine,
                cache_census_state,
                cache_work_limit,
                cache_wall_seconds,
                cache_plan_total_seconds,
                gate,
                all_machines,
                format,
            } => match crate::audit_state::audit_cli(
                root.as_deref(),
                machine.as_deref(),
                crate::audit_state::CensusRequest {
                    state: cache_census_state,
                    work_limit: cache_work_limit,
                    wall_seconds: cache_wall_seconds,
                    plan_total_seconds: cache_plan_total_seconds,
                },
                gate,
                format == "human",
                all_machines,
            ) {
                Ok((text, code)) => {
                    print!("{text}");
                    return u8::try_from(code).unwrap_or(1);
                }
                Err(error) => {
                    eprintln!("wrkslotsd: {error}");
                    return 1;
                }
            },
            Readers::ShellCommand {
                slot,
                slot_type,
                box_mode,
                name,
                writable,
                cwd,
                isolation,
                shell,
                format,
                root,
                machine,
                command,
            } => match crate::shell_command_state::shell_command_cli(
                root.as_deref(),
                machine.as_deref(),
                slot.as_deref(),
                &slot_type,
                box_mode,
                &name,
                writable.as_deref(),
                cwd.as_deref(),
                isolation.as_deref(),
                shell.as_deref(),
                format == "json",
                &command,
                &after_separator(),
            ) {
                Ok(text) => {
                    print!("{text}");
                    return 0;
                }
                Err(error) => {
                    eprintln!("wrkslotsd: {error}");
                    return 1;
                }
            },
            Readers::ParseStatus { root } => {
                match crate::status_state::parse_status(root.as_deref()) {
                    Ok(value) => {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&value).unwrap_or_default()
                        );
                        return 0;
                    }
                    Err(error) => {
                        eprintln!("wrkslotsd: {error}");
                        return 1;
                    }
                }
            }
            Readers::ParseAudit {
                root,
                machine,
                cache_census_state,
                cache_work_limit,
                cache_wall_seconds,
                cache_plan_total_seconds,
                all_machines,
            } => {
                match crate::audit_state::parse_audit(
                    root.as_deref(),
                    machine.as_deref(),
                    crate::audit_state::CensusRequest {
                        state: cache_census_state,
                        work_limit: cache_work_limit,
                        wall_seconds: cache_wall_seconds,
                        plan_total_seconds: cache_plan_total_seconds,
                    },
                    all_machines,
                ) {
                    Ok(value) => {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&value).unwrap_or_default()
                        );
                        return 0;
                    }
                    Err(error) => {
                        eprintln!("wrkslotsd: {error}");
                        return 1;
                    }
                }
            }
            Readers::Unpushed {
                root,
                machine,
                slot,
            } => match crate::unpushed_state::unpushed(
                root.as_deref(),
                machine.as_deref(),
                slot.as_deref(),
            ) {
                Ok(value) => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&value).unwrap_or_default()
                    );
                    return 0;
                }
                Err(error) => {
                    eprintln!("wrkslotsd: {error}");
                    return 1;
                }
            },
            Readers::UnpushedText {
                root,
                machine,
                slot,
                format,
            } => match crate::unpushed_state::unpushed_text(
                root.as_deref(),
                machine.as_deref(),
                slot.as_deref(),
                format == "json",
            ) {
                Ok(text) => {
                    print!("{text}");
                    return 0;
                }
                Err(error) => {
                    eprintln!("wrkslotsd: {error}");
                    return 1;
                }
            },
            Readers::ParseDoctor {
                root,
                machine,
                all_machines,
            } => match crate::doctor_state::parse_doctor(
                root.as_deref(),
                machine.as_deref(),
                all_machines,
            ) {
                Ok(value) => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&value).unwrap_or_default()
                    );
                    return 0;
                }
                Err(error) => {
                    eprintln!("wrkslotsd: {error}");
                    return 1;
                }
            },
            Readers::ParseStorage { root } => {
                match crate::storage_state::parse_storage(root.as_deref()) {
                    Ok(value) => {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&value).unwrap_or_default()
                        );
                        return 0;
                    }
                    Err(error) => {
                        eprintln!("wrkslotsd: {error}");
                        return 1;
                    }
                }
            }
            Readers::ParseStorageGit { root } => {
                match crate::git_storage_state::parse_storage_git(root.as_deref()) {
                    Ok(value) => {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&value).unwrap_or_default()
                        );
                        return 0;
                    }
                    Err(error) => {
                        eprintln!("wrkslotsd: {error}");
                        return 1;
                    }
                }
            }
            Readers::Hold {
                slot,
                reason,
                root,
                machine,
            } => {
                match crate::hold_write::hold_slot(
                    root.as_deref(),
                    machine.as_deref(),
                    &slot,
                    &reason,
                ) {
                    Ok(value) => {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&value).unwrap_or_default()
                        );
                        return 0;
                    }
                    Err(error) => {
                        eprintln!("wrkslotsd: {error}");
                        return 1;
                    }
                }
            }
            Readers::Unhold {
                slot,
                root,
                machine,
            } => match crate::hold_write::unhold_slot(root.as_deref(), machine.as_deref(), &slot) {
                Ok(value) => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&value).unwrap_or_default()
                    );
                    return 0;
                }
                Err(error) => {
                    eprintln!("wrkslotsd: {error}");
                    return 1;
                }
            },
            Readers::Heartbeat {
                slot,
                agent,
                owner_pid,
                expected_generation,
                root,
                machine,
            } => {
                match crate::heartbeat_write::heartbeat(
                    root.as_deref(),
                    machine.as_deref(),
                    &slot,
                    &agent,
                    owner_pid,
                    expected_generation,
                ) {
                    Ok(value) => {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&value).unwrap_or_default()
                        );
                        return 0;
                    }
                    Err(error) => {
                        eprintln!("wrkslotsd: {error}");
                        return 1;
                    }
                }
            }
            Readers::WriteHandoff {
                slot,
                agent,
                owner_pid,
                expected_generation,
                from_file,
                root,
                machine,
            } => {
                match crate::handoff_write::write_handoff(
                    root.as_deref(),
                    machine.as_deref(),
                    &slot,
                    &agent,
                    owner_pid,
                    expected_generation,
                    &from_file,
                ) {
                    Ok(value) => {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&value).unwrap_or_default()
                        );
                        return 0;
                    }
                    Err(error) => {
                        eprintln!("wrkslotsd: {error}");
                        return 1;
                    }
                }
            }
            Readers::Release {
                slot,
                agent,
                owner_pid,
                expected_generation,
                root,
                machine,
            } => {
                match crate::release_write::release_slot(
                    root.as_deref(),
                    machine.as_deref(),
                    &slot,
                    &agent,
                    owner_pid,
                    expected_generation,
                ) {
                    Ok(value) => {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&value).unwrap_or_default()
                        );
                        return 0;
                    }
                    Err(error) => {
                        eprintln!("wrkslotsd: {error}");
                        return 1;
                    }
                }
            }
        },
        Command::Recovery(inner) => match inner {
            Recovery::ReadHandoff {
                slot,
                coordinator_pid,
                coordinator_authorized,
                adopt_legacy_change,
                expected_generation,
                root,
                machine,
            } => {
                match crate::read_handoff_write::read_handoff(
                    root.as_deref(),
                    machine.as_deref(),
                    &slot,
                    coordinator_pid,
                    coordinator_authorized,
                    adopt_legacy_change,
                    expected_generation,
                ) {
                    Ok(value) => {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&value).unwrap_or_default()
                        );
                        return 0;
                    }
                    Err(error) => {
                        eprintln!("wrkslotsd: {error}");
                        return 1;
                    }
                }
            }
            Recovery::Adopt {
                slot,
                agent,
                owner_pid,
                expected_generation,
                task_scope_unit,
                task_scope_invocation_id,
                root,
                machine,
            } => {
                match crate::adopt_write::adopt_slot(
                    root.as_deref(),
                    machine.as_deref(),
                    &slot,
                    &agent,
                    owner_pid,
                    expected_generation,
                    task_scope_unit.as_deref(),
                    task_scope_invocation_id.as_deref(),
                ) {
                    Ok(value) => {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&value).unwrap_or_default()
                        );
                        return 0;
                    }
                    Err(error) => {
                        eprintln!("wrkslotsd: {error}");
                        return 1;
                    }
                }
            }
            Recovery::TransferOwner {
                slot,
                agent,
                owner_pid,
                expected_generation,
                reopen,
                coordinator_pid,
                coordinator_authorized,
                reason,
                root,
                machine,
            } => {
                match crate::transfer_write::transfer_owner(
                    root.as_deref(),
                    machine.as_deref(),
                    &slot,
                    &agent,
                    owner_pid,
                    expected_generation,
                    reopen,
                    coordinator_pid,
                    coordinator_authorized,
                    reason.as_deref(),
                ) {
                    Ok(value) => {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&value).unwrap_or_default()
                        );
                        return 0;
                    }
                    Err(error) => {
                        eprintln!("wrkslotsd: {error}");
                        return 1;
                    }
                }
            }
            Recovery::RecoverUnboundOwner {
                slot,
                coordinator_pid,
                expected_generation,
                recovery_note,
                validation,
                limitation,
                root,
                machine,
            } => {
                match crate::recover_unbound_write::recover_unbound_owner(
                    root.as_deref(),
                    machine.as_deref(),
                    &slot,
                    coordinator_pid,
                    expected_generation,
                    &recovery_note,
                    &validation,
                    &limitation,
                ) {
                    Ok(value) => {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&value).unwrap_or_default()
                        );
                        return 0;
                    }
                    Err(error) => {
                        eprintln!("wrkslotsd: {error}");
                        return 1;
                    }
                }
            }
            Recovery::RecoverAbsentAgentRow {
                slot,
                expected_generation,
                record_sha256,
                apply_mode,
                coordinator_authorized,
                coordinator_pid,
                root,
                machine,
            } => {
                match crate::recover_absent_write::recover_absent_agent_row(
                    root.as_deref(),
                    machine.as_deref(),
                    &slot,
                    expected_generation,
                    &record_sha256,
                    apply_mode,
                    coordinator_authorized,
                    coordinator_pid,
                ) {
                    Ok(value) => {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&value).unwrap_or_default()
                        );
                        return 0;
                    }
                    Err(error) => {
                        eprintln!("wrkslotsd: {error}");
                        return 1;
                    }
                }
            }
            Recovery::RecoverAbsentAgentRows {
                row,
                input,
                apply_mode,
                coordinator_authorized,
                coordinator_pid,
                root,
                machine,
            } => {
                match crate::recover_absent_write::recover_absent_agent_rows(
                    root.as_deref(),
                    machine.as_deref(),
                    &row,
                    input.as_deref(),
                    apply_mode,
                    coordinator_authorized,
                    coordinator_pid,
                ) {
                    Ok(value) => {
                        let refused = value
                            .get("counts")
                            .and_then(|counts| counts.get("refused"))
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(0);
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&value).unwrap_or_default()
                        );
                        return if refused > 0 { 1 } else { 0 };
                    }
                    Err(error) => {
                        eprintln!("wrkslotsd: {error}");
                        return 1;
                    }
                }
            }
            Recovery::RecoverAbsentValidateRows {
                input,
                apply_mode,
                coordinator_authorized,
                coordinator_pid,
                format,
                root,
                machine,
            } => {
                let (refusal, result) =
                    crate::recover_validate_write::recover_absent_validate_rows_wrapped(
                        root.as_deref(),
                        machine.as_deref(),
                        &input,
                        apply_mode,
                        coordinator_authorized,
                        coordinator_pid,
                    );
                match result {
                    Ok(value) => {
                        if format == "json" {
                            println!(
                                "{}",
                                serde_json::to_string_pretty(&value).unwrap_or_default()
                            );
                        } else if let Some(rows) =
                            value.get("rows").and_then(|rows| rows.as_array())
                        {
                            for row in rows {
                                println!(
                                    "ROW machine={} slot={} generation={} outcome={} detail={}",
                                    row.get("machine")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or_default(),
                                    row.get("slot").and_then(|v| v.as_str()).unwrap_or_default(),
                                    row.get("generation")
                                        .and_then(|v| v.as_i64())
                                        .unwrap_or_default(),
                                    row.get("outcome")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or_default(),
                                    serde_json::to_string(
                                        &row.get("detail").cloned().unwrap_or_default()
                                    )
                                    .unwrap_or_default(),
                                );
                            }
                        }
                        return 0;
                    }
                    Err(error) => {
                        if let Some(value) = refusal {
                            if format == "json" {
                                println!(
                                    "{}",
                                    serde_json::to_string_pretty(&value).unwrap_or_default()
                                );
                            } else if let Some(rows) =
                                value.get("rows").and_then(|rows| rows.as_array())
                            {
                                for row in rows {
                                    println!(
                                        "ROW machine={} slot={} generation={} outcome={} detail={}",
                                        row.get("machine")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default(),
                                        row.get("slot")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default(),
                                        row.get("generation")
                                            .and_then(|v| v.as_i64())
                                            .unwrap_or_default(),
                                        row.get("outcome")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default(),
                                        serde_json::to_string(
                                            &row.get("detail").cloned().unwrap_or_default()
                                        )
                                        .unwrap_or_default(),
                                    );
                                }
                            }
                        }
                        eprintln!("wrkslotsd: {error}");
                        return 1;
                    }
                }
            }
            Recovery::RecoverOwnerlessAgentWorktree {
                path,
                repository,
                head,
                branch,
                detached,
                remote,
                remote_url_sha256,
                handoff_sha256,
                apply_mode,
                coordinator_authorized,
                coordinator_pid,
                root,
                machine,
            } => {
                match crate::recover_ownerless_write::recover_ownerless_agent_worktree(
                    root.as_deref(),
                    machine.as_deref(),
                    &path,
                    &repository,
                    &head,
                    branch.as_deref(),
                    detached,
                    &remote,
                    &remote_url_sha256,
                    handoff_sha256.as_deref(),
                    apply_mode,
                    coordinator_authorized,
                    coordinator_pid,
                ) {
                    Ok(value) => {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&value).unwrap_or_default()
                        );
                        return 0;
                    }
                    Err(error) => {
                        eprintln!("wrkslotsd: {error}");
                        return 1;
                    }
                }
            }
            Recovery::RecoverOwnerlessAgentClone {
                path,
                remote_urls,
                handoff_sha256s,
                discard_ignored,
                apply_mode,
                coordinator_authorized,
                coordinator_pid,
                root,
                machine,
            } => {
                let parsed_urls: Vec<(String, String)> = remote_urls
                    .iter()
                    .map(|raw| {
                        let (key, url) = raw.split_once('=').unwrap_or((raw.as_str(), ""));
                        (key.to_string(), url.to_string())
                    })
                    .collect();
                match crate::recover_clone_write::recover_ownerless_agent_clone(
                    root.as_deref(),
                    machine.as_deref(),
                    &path,
                    &parsed_urls,
                    &handoff_sha256s,
                    discard_ignored,
                    apply_mode,
                    coordinator_authorized,
                    coordinator_pid,
                ) {
                    Ok(value) => {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&value).unwrap_or_default()
                        );
                        return 0;
                    }
                    Err(error) => {
                        eprintln!("wrkslotsd: {error}");
                        return 1;
                    }
                }
            }
            Recovery::RecoverOwnerlessAgentCache {
                apply_mode,
                coordinator_authorized,
                coordinator_pid,
                root,
                machine,
            } => {
                match crate::recover_cache_write::recover_ownerless_agent_cache(
                    root.as_deref(),
                    machine.as_deref(),
                    apply_mode,
                    coordinator_authorized,
                    coordinator_pid,
                ) {
                    Ok(value) => {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&value).unwrap_or_default()
                        );
                        return 0;
                    }
                    Err(error) => {
                        eprintln!("wrkslotsd: {error}");
                        return 1;
                    }
                }
            }
            Recovery::RecoverOwnerlessValidateBatch {
                checkouts,
                frozen_validate_checkouts,
                completed_records,
                repositories,
                validation_proof_manifests,
                coordinator_authorized,
                coordinator_pid,
                root,
                machine,
            } => {
                match crate::recover_cache_write::recover_ownerless_validate_batch(
                    root.as_deref(),
                    machine.as_deref(),
                    &checkouts,
                    &frozen_validate_checkouts,
                    &completed_records,
                    &repositories,
                    &validation_proof_manifests,
                    coordinator_authorized,
                    coordinator_pid,
                ) {
                    Ok(value) => {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&value).unwrap_or_default()
                        );
                        return 0;
                    }
                    Err(error) => {
                        eprintln!("wrkslotsd: {error}");
                        return 1;
                    }
                }
            }
            Recovery::ClassifyOwnerlessValidateBatch {
                checkouts,
                frozen_validate_checkouts,
                completed_records,
                repositories,
                validation_proof_manifests,
                root,
                machine,
            } => {
                match crate::recover_cache_write::classify_ownerless_validate_batch(
                    root.as_deref(),
                    machine.as_deref(),
                    &checkouts,
                    &frozen_validate_checkouts,
                    &completed_records,
                    &repositories,
                    &validation_proof_manifests,
                ) {
                    Ok(value) => {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&value).unwrap_or_default()
                        );
                        return 0;
                    }
                    Err(error) => {
                        eprintln!("wrkslotsd: {error}");
                        return 1;
                    }
                }
            }
            Recovery::RecoverAbsentAgentRowsText {
                row,
                input,
                apply_mode,
                coordinator_authorized,
                coordinator_pid,
                format,
                root,
                machine,
            } => match crate::recover_absent_write::recover_absent_agent_rows(
                root.as_deref(),
                machine.as_deref(),
                &row,
                input.as_deref(),
                apply_mode,
                coordinator_authorized,
                coordinator_pid,
            ) {
                Ok(value) => {
                    let (text, refused) = crate::batch_text::agent_rows(&value, format == "json");
                    print!("{text}");
                    return if refused { 1 } else { 0 };
                }
                Err(error) => {
                    eprintln!("wrkslotsd: {error}");
                    return 1;
                }
            },
            Recovery::RecoverAbsentValidateRowsText {
                input,
                apply_mode,
                coordinator_authorized,
                coordinator_pid,
                format,
                root,
                machine,
            } => {
                let (refusal, result) =
                    crate::recover_validate_write::recover_absent_validate_rows_wrapped(
                        root.as_deref(),
                        machine.as_deref(),
                        &input,
                        apply_mode,
                        coordinator_authorized,
                        coordinator_pid,
                    );
                match result {
                    Ok(value) => {
                        print!(
                            "{}",
                            crate::batch_text::validate_rows(&value, format == "json")
                        );
                        return 0;
                    }
                    Err(error) => {
                        if let Some(value) = refusal {
                            print!(
                                "{}",
                                crate::batch_text::validate_rows(&value, format == "json")
                            );
                        }
                        eprintln!("wrkslotsd: {error}");
                        return 1;
                    }
                }
            }
            Recovery::RecoverOwnerlessValidateBatchText {
                checkouts,
                frozen_validate_checkouts,
                completed_records,
                repositories,
                validation_proof_manifests,
                coordinator_authorized,
                coordinator_pid,
                format,
                root,
                machine,
            } => match crate::recover_cache_write::recover_ownerless_validate_batch(
                root.as_deref(),
                machine.as_deref(),
                &checkouts,
                &frozen_validate_checkouts,
                &completed_records,
                &repositories,
                &validation_proof_manifests,
                coordinator_authorized,
                coordinator_pid,
            ) {
                Ok(value) => {
                    print!(
                        "{}",
                        crate::batch_text::ownerless_recover(&value, format == "json")
                    );
                    return 0;
                }
                Err(error) => {
                    eprintln!("wrkslotsd: {error}");
                    return 1;
                }
            },
        },
        Command::Lifecycle(inner) => match inner {
            Lifecycle::Quickstart => {
                println!("{QUICKSTART}");
                return 0;
            }
            Lifecycle::RetirementQueue {
                format,
                root,
                machine,
            } => match crate::retirement_queue::retirement_queue(
                root.as_deref(),
                machine.as_deref(),
                format == "json",
            ) {
                Ok(text) => {
                    print!("{text}");
                    return 0;
                }
                Err(error) => {
                    eprintln!("wrkslotsd: {error}");
                    return 1;
                }
            },
            Lifecycle::Register {
                slot,
                slot_type,
                coordinator_authorized,
                agent,
                task,
                purpose,
                owner_pid,
                coordinator_pid,
                verified_live,
                repo,
                remote,
                remote_url,
                task_scope_unit,
                task_scope_invocation_id,
                root,
                machine,
            } => match crate::register_write::register_slot(
                root.as_deref(),
                machine.as_deref(),
                &slot,
                slot_type.as_deref(),
                coordinator_authorized,
                &agent,
                &task,
                &purpose,
                owner_pid,
                coordinator_pid,
                verified_live,
                &repo,
                &remote,
                &remote_url,
                task_scope_unit.as_deref(),
                task_scope_invocation_id.as_deref(),
            ) {
                Ok(text) => {
                    print!("{text}");
                    return 0;
                }
                Err(error) => {
                    eprintln!("wrkslotsd: {error}");
                    return 1;
                }
            },
            Lifecycle::SupersedeHandoff {
                slot,
                agent,
                owner_pid,
                expected_generation,
                coordinator_pid,
                coordinator_authorized,
                reason,
                root,
                machine,
            } => match crate::supersede_write::supersede_handoff(
                root.as_deref(),
                machine.as_deref(),
                &slot,
                &agent,
                owner_pid,
                expected_generation,
                coordinator_pid,
                coordinator_authorized,
                &reason,
            ) {
                Ok(text) => {
                    print!("{text}");
                    return 0;
                }
                Err(error) => {
                    eprintln!("wrkslotsd: {error}");
                    return 1;
                }
            },
            Lifecycle::Config { action } => match action {
                Some(ConfigCommand::Convert {
                    target,
                    root,
                    machine,
                }) => match crate::config_convert::config_convert(
                    root.as_deref(),
                    machine.as_deref(),
                    &target,
                ) {
                    Ok(text) => {
                        print!("{text}");
                        return 0;
                    }
                    Err(error) => {
                        eprintln!("wrkslotsd: {error}");
                        return 1;
                    }
                },
                None => {
                    let mut command = Cli::command();
                    if let Some(config) = command
                        .get_subcommands_mut()
                        .find(|sub| sub.get_name() == "config")
                    {
                        let _ = config.write_long_help(&mut std::io::stdout());
                        println!();
                    }
                    return 0;
                }
            },
            Lifecycle::Plan {
                index,
                target_bytes,
                limit,
            } => {
                read_pressure_plan(&index, target_bytes, limit).and_then(|plan| encode_value(&plan))
            }
            Lifecycle::ClassifyOwnerlessValidateBatchText {
                checkouts,
                frozen_validate_checkouts,
                completed_records,
                repositories,
                validation_proof_manifests,
                format,
                root,
                machine,
            } => match crate::recover_cache_write::classify_ownerless_validate_batch(
                root.as_deref(),
                machine.as_deref(),
                &checkouts,
                &frozen_validate_checkouts,
                &completed_records,
                &repositories,
                &validation_proof_manifests,
            ) {
                Ok(value) => {
                    print!(
                        "{}",
                        crate::batch_text::ownerless_classify(&value, format == "json")
                    );
                    return 0;
                }
                Err(error) => {
                    eprintln!("wrkslotsd: {error}");
                    return 1;
                }
            },
            Lifecycle::Freeze {
                action,
                reason,
                root,
                machine,
            } => match crate::freeze::freeze_command(
                root.as_deref(),
                machine.as_deref(),
                &action,
                &reason,
            ) {
                Ok(text) => {
                    print!("{text}");
                    return 0;
                }
                Err(error) => {
                    eprintln!("wrkslotsd: {error}");
                    return 1;
                }
            },
            Lifecycle::ClassifyCreateJournals {
                slot,
                agent,
                all_kinds,
                slot_type,
                format,
                all_machines,
                root,
                machine,
            } => match crate::classify_journals::classify_create_journals(
                root.as_deref(),
                machine.as_deref(),
                slot.as_deref(),
                &slot_type,
                agent.as_deref(),
                format == "json",
                all_machines,
                all_kinds,
            ) {
                Ok(text) => {
                    print!("{text}");
                    return 0;
                }
                Err(crate::classify_journals::ClassifyError::Blocked { first, payload }) => {
                    // Seal refusal (c4c4c64a): structured reasons on stdout
                    // in JSON mode; refusal exit 3 like the Python main.
                    if let Some(payload) = payload {
                        print!("{payload}");
                    }
                    eprintln!(
                        "wrkslotsd: interrupted validation-batch seal recorded in {first}; run 'wrkslots recover' first; list blocking journals with 'wrkslots classify-create-journals --all-kinds'"
                    );
                    return 3;
                }
                Err(crate::classify_journals::ClassifyError::Busy { kind, pid, journal }) => {
                    // Retryable busy (#992): exit 75 (TEMPFAIL), not a refusal.
                    if format == "json" {
                        println!(
                            "{{\"blocking\":true,\"busy\":{{\"journal\":{},\"kind\":{},\"pid\":{pid}}},\"status\":\"busy\"}}",
                            serde_json::to_string(&journal).unwrap_or_default(),
                            serde_json::to_string(&kind).unwrap_or_default(),
                        );
                    }
                    eprintln!("BUSY: busy: live '{kind}' mutation {journal} by pid {pid}; retry");
                    return 75;
                }
                Err(error) => {
                    eprintln!("wrkslotsd: {error}");
                    return 1;
                }
            },
            Lifecycle::ReapStrayWorktrees {
                repository,
                apply_mode,
                format,
                root,
                machine,
            } => match crate::reap_strays::reap_stray_worktrees(
                root.as_deref(),
                machine.as_deref(),
                &repository,
                apply_mode,
                format == "json",
            ) {
                Ok((text, retained_any)) => {
                    print!("{text}");
                    return if apply_mode && retained_any { 1 } else { 0 };
                }
                Err(error) => {
                    eprintln!("wrkslotsd: {error}");
                    return 1;
                }
            },
            Lifecycle::ReclaimOrphanState {
                apply_mode,
                format,
                root,
                machine,
            } => match crate::reclaim_state::reclaim_orphan_state(
                root.as_deref(),
                machine.as_deref(),
                apply_mode,
                format == "json",
            ) {
                Ok(text) => {
                    print!("{text}");
                    return 0;
                }
                Err(error) => {
                    eprintln!("wrkslotsd: {error}");
                    return 1;
                }
            },
            Lifecycle::Recover {
                coordinator_pid,
                slot,
                coordinator_authorized,
                discard_partial,
                discard_stale_image_copy,
                abort_import,
                legacy_validate_checkout,
                ownerless_validate_checkout,
                ownerless_validate_cargo_home,
                frozen_validate_checkout,
                completed_record,
                repository,
                recovery_note,
                validation_proof_manifest,
                root,
                machine,
            } => {
                match crate::recover_main_write::recover_main(
                    root.as_deref(),
                    machine.as_deref(),
                    coordinator_pid,
                    slot.as_deref(),
                    coordinator_authorized,
                    discard_partial,
                    discard_stale_image_copy,
                    abort_import,
                    legacy_validate_checkout.as_deref(),
                    ownerless_validate_checkout.as_deref(),
                    ownerless_validate_cargo_home.as_deref(),
                    frozen_validate_checkout.as_deref(),
                    completed_record.as_deref(),
                    repository.as_deref(),
                    recovery_note.as_deref(),
                    validation_proof_manifest.as_deref(),
                ) {
                    Ok(value) => {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&value).unwrap_or_default()
                        );
                        return 0;
                    }
                    Err(error) => {
                        eprintln!("wrkslotsd: {error}");
                        return 1;
                    }
                }
            }
            Lifecycle::Create {
                slot,
                slot_type,
                coordinator_authorized,
                agent,
                task,
                purpose,
                owner_pid,
                coordinator_pid,
                repo,
                remote,
                remote_url,
                branch,
                start,
                override_disk_floor,
                task_scope_unit,
                task_scope_invocation_id,
                representation,
                root,
                machine,
            } => {
                match crate::create_write::create_slot(
                    root.as_deref(),
                    machine.as_deref(),
                    &slot,
                    &slot_type,
                    coordinator_authorized,
                    &agent,
                    &task,
                    &purpose,
                    owner_pid,
                    coordinator_pid,
                    &repo,
                    &remote,
                    &remote_url,
                    &branch,
                    &start,
                    override_disk_floor,
                    &representation,
                    task_scope_unit.as_deref(),
                    task_scope_invocation_id.as_deref(),
                ) {
                    Ok(value) => {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&value).unwrap_or_default()
                        );
                        return 0;
                    }
                    Err(error) => {
                        eprintln!("wrkslotsd: {error}");
                        return 1;
                    }
                }
            }
            Lifecycle::CreateText {
                slot,
                slot_type,
                coordinator_authorized,
                agent,
                task,
                purpose,
                owner_pid,
                coordinator_pid,
                repo,
                remote,
                remote_url,
                branch,
                start,
                override_disk_floor,
                task_scope_unit,
                task_scope_invocation_id,
                representation,
                format,
                root,
                machine,
            } => match crate::create_text::create_text(
                root.as_deref(),
                machine.as_deref(),
                &slot,
                &slot_type,
                coordinator_authorized,
                &agent,
                &task,
                &purpose,
                owner_pid,
                coordinator_pid,
                &repo,
                &remote,
                &remote_url,
                &branch,
                &start,
                override_disk_floor,
                &representation,
                task_scope_unit.as_deref(),
                task_scope_invocation_id.as_deref(),
                format == "json",
            ) {
                Ok(text) => {
                    print!("{text}");
                    return 0;
                }
                Err(error) => {
                    eprintln!("wrkslotsd: {error}");
                    return 1;
                }
            },
            Lifecycle::Finish {
                slot,
                agent,
                owner_pid,
                expected_generation,
                validation,
                limitation,
                root,
                machine,
            } => {
                match crate::finish_write::finish_slot(
                    root.as_deref(),
                    machine.as_deref(),
                    &slot,
                    &agent,
                    owner_pid,
                    expected_generation,
                    &validation,
                    &limitation,
                ) {
                    Ok(value) => {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&value).unwrap_or_default()
                        );
                        return 0;
                    }
                    Err(error) => {
                        eprintln!("wrkslotsd: {error}");
                        return 1;
                    }
                }
            }
            Lifecycle::EnsureStates { root, machine } => {
                match crate::ensure_states::ensure_states(root.as_deref(), machine.as_deref()) {
                    Ok(value) => {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&value).unwrap_or_default()
                        );
                        return 0;
                    }
                    Err(error) => {
                        eprintln!("wrkslotsd: {error}");
                        return 1;
                    }
                }
            }
            Lifecycle::ParseConfig { config } => {
                let parsed = std::fs::read_to_string(&config)
                    .map_err(|error| {
                        crate::ObserverError::invalid(format!(
                            "cannot read config {}: {error}",
                            config.display()
                        ))
                    })
                    .and_then(|text| {
                        crate::yamlconfig::load_document(&text)
                            .map_err(|error| crate::ObserverError::invalid(error.to_string()))
                    });
                match parsed {
                    Ok((value, format)) => {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(
                                &serde_json::json!({"format": format, "value": value})
                            )
                            .unwrap_or_default()
                        );
                        return 0;
                    }
                    Err(error) => {
                        eprintln!("wrkslotsd: {error}");
                        return 1;
                    }
                }
            }
        },
    };
    match result {
        Ok(value) => {
            println!("{value}");
            0
        }
        Err(error) => {
            eprintln!("wrkslotsd: {error}");
            1
        }
    }
}

fn encode_value(value: &impl Serialize) -> Result<serde_json::Value, crate::ObserverError> {
    serde_json::to_value(value)
        .map_err(|error| crate::ObserverError::with_source("cannot encode command output", error))
}

#[cfg(test)]
mod py_compat_tests {
    use super::translate_python_argv_routed;

    fn argv(parts: &[&str]) -> Vec<String> {
        std::iter::once("wrkslotsd".to_string())
            .chain(parts.iter().map(|part| part.to_string()))
            .collect()
    }

    fn translated(parts: &[&str], routed: bool) -> Option<Vec<String>> {
        translate_python_argv_routed(argv(parts), routed)
            .unwrap()
            .map(|(out, _wait)| out)
    }

    #[test]
    fn globals_and_renamed_status_translate() {
        let out = translated(
            &[
                "--project-root",
                "/p",
                "--machine=node-a",
                "status",
                "--format",
                "json",
            ],
            false,
        )
        .unwrap();
        assert_eq!(
            out,
            argv(&[
                "status-text",
                "--format",
                "json",
                "--root",
                "/p",
                "--machine",
                "node-a"
            ])
        );
    }

    #[test]
    fn native_status_with_index_and_create_without_format_pass_through() {
        assert!(translated(&["status", "--index", "/tmp/i.db"], false).is_none());
        assert!(translated(&["create", "slot-b"], false).is_none());
        assert!(translated(&["parse-state"], false).is_none());
    }

    #[test]
    fn format_and_routed_create_rename() {
        assert_eq!(
            translated(&["create", "slot-b", "--format", "json"], false).unwrap()[1],
            "create-text"
        );
        assert_eq!(
            translated(&["create", "slot-b"], true).unwrap()[1],
            "create-text"
        );
        assert_eq!(translated(&["doctor"], false).unwrap()[1], "doctor-text");
    }

    #[test]
    fn unported_and_flag_corners_refuse_by_name() {
        let err = translate_python_argv_routed(argv(&["init", "."]), true).unwrap_err();
        assert!(err.contains("'init' stays with the Python edition"));
        let err = translate_python_argv_routed(argv(&["status", "--verbose"]), false).unwrap_err();
        assert!(err.contains("status --verbose"));
        let err = translate_python_argv_routed(
            argv(&[
                "create",
                "s",
                "--format",
                "json",
                "--image-ceiling-gib",
                "2",
            ]),
            false,
        )
        .unwrap_err();
        assert!(err.contains("--image-ceiling-gib"));
    }

    #[test]
    fn separator_tail_is_untouched_and_quickstart_drops_root() {
        let out = translated(
            &[
                "--machine",
                "node-a",
                "shell-command",
                "slot-b",
                "--",
                "echo",
                "--machine",
            ],
            false,
        )
        .unwrap();
        assert_eq!(
            out,
            argv(&[
                "shell-command",
                "slot-b",
                "--machine",
                "node-a",
                "--",
                "echo",
                "--machine"
            ])
        );
        let out = translated(&["--project-root", "/p", "quickstart"], false).unwrap();
        assert_eq!(out, argv(&["quickstart"]));
    }
}
