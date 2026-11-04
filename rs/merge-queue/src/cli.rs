//! Command-line interface.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::git::{CombineMode, Git};
use crate::model::ItemStatus;
use crate::ops::{self, Submission};
use crate::plan::{render, short};
use crate::run::{self, RunOptions, RunReport, Runner};
use crate::serve::{self, ServeOptions};
use crate::state::{
    ClassRule, Config, GitHubConfig, IdentityConfig, LandConfig, NotifyConfig, PriorityConfig,
    RegenerateConfig, State, ValidatorConfig, DEFAULT_DEPTH, DEFAULT_MAX_BATCH, DEFAULT_PRIORITY,
};
use crate::tests::{parse_selection, ValidationClaim};

const AFTER_HELP: &str = "\
Start with `merge-queue quickstart`; the full reference is `merge-queue userguide`.

Examples:
  merge-queue init --remote origin --base main
  merge-queue submit 42 --head feature/x --tested-with 'rust.planner.test' --policy manual
  merge-queue plan
  merge-queue plan --json > plan.json
  merge-queue run --dry-run
  merge-queue run --depth 2
  merge-queue widen
  merge-queue serve
  merge-queue serve --shadow
  merge-queue status
  merge-queue donor run --queue 'ssh qhost merge-queue --repo /srv/q' --clone ~/q --name h2 --jobs 2
  merge-queue donor status";

/// Local merge queue: order, batch, validate and land ready changes while keeping the base green.
#[derive(Debug, Parser)]
#[command(
    name = "merge-queue",
    version = crate::build_info::version_text(),
    after_help = AFTER_HELP
)]
struct Cli {
    /// State directory. Default: $MERGE_QUEUE_STATE, else `merge-queue/` inside the Git common
    /// directory of --repo (or of the current directory), so every worktree shares one queue.
    #[arg(long, global = true, value_name = "DIR")]
    state: Option<PathBuf>,
    /// Repository the queue operates on (a clone; bare is fine). Default: the current directory.
    #[arg(long, global = true, value_name = "DIR")]
    repo: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

/// `merge-queue donor` verbs.
#[derive(Debug, Subcommand)]
enum DonorCommand {
    /// Run a donor: claim jobs from the queue through --queue, validate each exact candidate
    /// in a checkout of --clone, and report receipts. Stops on SIGINT/SIGTERM, giving its jobs
    /// back.
    Run {
        /// Command that runs the queue's merge-queue on the queue host, to which each request
        /// is appended; shell words, for example
        /// 'agentcloudctl node exec --node HOST -- /path/merge-queue --repo /path/queue'
        /// (or 'ssh HOST merge-queue --repo ...', or a local merge-queue path).
        #[arg(long, value_name = "COMMAND")]
        queue: String,
        /// This host's clone of the queue's repository (candidate checkouts are worktrees of it).
        #[arg(long, value_name = "DIR")]
        clone: PathBuf,
        /// Remote of --clone to fetch the base from before asking for a bundle ("" = never fetch).
        #[arg(long, default_value = "origin")]
        remote: String,
        /// Donor name, unique among the queue's donors.
        #[arg(long)]
        name: String,
        /// Most jobs this donor runs at once.
        #[arg(long, default_value_t = 1)]
        jobs: usize,
        /// Seconds between claims while idle.
        #[arg(long, default_value_t = 5)]
        poll_secs: u64,
        /// Work directory for checkouts and logs (default: merge-queue-donor/ inside --clone's Git directory).
        #[arg(long, value_name = "DIR")]
        work: Option<PathBuf>,
        /// Seconds to stop claiming after declining a job because this host's tools differ.
        #[arg(long, default_value_t = 300)]
        mismatch_backoff_secs: u64,
        /// Exit once no job is running and none is available (scripts and tests).
        #[arg(long)]
        until_idle: bool,
    },
    /// Queue host: lease the oldest job this donor may take; prints `{"job": ...}` (the job
    /// carries the lease token) or `{"job": null}`.
    Claim {
        /// Donor name.
        #[arg(long)]
        donor: String,
        /// Donor host.
        #[arg(long, default_value = "")]
        host: String,
        /// Most leases this donor holds at once.
        #[arg(long, default_value_t = 1)]
        jobs: usize,
    },
    /// Queue host: extend a lease (token on stdin); prints continue or stop.
    Renew {
        /// Job id.
        #[arg(long)]
        job: String,
    },
    /// Queue host: write the candidate's git bundle to stdout (token on stdin).
    Bundle {
        /// Job id.
        #[arg(long)]
        job: String,
    },
    /// Queue host: submit a receipt (`{"token", "receipt"}` JSON on stdin); exits 1 with the
    /// reason when it is refused.
    Report {
        /// Job id.
        #[arg(long)]
        job: String,
    },
    /// Queue host: give a job back (token on stdin); it is queued again.
    Release {
        /// Job id.
        #[arg(long)]
        job: String,
        /// Why.
        #[arg(long, default_value = "released")]
        reason: String,
        /// Never hand this job to this donor again (it cannot run it).
        #[arg(long)]
        decline: bool,
    },
    /// Queue host: print what a donor checks before claiming, as JSON: the URL of the queue
    /// repository's remote and this merge-queue's revision (`donor run` asks for it).
    Info,
    /// Exit 0 when two repository URLs name the same repository (`https` and `ssh` forms,
    /// with or without `.git`), 1 otherwise; prints both normalized forms. For a validator
    /// that checks its checkout's origin.
    SameRepository {
        /// One URL.
        a: String,
        /// The other.
        b: String,
    },
    /// List donor jobs, leases (never their tokens) and compare-mode verdicts.
    Status {
        /// Print JSON.
        #[arg(long)]
        json: bool,
    },
}

/// The lease token a donor sends on stdin.
fn read_token() -> Result<String, String> {
    let mut text = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)
        .map_err(|error| format!("cannot read the lease token from stdin: {error}"))?;
    let token = text.trim().to_owned();
    if token.is_empty() {
        return Err("no lease token on stdin".to_owned());
    }
    Ok(token)
}

fn donor_store(state: &State, config: &Config) -> crate::donor::Store {
    let mut store = crate::donor::Store::new(&state.root);
    store.lease_secs = config.donors.lease();
    store.could_not_run_limit = config.donors.could_not_run_limit();
    store
}

