//! Command-line interface for rebuilding and reading the observer index.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
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

#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum Command {
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
    /// Diagnose every active record (owner process, heartbeat, hold) and print one status object per record
    ParseStatus {
        /// Project root containing .wrkslots.yml; discovered from the cwd when omitted
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
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
        /// Agent name already recorded on the slot
        #[arg(long, value_name = "NAME")]
        agent: String,
        /// Live successor PID to bind
        #[arg(long, value_name = "PID")]
        owner_pid: i64,
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

/// Parse process arguments, print one JSON summary, and return a process code.
pub fn run() -> u8 {
    let command = Cli::parse().command;
    let result: Result<serde_json::Value, crate::ObserverError> = match command {
        Command::Rebuild {
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
        Command::Status { index } => {
            read_index(&index).and_then(|summary| encode_value(&CliSummary::from(&summary)))
        }
        Command::Explain { index, slot } => {
            read_decision(&index, &slot).and_then(|decision| encode_value(&decision))
        }
        Command::Plan {
            index,
            target_bytes,
            limit,
        } => read_pressure_plan(&index, target_bytes, limit).and_then(|plan| encode_value(&plan)),
        Command::ValidateConfig { root, machine } => {
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
        Command::ParseState { root, machine } => {
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
        Command::ParseArchive { root, machine } => {
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
        Command::ParseHandoff {
            root,
            machine,
            slot,
        } => {
            match crate::handoff_state::parse_handoff(root.as_deref(), machine.as_deref(), &slot) {
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
        Command::ParseHold {
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
        Command::ParseJournal {
            root,
            machine,
            name,
        } => {
            match crate::journal_state::parse_journal(root.as_deref(), machine.as_deref(), &name) {
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
        Command::ParseStates { root, machine } => {
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
        Command::ParseGlobal { root } => match crate::global_state::parse_global(root.as_deref()) {
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
        Command::ParseHolds { root } => match crate::record_holds::parse_holds(root.as_deref()) {
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
        Command::ParseJournals { root } => {
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
        Command::ParseStatus { root } => match crate::status_state::parse_status(root.as_deref()) {
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
        Command::ParseDoctor {
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
        Command::ParseStorage { root } => {
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
        Command::ParseStorageGit { root } => {
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
        Command::Hold {
            slot,
            reason,
            root,
            machine,
        } => {
            match crate::hold_write::hold_slot(root.as_deref(), machine.as_deref(), &slot, &reason)
            {
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
        Command::Unhold {
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
        Command::Heartbeat {
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
        Command::WriteHandoff {
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
        Command::Remove {
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
        Command::Release {
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
        Command::ReadHandoff {
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
        Command::Adopt {
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
        Command::TransferOwner {
            slot,
            agent,
            owner_pid,
            expected_generation,
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
        Command::RecoverUnboundOwner {
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
        Command::RecoverAbsentAgentRow {
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
        Command::RecoverAbsentAgentRows {
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
        Command::RecoverAbsentValidateRows {
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
                    } else if let Some(rows) = value.get("rows").and_then(|rows| rows.as_array()) {
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
                    }
                    eprintln!("wrkslotsd: {error}");
                    return 1;
                }
            }
        }
        Command::RecoverOwnerlessAgentWorktree {
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
        Command::RecoverOwnerlessAgentClone {
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
        Command::RecoverOwnerlessAgentCache {
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
        Command::RecoverOwnerlessValidateBatch {
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
        Command::ClassifyOwnerlessValidateBatch {
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
        Command::Recover {
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
        Command::Create {
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
        Command::Finish {
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
        Command::EnsureStates { root, machine } => {
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
        Command::ParseConfig { config } => {
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