fn donor_command(state: &State, action: DonorCommand) -> Result<i32, String> {
    let config = state.config()?;
    let store = donor_store(state, &config);
    match action {
        DonorCommand::Run { .. } => Err("donor run is handled before the queue state".to_owned()),
        DonorCommand::SameRepository { .. } => {
            Err("donor same-repository is handled before the queue state".to_owned())
        }
        DonorCommand::Info => {
            let info = crate::donor::QueueInfo {
                remote_url: crate::donor::remote_url(&config.repo, &config.remote),
                merge_queue: crate::build_info::revision(),
            };
            println!(
                "{}",
                serde_json::to_string(&info).map_err(|error| error.to_string())?
            );
            Ok(0)
        }
        DonorCommand::Claim { donor, host, jobs } => {
            let cap = config.donors.max_jobs_per_donor;
            let jobs = if cap > 0 { jobs.min(cap) } else { jobs };
            let job = store.claim(&donor, &host, jobs)?;
            let text = serde_json::to_string(&crate::donor::Claimed { job })
                .map_err(|error| error.to_string())?;
            println!("{text}");
            Ok(0)
        }
        DonorCommand::Renew { job } => {
            let renewal = store.renew(&job, &read_token()?)?;
            println!(
                "{}",
                serde_json::to_string(&renewal).map_err(|error| error.to_string())?
            );
            Ok(0)
        }
        DonorCommand::Bundle { job } => {
            let record = store.for_bundle(&job, &read_token()?)?;
            let stdout = std::io::stdout();
            let mut out = stdout.lock();
            crate::donor::write_bundle(
                &config.repo,
                &state.root.join("donor").join("scratch"),
                &record,
                config.donors.max_bundle(),
                &mut out,
            )?;
            Ok(0)
        }
        DonorCommand::Report { job } => {
            let mut text = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)
                .map_err(|error| format!("cannot read the report from stdin: {error}"))?;
            let report: crate::donor::Report = serde_json::from_str(&text)
                .map_err(|error| format!("unreadable report: {error}"))?;
            store.report(&job, &report.token, report.receipt)?;
            println!("{{\"accepted\": true}}");
            Ok(0)
        }
        DonorCommand::Release {
            job,
            reason,
            decline,
        } => {
            store.release(&job, &read_token()?, &reason, decline)?;
            Ok(0)
        }
        DonorCommand::Status { json } => {
            let jobs = store.jobs()?;
            if json {
                let view: Vec<serde_json::Value> =
                    jobs.iter().map(crate::donor::redacted).collect();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&view).map_err(|error| error.to_string())?
                );
                return Ok(0);
            }
            let count =
                |state: crate::donor::JobState| jobs.iter().filter(|j| j.state == state).count();
            let revision = crate::build_info::revision();
            println!(
                "merge-queue here: {}",
                match crate::build_info::unusable(revision.as_ref()) {
                    None => revision.as_ref().map_or_else(String::new, |r| r.describe()),
                    Some(problem) => format!("unusable for donors: {problem}"),
                }
            );
            println!(
                "donor jobs: {} queued, {} leased, {} done, {} cancelled",
                count(crate::donor::JobState::Queued),
                count(crate::donor::JobState::Leased),
                count(crate::donor::JobState::Done),
                count(crate::donor::JobState::Cancelled)
            );
            let could_not_run: usize = jobs.iter().map(|j| j.could_not_run.len()).sum();
            if could_not_run > 0 {
                // Receipts that said validation never ran: no verdict, no comparison.
                println!("could not run: {could_not_run} receipt(s), each queued again");
            }
            let compared: Vec<&String> =
                jobs.iter().filter_map(|j| j.comparison.as_ref()).collect();
            if !compared.is_empty() {
                let matched = compared.iter().filter(|c| c.as_str() == "match").count();
                println!(
                    "compare: {matched} of {} receipts match the local verdict",
                    compared.len()
                );
                // Receipts that arrived after the candidate landed or was superseded: the
                // donor is slower than the queue host.
                let late = jobs.iter().filter(|j| j.compared_after_landing()).count();
                println!("compared after landing: {late} of {}", compared.len());
            }
            for job in &jobs {
                let who = match (&job.lease, &job.receipt) {
                    (Some(lease), _) => format!(" leased to {} on {}", lease.donor, lease.host),
                    (None, Some(receipt)) => {
                        format!(" by {} on {}", receipt.donor.name, receipt.donor.host)
                    }
                    _ => String::new(),
                };
                println!(
                    "  {} {:?} {}{}{}{}",
                    job.id,
                    job.state,
                    &job.commit[..job.commit.len().min(12)],
                    who,
                    if job.attempts > 1 {
                        format!(", attempt {}", job.attempts)
                    } else {
                        String::new()
                    },
                    job.comparison
                        .as_ref()
                        .map(|c| format!(", {c}"))
                        .unwrap_or_default()
                );
            }
            Ok(0)
        }
    }
}

// `init` carries many flags; the enum is parsed once per process, so its size is irrelevant.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Subcommand)]
enum Command {
    /// Create the state directory and write its config.json.
    Init {
        /// Remote holding the base branch and change heads; empty for a local-only repository.
        #[arg(long, default_value = "origin")]
        remote: String,
        /// Base branch the queue lands on.
        #[arg(long, default_value = "main")]
        base: String,
        /// How changes are combined: `rebase` (replay commits, linear) or `merge`.
        #[arg(long, default_value = "rebase", value_parser = parse_combine)]
        combine: CombineMode,
        /// Maximum changes per batch.
        #[arg(long, default_value_t = DEFAULT_MAX_BATCH)]
        max_batch: usize,
        /// Batches validated concurrently, each on top of the one before.
        #[arg(long, default_value_t = DEFAULT_DEPTH)]
        depth: usize,
        /// Largest adaptive window: +1 per landed batch, halved per red batch. 0 = fixed at
        /// --depth.
        #[arg(long, default_value_t = 0)]
        max_depth: usize,
        /// Path pattern merged with Git's union driver (append-only line lists only).
        /// Repeatable.
        #[arg(long, value_name = "PATTERN")]
        union_path: Vec<String>,
        /// Label that makes a pull request a priority change when `sync` reads it: planned
        /// ahead of all other work and validated in a batch of its own (it still lands only on
        /// its own exact green). Compared without regard to case; `-` turns it off.
        #[arg(long, value_name = "LABEL", default_value = crate::state::DEFAULT_PRIORITY_LABEL)]
        priority_label: String,
        /// Path pattern of CI or validation infrastructure (exact path, directory, or `*`
        /// wildcards). `plan` suggests the priority label for a change touching only these
        /// paths; the queue never applies it. Repeatable.
        #[arg(long, value_name = "PATTERN")]
        ci_path: Vec<String>,
        /// Test-footprint policy file in the repository (version 2, such as
        /// `ci/test-footprints.json`), read at each plan's base: decision-log features then use
        /// its components and verdict (the shared change classifier). Default: none, and
        /// components are first directories.
        #[arg(long, value_name = "PATH", default_value = "")]
        classifier_policy: String,
        /// Generated-file path pattern the queue regenerates instead of treating its conflicts
        /// as real (exact path, directory with or without a trailing slash, or `*` wildcards).
        /// Repeatable.
        #[arg(long, value_name = "PATTERN")]
        regen_path: Vec<String>,
        /// Shell command, run as `sh -c` at the root of the candidate checkout, that
        /// rewrites every generated file deterministically. Runs with `MQ_PR`,
        /// `MQ_REGEN_PATHS` and `MQ_CONFLICTED` set.
        #[arg(long, default_value = "")]
        regen_command: String,
        /// Optional command run before --regen-command, such as building the generator; its
        /// failure ejects the change as `regen-build-failed` with the error tail.
        #[arg(long, default_value = "")]
        regen_build: String,
        /// When to regenerate: `conflict` (only when conflicts are confined to generated files)
        /// or `always` (after every change).
        #[arg(long, default_value = "conflict")]
        regen_when: String,
        /// Exact-commit rule: never carry a result to another commit; a tree lands only with
        /// green coverage recorded at that exact commit.
        #[arg(long)]
        exact_sha: bool,
        /// Treat the base as green in culprit searches (skip probing it). Sound when only this
        /// queue lands, always exactly validated commits.
        #[arg(long)]
        trust_base: bool,
        /// Failure class PATTERN=CLASS: failed tests whose ID (or step) matches PATTERN (exact or
        /// `*` wildcards) are labelled CLASS in ejections and reports. Repeatable.
        #[arg(long, value_name = "PATTERN=CLASS")]
        classify: Vec<String>,
        /// Name for commits the queue creates (default: Git's configuration).
        #[arg(long, default_value = "")]
        identity_name: String,
        /// Email for commits the queue creates (default: Git's configuration).
        #[arg(long, default_value = "")]
        identity_email: String,
        /// Peer queue NAME=DIR: another repository's merge-queue state directory, whose changes
        /// this queue's changes may require. Repeatable.
        #[arg(long, value_name = "NAME=DIR")]
        peer: Vec<String>,
        /// Hosting repository `owner/name`, for `sync --label/--all-open` and `--close-prs`.
        #[arg(long, default_value = "")]
        github_repo: String,
        /// Command (and leading arguments) to run instead of `gh`, e.g. a pacing wrapper.
        /// Repeatable. When unset, `FLEET_REAL_GH` (shell-style words) is used as a
        /// fallback on fleet hosts; an explicit `--gh` always wins.
        #[arg(long, value_name = "WORD", allow_hyphen_values = true)]
        gh: Vec<String>,
        /// API requests allowed per rolling hour across this queue's calls (0 = 60).
        #[arg(long, default_value_t = 0)]
        api_per_hour: u32,
        /// Budget file shared by every queue on the host (default: in the state directory).
        #[arg(long, default_value = "")]
        api_budget_file: String,
        /// Fetch without credentials (public repositories): no credential use per fetch.
        #[arg(long)]
        anonymous_fetch: bool,
        /// Require an approving review at the current head for `sync --label` /
        /// `sync --all-open` (reviews read in the same batched list call): an
        /// unapproved change is not submitted, and a queued change whose head
        /// moves without a fresh approval is withdrawn and its submitter
        /// notified. Off by default.
        #[arg(long)]
        require_approval_at_head: bool,
        /// Recount the generated count pins after merging each batch
        /// (base plus each change's own delta) instead of treating their
        /// conflicts as real. Off by default.
        #[arg(long)]
        recount_pins: bool,
        /// After landing, close each landed pull request with a pointer to the landed commit
        /// (budgeted; postponed closes are retried by `sync`).
        #[arg(long)]
        close_prs: bool,
        /// Command prefix for network Git operations, e.g. `--net-wrapper with-proxy`.
        /// Repeat for multiple words.
        #[arg(long, value_name = "WORD", allow_hyphen_values = true)]
        net_wrapper: Vec<String>,
        /// Validator kind: `command` (shell template with a JSON-lines results file) or `dagrun`.
        #[arg(long, default_value = "")]
        validator: String,
        /// For `command`: shell template that runs the selection (see the user guide).
        #[arg(long, default_value = "")]
        validator_run: String,
        /// Shell command printing the selection policy's test IDs for HEAD relative to $MQ_BASE.
        /// Empty: every validation runs the complete suite.
        #[arg(long, default_value = "")]
        select: String,
        /// Only output lines with this prefix are test IDs (e.g. `STEP `).
        #[arg(long, default_value = "")]
        id_prefix: String,
        /// Name recorded for the selection policy.
        #[arg(long, default_value = "")]
        policy: String,
        /// For `dagrun`: the DAG file, relative to the candidate checkout.
        #[arg(long, default_value = "")]
        dag: String,
        /// For `dagrun`: the dagrun executable (default `dagrun` on PATH).
        #[arg(long, default_value = "")]
        dagrun: String,
        /// Command prefix every validation runs under (host admission control). Repeatable.
        #[arg(long, value_name = "WORD", allow_hyphen_values = true)]
        admission: Vec<String>,
        /// Seconds before one validation is killed and counted as errors (0 = no limit).
        #[arg(long, default_value_t = 0)]
        timeout: u64,
        /// Extra attempts after an admission refusal (exit 75 / COULD_NOT_RUN) within one
        /// run, each after a backoff delay. A refusal is never a pass and never red: the
        /// change stays queued.
        #[arg(long, default_value_t = crate::state::DEFAULT_REFUSAL_MAX_RETRIES)]
        refusal_max_retries: usize,
        /// Base backoff in seconds between refusal retries (doubled per attempt, capped
        /// at 60). 0 = no delay.
        #[arg(long, default_value_t = crate::state::DEFAULT_REFUSAL_BACKOFF_SECS)]
        refusal_backoff_secs: u64,
        /// Seconds a GREEN_FLAKY batch (green only because some tests passed after a
        /// retry inside the run) is held for a ruling before it is re-queued.
        #[arg(long, default_value_t = crate::state::DEFAULT_GREEN_FLAKY_HOLD_SECS)]
        green_flaky_hold_secs: u64,
        /// Times a change may be deferred because validation did not run (refusals, a
        /// validator that cannot execute, a probe or donors that could not run) before it is
        /// escalated to its owner and held until `merge-queue retry`.
        #[arg(long, default_value_t = crate::state::DEFAULT_RETRY_BOUND)]
        max_deferrals: u32,
        /// A red validation naming failed tests whose follow-up probe cannot run: `eject`
        /// the change (the red run is the verdict) or `hold` it for its owner.
        #[arg(long, default_value = "eject", value_parser = ["eject", "hold"])]
        red_without_probe: String,
        /// Where validations run: `off` (in this process; the default), `compare` (in this
        /// process, which decides, and also on donors, whose receipts are only compared), or
        /// `decide` (on donors only; an accepted receipt is the verdict).
        #[arg(long, default_value = "off", value_parser = ["off", "compare", "decide"])]
        offload: String,
        /// A tool the validation depends on, as NAME=COMMAND (repeatable): the command prints
        /// its identity (a tool repository's commit, a toolchain version; never a hash of a
        /// binary, since builds differ byte for byte across hosts). A donor whose answer
        /// differs from the queue host's refuses the job. merge-queue's own revision (the
        /// second line of --version) is always compared.
        #[arg(long, value_name = "NAME=COMMAND")]
        tool_identity: Vec<String>,
        /// Delete ignored files (build caches) from reused checkouts before each validation.
        #[arg(long)]
        clean_ignored: bool,
        /// Land driver: `push` (fast-forward the base to the validated commit) or `command`.
        #[arg(long, default_value = "push")]
        land: String,
        /// For `--land command`: shell template run once per change (see the user guide).
        #[arg(long, default_value = "")]
        land_command: String,
        /// For `--land command`: allow batches of several changes (their intermediate trees
        /// reach the base without being validated on their own).
        #[arg(long)]
        allow_batches: bool,
        /// Command (and leading arguments) the `serve` daemon runs instead of `agentctl`
        /// to push state-change notifications, e.g. a wrapper or a test fake. Repeatable.
        /// The daemon runs `<command> send <name> --deliver-while-working --file <report>`.
        #[arg(long, value_name = "WORD", allow_hyphen_values = true)]
        agentctl: Vec<String>,
        /// Overwrite an existing config.json (items and history are kept).
        #[arg(long)]
        force: bool,
    },
    /// Submit (or resubmit) a change as ready to land.
    Submit {
        /// Change number (pull-request number).
        pr: i64,
        /// Head branch on the remote, or a full ref such as `refs/pull/42/head`.
        #[arg(long)]
        head: String,
        /// Exact head commit (or prefix) you mean; refused if the branch is elsewhere.
        #[arg(long, value_name = "SHA", default_value = "")]
        sha: String,
        /// Lower lands first.
        #[arg(long, default_value_t = DEFAULT_PRIORITY)]
        priority: i64,
        /// Change number that must land first (repeatable).
        #[arg(long, value_name = "PR")]
        after: Vec<i64>,
        /// Validate and land this change in a batch of its own.
        #[arg(long)]
        solo: bool,
        /// A priority change (as the priority label does for `sync`): planned ahead of all
        /// other work and validated in a batch of its own. It still lands only on its own exact
        /// green.
        #[arg(long)]
        prioritized: bool,
        /// Who is submitting (agent or person).
        #[arg(long, default_value = "")]
        by: String,
        /// One-line description.
        #[arg(long, default_value = "")]
        title: String,
        /// Tests you ran on this head: `all`, a comma-separated list of IDs (`step` or
        /// `step::case`), or `@FILE` with one ID per line. Recorded; focused claims are visible.
        #[arg(long, value_name = "SELECTION")]
        tested_with: Option<String>,
        /// A peer commit that must land first: `PEER#PR` (that peer change, which then lands as
        /// its own commits) or `PEER@SHA`. Repeatable.
        #[arg(long, value_name = "SPEC")]
        requires: Vec<String>,
        /// Name of the policy that chose --tested-with (e.g. `path-routing`, `manual`).
        #[arg(long, default_value = "manual")]
        policy: String,
        /// agentctl session name the `serve` daemon notifies exactly once per state
        /// change (landed, ejected, refusal-deferred, withdrawn). Stored on the item
        /// next to --by; resubmitting without it keeps the recorded name.
        #[arg(long, value_name = "NAME", default_value = "")]
        notify: String,
    },
    /// Reconcile the queue with a ready source: a label, every open pull request, or a file.
    Sync {
        /// Ready = open pull requests with this label (one batched API call).
        #[arg(long, conflicts_with_all = ["all_open", "file"])]
        label: Option<String>,
        /// Ready = every open, non-draft pull request targeting the base (one batched API call).
        #[arg(long, conflicts_with = "file")]
        all_open: bool,
        /// Ready = the changes listed in FILE, one per line (no API call); see the user guide.
        #[arg(long, value_name = "FILE")]
        file: Option<PathBuf>,
        /// Most pull requests to list (each 100 costs one API request).
        #[arg(long, default_value_t = 300)]
        limit: u32,
        /// Report what would change without submitting or withdrawing anything.
        #[arg(long)]
        dry_run: bool,
        /// Print JSON.
        #[arg(long)]
        json: bool,
    },
    /// Withdraw a queued change.
    Withdraw {
        /// Change number.
        pr: i64,
        /// Reason recorded in the change's history.
        #[arg(long, default_value = "withdrawn by submitter")]
        reason: String,
    },
    /// Return changes escalated to their owner (validation did not run the bounded number of
    /// times) to the queue: clears the escalation, the refusal count and the backoff. Run it
    /// once whatever kept validation from running is fixed.
    Retry {
        /// Change numbers.
        #[arg(required = true)]
        prs: Vec<i64>,
    },
    /// List changes and their status.
    List {
        /// Include landed, ejected and withdrawn changes.
        #[arg(long)]
        all: bool,
        /// Print JSON records instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Order and chunk queued changes into batches against the current base (read-only).
    Plan {
        /// Override the configured maximum batch size.
        #[arg(long)]
        max_batch: Option<usize>,
        /// Print the plan as JSON (schema merge-queue-plan/v1).
        #[arg(long)]
        json: bool,
    },
    /// Build, validate and land the planned batches; bisect and eject culprits.
    Run {
        /// Build candidates and compute selections only: no validation, no landing.
        #[arg(long)]
        dry_run: bool,
        /// Validate and bisect for real, but land on a private simulated base and leave every
        /// item's status unchanged.
        #[arg(long)]
        shadow: bool,
        /// Override the configured depth (initial window and culprit-search probe width).
        #[arg(long)]
        depth: Option<usize>,
        /// Override the configured largest adaptive window.
        #[arg(long)]
        max_depth: Option<usize>,
        /// Override the configured maximum batch size.
        #[arg(long)]
        max_batch: Option<usize>,
        /// Stop after this many batches (0 = drain the plan).
        #[arg(long, default_value_t = 0)]
        max_batches: usize,
        /// After draining, run the complete suite once to discharge widening obligations.
        #[arg(long)]
        widen: bool,
        /// First drain the peer queues holding changes this queue's changes require.
        #[arg(long)]
        with_peers: bool,
        /// Print the run report as JSON instead of Markdown.
        #[arg(long)]
        json: bool,
        /// Override `validator.offload` for this run: `off`, `compare` (donors validate too;
        /// their receipts are only compared with the local verdict) or `decide` (donors only).
        #[arg(long, value_parser = ["off", "compare", "decide"])]
        offload: Option<String>,
    },
    /// Run the queue forever: watch for submitted changes and a moving base, land batches.
    Serve {
        /// Idle poll interval in milliseconds (items watch + heartbeat cadence).
        #[arg(long, default_value_t = serve::DEFAULT_POLL_MS)]
        poll_ms: u64,
        /// Seconds between `git ls-remote` base polls.
        #[arg(long, default_value_t = serve::DEFAULT_BASE_POLL_SECS)]
        base_poll_secs: u64,
        /// Run at most one work cycle, then exit (scripts and tests).
        #[arg(long)]
        once: bool,
        /// Validate for real, but land on a simulated base (same shadow path as
        /// `run --shadow`): full plan/run/notify cycles execute, landings are
        /// recorded as shadow/simulated, item statuses stay queued, and the real
        /// base ref is never pushed. Daemon soak mode.
        #[arg(long)]
        shadow: bool,
        /// Override the configured depth for work cycles.
        #[arg(long)]
        depth: Option<usize>,
        /// Override the configured maximum batch size for work cycles.
        #[arg(long)]
        max_batch: Option<usize>,
        /// Command (and leading arguments) to run instead of the configured agentctl
        /// when pushing state-change notifications. Repeatable. Supplement:
        /// MERGE_QUEUE_AGENTCTL (shell-style words) is used when neither this flag
        /// nor the configured command is set.
        #[arg(long, value_name = "WORD", allow_hyphen_values = true)]
        agentctl: Vec<String>,
        /// Seconds one notification send may run; then its whole process group is
        /// killed and the send counts as a failed attempt (retried on a later
        /// cycle, at most 3 attempts per transition). Minimum 1.
        #[arg(long, default_value_t = 30, value_name = "SECS")]
        notify_timeout_secs: u64,
        /// Also ingest pull requests carrying LABEL, inside the daemon's own
        /// loop: the same ingestion as `merge-queue sync --label LABEL` (one
        /// batched list call, approval at head when configured, withdrawal
        /// when the label goes away), under the writer lock the work cycle
        /// uses, so there is no second writer. A failed sync is logged and
        /// recorded in the heartbeat and never stops validation. Needs
        /// `github.repo` configured.
        #[arg(long, value_name = "LABEL")]
        sync_label: Option<String>,
        /// Seconds between label syncs (with --sync-label; minimum 10). Each
        /// sync spends at least one hosting API request.
        #[arg(long, default_value_t = serve::DEFAULT_SYNC_EVERY_SECS, value_name = "SECS", requires = "sync_label")]
        sync_every: u64,
        /// Override `validator.offload` for this run: `off`, `compare` (donors validate too;
        /// their receipts are only compared with the local verdict) or `decide` (donors only).
        #[arg(long, value_parser = ["off", "compare", "decide"])]
        offload: Option<String>,
    },
    /// Render and install the systemd user unit for `serve`, with the
    /// toolchain directory (containing `cargo`) resolved explicitly at
    /// install time and written as an absolute service `PATH`.
    InstallServeUnit {
        /// Directory containing `cargo` (default: resolved from
        /// `rustup which cargo`, then `cargo` on PATH, then
        /// `$CARGO_HOME/bin` or `~/.cargo/bin`).
        #[arg(long, value_name = "DIR")]
        toolchain_bin: Option<PathBuf>,
        /// Unit file to write (default:
        /// `$XDG_CONFIG_HOME/systemd/user/merge-queue-serve.service`, else
        /// `~/.config/systemd/user/merge-queue-serve.service`).
        #[arg(long, value_name = "FILE")]
        output: Option<PathBuf>,
        /// Print the rendered unit instead of writing it.
        #[arg(long)]
        print: bool,
    },
    /// Donor validators: other hosts validate queued candidates and return exact-SHA receipts
    /// (see the user guide's "Donor validators"). `run` is the donor daemon; the other verbs
    /// are what a donor calls on the queue host through its route.
    Donor {
        #[command(subcommand)]
        action: DonorCommand,
    },
    /// Show daemon liveness, the current window and slot occupancy.
    Status {
        /// Print the status report as JSON.
        #[arg(long)]
        json: bool,
        /// Heartbeat age in seconds after which a live daemon counts as stale.
        #[arg(long, default_value_t = serve::DEFAULT_STALE_AFTER_SECS)]
        stale_after_secs: u64,
    },
    /// Run the complete suite on the base and discharge the obligations it covers.
    Widen {
        /// Print the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// List widening obligations left by focused landings.
    Obligations {
        /// Include discharged obligations.
        #[arg(long)]
        all: bool,
        /// Print JSON records.
        #[arg(long)]
        json: bool,
    },
    /// Throughput metrics for a window, from the decision log (one record per validation):
    /// batch-width distribution, success rate by width, changes landed per validate-hour and
    /// ready-to-land latency percentiles.
    Stats {
        /// Window start (inclusive): an RFC 3339 time, or a duration before now such as `24h`,
        /// `90m` or `7d`.
        #[arg(long, default_value = "24h", value_name = "TIME")]
        since: String,
        /// Window end (exclusive): an RFC 3339 time, or a duration before now. Default: now.
        #[arg(long, value_name = "TIME")]
        until: Option<String>,
        /// Count only validations of this mode: `run` (real landings), `shadow` (simulated
        /// landings) or `widen`. Default: every mode.
        #[arg(long, value_parser = ["run", "shadow", "widen"])]
        mode: Option<String>,
        /// Rebuild the records from the run reports under `runs/` instead of reading the
        /// decision log, for windows before the log existed, and list what cannot be
        /// reconstructed.
        #[arg(long)]
        reconstruct: bool,
        /// Print the metrics as JSON; with --records, the records themselves as JSON lines.
        #[arg(long)]
        json: bool,
        /// Print the window's decision records (JSON lines) instead of the metrics.
        #[arg(long)]
        records: bool,
    },
    /// Measure landing throughput of the queue against a serial lander on a synthetic repository.
    Bench {
        /// Number of changes in the synthetic queue.
        #[arg(long, default_value_t = 16)]
        prs: usize,
        /// Fraction of changes that fail their test.
        #[arg(long, default_value_t = 0.125)]
        fail_rate: f64,
        /// Wall seconds every validation costs (stands in for the real validation time).
        #[arg(long, default_value_t = 1.0)]
        test_secs: f64,
        /// Queue speculative depth.
        #[arg(long, default_value_t = DEFAULT_DEPTH)]
        depth: usize,
        /// Queue maximum batch size.
        #[arg(long, default_value_t = DEFAULT_MAX_BATCH)]
        max_batch: usize,
        /// Seed that picks the failing changes.
        #[arg(long, default_value_t = 1)]
        seed: u64,
        /// Keep the synthetic repositories in DIR (default: a temporary directory, deleted).
        #[arg(long, value_name = "DIR")]
        keep: Option<PathBuf>,
        /// Print JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Print a short getting-started guide.
    Quickstart,
    /// Print the complete user guide.
    Userguide,
}

fn print_report(report: &RunReport, json: bool) -> Result<(), String> {
    if json {
        let text = serde_json::to_string_pretty(report).map_err(|e| e.to_string())?;
        println!("{text}");
    } else {
        print!("{}", run::render_report(report));
    }
    Ok(())
}

fn parse_combine(text: &str) -> Result<CombineMode, String> {
    match text {
        "rebase" => Ok(CombineMode::Rebase),
        "merge" => Ok(CombineMode::Merge),
        other => Err(format!(
            "unknown combine mode {other:?}; use rebase or merge"
        )),
    }
}

fn repo_dir(cli_repo: Option<PathBuf>) -> Result<PathBuf, String> {
    let start = match cli_repo {
        Some(path) => path,
        None => {
            std::env::current_dir().map_err(|error| format!("no current directory: {error}"))?
        }
    };
    let git = Git::new(&start);
    let top = git
        .run(&["rev-parse", "--show-toplevel"])
        .ok()
        .filter(|top| !top.is_empty());
    let dir = match top {
        Some(top) => PathBuf::from(top),
        None => git
            .run(&["rev-parse", "--absolute-git-dir"])
            .map(PathBuf::from)
            .map_err(|_| format!("{} is not inside a Git repository", start.display()))?,
    };
    Ok(dir)
}

fn state_dir(cli_state: Option<PathBuf>, repo: &std::path::Path) -> Result<PathBuf, String> {
    if let Some(dir) = cli_state {
        return Ok(dir);
    }
    if let Some(dir) = std::env::var_os("MERGE_QUEUE_STATE").filter(|dir| !dir.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    Ok(Git::new(repo).common_dir()?.join("merge-queue"))
}

/// Run the command line; returns the process exit code.
pub fn main(args: impl IntoIterator<Item = OsString>) -> i32 {
    let argv = std::iter::once(OsString::from("merge-queue")).chain(args);
    let cli = match Cli::try_parse_from(argv) {
        Ok(cli) => cli,
        Err(error) => {
            let code = if error.use_stderr() { 2 } else { 0 };
            let _ = error.print();
            return code;
        }
    };
    match dispatch(cli) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("merge-queue: {error}");
            1
        }
    }
}

fn dispatch(cli: Cli) -> Result<i32, String> {
    match cli.command {
        Command::Quickstart => {
            print!("{}", crate::QUICKSTART);
            return Ok(0);
        }
        Command::Userguide => {
            print!("{}", crate::USER_GUIDE);
            return Ok(0);
        }
        Command::Bench {
            prs,
            fail_rate,
            test_secs,
            depth,
            max_batch,
            seed,
            keep,
            json,
        } => {
            let report = crate::bench::bench(&crate::bench::BenchOptions {
                prs,
                fail_rate,
                test_secs,
                depth: depth.max(1),
                max_batch: max_batch.max(1),
                seed,
                keep,
            })?;
            if json {
                let text = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
                println!("{text}");
            } else {
                println!(
                    "{} changes, {} failing ({}), {}s per validation",
                    report.prs,
                    report.failing.len(),
                    report
                        .failing
                        .iter()
                        .map(|pr| format!("#{pr}"))
                        .collect::<Vec<_>>()
                        .join(","),
                    report.test_secs
                );
                println!(
                    "{:<8} {:>5} {:>9} {:>9} {:>7} {:>8} {:>11} {:>8} {:>10}",
                    "config",
                    "depth",
                    "max-batch",
                    "selection",
                    "landed",
                    "ejected",
                    "validations",
                    "wall-s",
                    "changes/h"
                );
                for row in &report.rows {
                    println!(
                        "{:<8} {:>5} {:>9} {:>9} {:>7} {:>8} {:>11} {:>8.1} {:>10.1}",
                        row.name,
                        row.depth,
                        row.max_batch,
                        row.selection,
                        row.landed,
                        row.ejected,
                        row.validations,
                        row.wall_s,
                        row.prs_per_hour
                    );
                }
                println!("speedup: {:.2}x", report.speedup);
            }
            return Ok(0);
        }
        Command::Donor {
            action: DonorCommand::SameRepository { a, b },
        } => {
            let (key_a, key_b) = (
                crate::donor::repository_key(&a),
                crate::donor::repository_key(&b),
            );
            println!("{key_a}\n{key_b}");
            return Ok(if key_a == key_b { 0 } else { 1 });
        }
        Command::Donor {
            action:
                DonorCommand::Run {
                    queue,
                    clone,
                    remote,
                    name,
                    jobs,
                    poll_secs,
                    work,
                    mismatch_backoff_secs,
                    until_idle,
                },
        } => {
            let queue = shell_words::split(&queue)
                .map_err(|error| format!("--queue {queue:?}: {error}"))?;
            let work = match work {
                Some(work) => work,
                None => {
                    let dir =
                        crate::git::Git::new(&clone).run(&["rev-parse", "--absolute-git-dir"])?;
                    PathBuf::from(dir).join("merge-queue-donor")
                }
            };
            return crate::donor::run_donor(&crate::donor::DonorOptions {
                queue,
                clone,
                remote,
                name,
                jobs,
                poll: std::time::Duration::from_secs(poll_secs.max(1)),
                work,
                mismatch_backoff: std::time::Duration::from_secs(mismatch_backoff_secs),
                until_idle,
            });
        }
        _ => {}
    }
    let repo = repo_dir(cli.repo)?;
    let state = State::at(state_dir(cli.state, &repo)?);
    match cli.command {
        Command::Init {
            remote,
            base,
            combine,
            max_batch,
            depth,
            max_depth,
            union_path,
            priority_label,
            ci_path,
            classifier_policy,
            regen_path,
            regen_command,
            regen_build,
            regen_when,
            exact_sha,
            trust_base,
            classify,
            identity_name,
            identity_email,
            peer,
            github_repo,
            gh,
            api_per_hour,
            api_budget_file,
            anonymous_fetch,
            require_approval_at_head,
            recount_pins,
            close_prs,
            net_wrapper,
            validator,
            validator_run,
            select,
            id_prefix,
            policy,
            dag,
            dagrun,
            admission,
            timeout,
            refusal_max_retries,
            refusal_backoff_secs,
            green_flaky_hold_secs,
            max_deferrals,
            red_without_probe,
            offload,
            tool_identity,
            clean_ignored,
            land,
            land_command,
            allow_batches,
            agentctl,
            force,
        } => {
            if state.exists() && !force {
                return Err(format!(
                    "{} already exists (use --force to rewrite the config)",
                    state.config_path().display()
                ));
            }
            let config = Config {
                repo,
                remote,
                base,
                net_wrapper,
                combine,
                max_batch: max_batch.max(1),
                depth: depth.max(1),
                max_depth,
                union_paths: union_path,
                priority: PriorityConfig {
                    label: if priority_label == crate::state::DEFAULT_PRIORITY_LABEL {
                        String::new()
                    } else {
                        priority_label
                    },
                    ci_paths: ci_path,
                },
                classifier: crate::state::ClassifierConfig {
                    policy: classifier_policy,
                },
                regenerate: RegenerateConfig {
                    paths: regen_path,
                    command: regen_command,
                    build_command: regen_build,
                    when: regen_when,
                },
                exact_sha,
                trust_base,
                require_approval_at_head,
                recount_pins,
                identity: IdentityConfig {
                    name: identity_name,
                    email: identity_email,
                },
                classify: classify
                    .iter()
                    .map(|spec| {
                        spec.split_once('=')
                            .filter(|(pattern, class)| !pattern.is_empty() && !class.is_empty())
                            .map(|(pattern, class)| ClassRule {
                                pattern: pattern.to_owned(),
                                class: class.to_owned(),
                            })
                            .ok_or_else(|| format!("--classify {spec:?}: expected PATTERN=CLASS"))
                    })
                    .collect::<Result<_, _>>()?,
                peers: peer
                    .iter()
                    .map(|spec| {
                        spec.split_once('=')
                            .map(|(name, dir)| (name.to_owned(), PathBuf::from(dir)))
                            .ok_or_else(|| format!("--peer {spec:?}: expected NAME=DIR"))
                    })
                    .collect::<Result<_, _>>()?,
                github: GitHubConfig {
                    repo: github_repo,
                    gh,
                    max_per_hour: api_per_hour,
                    budget_file: api_budget_file,
                    anonymous_fetch,
                },
                validator: ValidatorConfig {
                    kind: validator,
                    list: String::new(),
                    select,
                    id_prefix,
                    policy,
                    run: validator_run,
                    dagrun,
                    dag,
                    admission,
                    timeout_secs: timeout,
                    clean_ignored,
                    refusal_max_retries,
                    refusal_backoff_secs,
                    green_flaky_hold_secs: if green_flaky_hold_secs
                        == crate::state::DEFAULT_GREEN_FLAKY_HOLD_SECS
                    {
                        0
                    } else {
                        green_flaky_hold_secs
                    },
                    max_deferrals: if max_deferrals == crate::state::DEFAULT_RETRY_BOUND {
                        0
                    } else {
                        max_deferrals
                    },
                    red_without_probe: if red_without_probe == "eject" {
                        String::new()
                    } else {
                        red_without_probe
                    },
                    offload: if offload == "off" {
                        String::new()
                    } else {
                        offload
                    },
                    tool_identity: tool_identity
                        .iter()
                        .map(|spec| {
                            spec.split_once('=')
                                .filter(|(name, command)| !name.is_empty() && !command.is_empty())
                                .map(|(name, command)| (name.to_owned(), command.to_owned()))
                                .ok_or_else(|| {
                                    format!("--tool-identity {spec:?}: expected NAME=COMMAND")
                                })
                        })
                        .collect::<Result<_, _>>()?,
                },
                land: LandConfig {
                    driver: land,
                    command: land_command,
                    allow_batches,
                    close_prs,
                },
                notify: NotifyConfig { agentctl },
                ..Config::default()
            };
            state.init(&config)?;
            println!("initialised merge queue at {}", state.root.display());
            Ok(0)
        }
        Command::Submit {
            pr,
            head,
            sha,
            priority,
            after,
            solo,
            prioritized,
            by,
            title,
            tested_with,
            requires,
            policy,
            notify,
        } => {
            let config = state.config()?;
            let _lock = state.lock()?;
            let claim = match tested_with {
                Some(text) => Some(ValidationClaim::new(
                    parse_selection(&text)?,
                    policy,
                    sha.clone(),
                )),
                None => None,
            };
            let item = ops::submit(
                &state,
                &config,
                Submission {
                    pr,
                    head_ref: head,
                    head: sha,
                    priority,
                    by,
                    notify,
                    title,
                    after,
                    solo,
                    prioritized,
                    claim,
                    source: String::new(),
                    requires: requires
                        .iter()
                        .map(|spec| ops::resolve_requirement(&config, spec))
                        .collect::<Result<_, _>>()?,
                },
            )?;
            let claim = match &item.claim {
                Some(claim) => format!(
                    "; validated with {} ({})",
                    claim.selection.summary(),
                    claim.scope.as_str()
                ),
                None => String::new(),
            };
            println!(
                "queued #{} at {} (priority {}{}){claim}",
                item.pr,
                short(&item.head),
                item.priority,
                if item.prioritized {
                    ", front of the queue"
                } else {
                    ""
                }
            );
            Ok(0)
        }
        Command::Sync {
            label,
            all_open,
            file,
            limit,
            dry_run,
            json,
        } => {
            let config = state.config()?;
            let source = match (label, all_open, file) {
                (Some(label), _, _) => crate::sync::ReadySource::Label(label),
                (_, true, _) => crate::sync::ReadySource::AllOpen,
                (_, _, Some(file)) => crate::sync::ReadySource::File(file),
                _ => return Err("choose a ready source: --label, --all-open or --file".to_owned()),
            };
            let lock = if dry_run { None } else { Some(state.lock()?) };
            let report = crate::sync::sync(&state, &config, &source, limit, lock.as_ref(), None)?;
            if json {
                let text = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
                println!("{text}");
            } else {
                let list = |prs: &[i64]| {
                    prs.iter()
                        .map(|pr| format!("#{pr}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                println!(
                    "{}{}: {} ready; submitted [{}], unchanged {}, withdrawn [{}]{}",
                    report.source,
                    if report.dry_run { " (dry run)" } else { "" },
                    report.listed,
                    list(&report.submitted),
                    report.unchanged.len(),
                    list(&report.withdrawn),
                    report
                        .api_remaining
                        .map(|left| format!("; {left} API requests left this hour"))
                        .unwrap_or_default()
                );
                if !report.drafts.is_empty() {
                    let drafts: Vec<String> =
                        report.drafts.iter().map(|pr| format!("#{pr}")).collect();
                    println!("  drafts, not ready: {}", drafts.join(", "));
                }
                for (pr, why) in &report.refused {
                    println!("  refused #{pr}: {why}");
                }
                for pr in &report.unapproved {
                    println!("  not ready #{pr}: no approving review at current head");
                }
                if !report.closed.is_empty() {
                    println!("  closed landed: {}", list(&report.closed));
                }
            }
            Ok(if report.refused.is_empty() { 0 } else { 1 })
        }
        Command::Withdraw { pr, reason } => {
            let _lock = state.lock()?;
            let item = ops::withdraw(&state, pr, &reason)?;
            println!("withdrew #{}", item.pr);
            Ok(0)
        }
        Command::Retry { prs } => {
            let _lock = state.lock()?;
            for pr in prs {
                ops::retry(&state, pr)?;
                println!("#{pr} is back in the queue");
            }
            Ok(0)
        }
        Command::List { all, json } => {
            let items: Vec<_> = state
                .items()?
                .into_iter()
                .filter(|item| all || item.status == ItemStatus::Queued)
                .collect();
            if json {
                let text = serde_json::to_string_pretty(&items).map_err(|e| e.to_string())?;
                println!("{text}");
            } else if items.is_empty() {
                println!("queue is empty");
            } else {
                println!(
                    "{:<8} {:<10} {:<8} {:<10} {:<10} TITLE",
                    "PR", "STATUS", "PRIO", "HEAD", "TESTED"
                );
                for item in items {
                    let tested = item
                        .claim
                        .as_ref()
                        .map_or("-", |claim| claim.scope.as_str());
                    println!(
                        "#{:<7} {:<10} {:<8} {:<10} {:<10} {}",
                        item.pr,
                        item.status.as_str(),
                        if item.prioritized {
                            format!("{}!", item.priority)
                        } else {
                            item.priority.to_string()
                        },
                        short(&item.head),
                        tested,
                        item.title
                    );
                }
            }
            Ok(0)
        }
        Command::Plan { max_batch, json } => {
            let config = state.config()?;
            let plan = ops::plan(&state, &config, max_batch.unwrap_or(config.max_batch))?;
            if json {
                let text = serde_json::to_string_pretty(&plan).map_err(|e| e.to_string())?;
                println!("{text}");
            } else {
                print!("{}", render(&plan));
            }
            Ok(0)
        }
        Command::Run {
            dry_run,
            shadow,
            depth,
            max_depth,
            max_batch,
            max_batches,
            widen,
            with_peers,
            json,
            offload,
        } => {
            let _lock = state.lock()?;
            // Ctrl-C or a service stop cancels the validations and stops with no verdict
            // instead of orphaning them in their own process groups.
            crate::interrupt::install();
            let runner = Runner::new(
                &state,
                RunOptions {
                    dry_run,
                    shadow,
                    depth,
                    max_depth,
                    max_batch,
                    max_batches,
                    widen,
                    quiet: false,
                    with_peers,
                    offload,
                },
            )?;
            let report = runner.run()?;
            print_report(&report, json)?;
            Ok(if report.stopped.is_empty() { 0 } else { 3 })
        }
        Command::Serve {
            poll_ms,
            base_poll_secs,
            once,
            shadow,
            depth,
            max_batch,
            agentctl,
            notify_timeout_secs,
            sync_label,
            sync_every,
            offload,
        } => serve::serve(
            &state,
            &ServeOptions {
                poll: std::time::Duration::from_millis(poll_ms.max(10)),
                base_poll: std::time::Duration::from_secs(base_poll_secs),
                once,
                shadow,
                depth,
                max_batch,
                agentctl: if agentctl.is_empty() {
                    None
                } else {
                    Some(agentctl)
                },
                notify_timeout: std::time::Duration::from_secs(notify_timeout_secs.max(1)),
                sync_label,
                sync_every: std::time::Duration::from_secs(sync_every.max(10)),
                offload,
            },
        ),
        Command::InstallServeUnit {
            toolchain_bin,
            output,
            print,
        } => {
            let binary = std::env::current_exe()
                .map_err(|e| format!("cannot find this merge-queue binary: {e}"))?;
            let queue_repo = state
                .config()
                .ok()
                .filter(|c| !c.repo.as_os_str().is_empty())
                .map_or_else(|| repo.clone(), |c| c.repo);
            if print {
                let toolchain = serve::checked_toolchain_bin(toolchain_bin.as_deref())?;
                print!(
                    "{}",
                    serve::render_serve_unit(&binary, &queue_repo, &toolchain)?
                );
                return Ok(0);
            }
            let output = match output {
                Some(path) => path,
                None => serve::default_unit_dir()?.join("merge-queue-serve.service"),
            };
            let written =
                serve::install_serve_unit(&binary, &queue_repo, toolchain_bin.as_deref(), &output)?;
            println!("wrote {}", written.display());
            println!(
                "next: systemctl --user daemon-reload && systemctl --user enable --now merge-queue-serve"
            );
            Ok(0)
        }
        Command::Donor { action } => donor_command(&state, action),
        Command::Status {
            json,
            stale_after_secs,
        } => {
            if !state.exists() {
                return Err(format!(
                    "no merge queue at {} (daemon cannot be running); remedy: run `merge-queue init` first, then `merge-queue serve`",
                    state.root.display()
                ));
            }
            let report = serve::collect_status(&state, stale_after_secs)?;
            if json {
                let text = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
                println!("{text}");
            } else {
                print!("{}", serve::render_status(&report));
            }
            Ok(if report.healthy() { 0 } else { 1 })
        }
        Command::Widen { json } => {
            let _lock = state.lock()?;
            let report = run::widen(&state)?;
            print_report(&report, json)?;
            // A refusal is not red, but it also discharged nothing: exit 3 (retry later),
            // never 0-as-pass on a refused complete run.
            let refused = report.validations.iter().any(|v| v.refused);
            let red = report
                .validations
                .iter()
                .any(|v| !v.green() && !v.refused && !v.cancelled);
            Ok(if red || refused { 3 } else { 0 })
        }
        Command::Stats {
            since,
            until,
            mode,
            reconstruct,
            json,
            records,
        } => {
            let now = chrono::Utc::now();
            let since = crate::decisions::parse_bound(&since, now)?;
            let until = match until {
                Some(text) => crate::decisions::parse_bound(&text, now)?,
                None => now + chrono::Duration::seconds(1),
            };
            let (mut decisions, notes) = if reconstruct {
                let config = state.config()?;
                let items: std::collections::BTreeMap<i64, crate::model::Item> = state
                    .items()?
                    .into_iter()
                    .map(|item| (item.pr, item))
                    .collect();
                let git = crate::git::Git::new(&config.repo);
                let sources = crate::decisions::Sources {
                    items: &items,
                    git: Some(&git),
                    priority: &config.priority,
                    classifier_policy: &config.classifier.policy,
                };
                crate::decisions::reconstruct(&state.root, &sources, since, until)?
            } else {
                (
                    crate::decisions::read(&state.root, since, until)?,
                    Vec::new(),
                )
            };
            if let Some(mode) = &mode {
                decisions.retain(|decision| &decision.mode == mode);
            }
            if records {
                for decision in &decisions {
                    println!(
                        "{}",
                        serde_json::to_string(decision).map_err(|e| e.to_string())?
                    );
                }
                return Ok(0);
            }
            let mut stats = crate::decisions::stats(&decisions, since, until);
            if reconstruct {
                stats.reconstructed = true;
                stats.not_reconstructable = crate::decisions::NOT_RECONSTRUCTABLE
                    .iter()
                    .map(|line| (*line).to_owned())
                    .chain(notes)
                    .collect();
            }
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&stats).map_err(|e| e.to_string())?
                );
            } else {
                print!("{}", crate::decisions::render(&stats));
            }
            Ok(0)
        }
        Command::Obligations { all, json } => {
            let obligations: Vec<_> = state
                .obligations()?
                .into_iter()
                .filter(|o| all || o.discharged.is_none())
                .collect();
            if json {
                let text = serde_json::to_string_pretty(&obligations).map_err(|e| e.to_string())?;
                println!("{text}");
            } else if obligations.is_empty() {
                println!("no open obligations");
            } else {
                for o in obligations {
                    let status = match &o.discharged {
                        None => "open".to_owned(),
                        Some(d) if d.green() => {
                            format!("discharged green at {}", short(&d.commit))
                        }
                        Some(d) => format!(
                            "complete run RED at {}: {}",
                            short(&d.commit),
                            crate::run::format_failed_results(&d.failed_results())
                        ),
                    };
                    println!(
                        "{}  landed {}  {}  validated with {} ({})  {status}",
                        o.id,
                        short(&o.landed),
                        o.prs
                            .iter()
                            .map(|pr| format!("#{pr}"))
                            .collect::<Vec<_>>()
                            .join(","),
                        o.validated.selection.summary(),
                        o.validated.policy
                    );
                }
            }
            Ok(0)
        }
        Command::Quickstart | Command::Userguide | Command::Bench { .. } => {
            unreachable!("handled above")
        }
    }
}
