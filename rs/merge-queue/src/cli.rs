//! Command-line interface.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::git::{CombineMode, Git};
use crate::intake;
use crate::model::{waiting_on, Item, ItemStatus, Waiting};
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
  merge-queue drain-for-upgrade --wait
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

/// `merge-queue known-flaky` verbs.
#[derive(Debug, Subcommand)]
enum KnownFlakyCommand {
    /// Add a test to the list (or replace its entry). The entry covers the test, its cells
    /// (`TEST[cell]`) and its cases (`TEST::case`).
    Add {
        /// The test ID.
        test: String,
        /// The issue tracking its flakiness: a link or a tracker ID.
        #[arg(long)]
        issue: String,
        /// Who adds it.
        #[arg(long)]
        by: String,
        /// Why it is known flaky.
        #[arg(long, default_value = "")]
        reason: String,
    },
    /// Remove a test's entry from the list.
    Remove {
        /// The test ID, exactly as listed.
        test: String,
        /// Who removes it (printed with the removal).
        #[arg(long)]
        by: String,
    },
    /// Print the list.
    List {
        /// Print the list as JSON (schema merge-queue-known-flaky/v1).
        #[arg(long)]
        json: bool,
    },
}

/// `merge-queue signal` verbs.
#[derive(Debug, Subcommand)]
enum SignalCommand {
    /// Append one externally-produced signal (focused tests, a review, a full-validate
    /// ledger run) to a change's log.
    Add {
        /// Change number.
        pr: i64,
        /// Signal kind: `focused-tests`, `review` or `full-validate` (the queue writes
        /// `head` and `validated` itself).
        #[arg(long)]
        kind: String,
        /// Kind-specific fields as one JSON object, e.g.
        /// '{"actor":"dev","head":"abc","tests":["a"],"outcome":"pass"}'.
        #[arg(long, default_value = "{}")]
        json: String,
    },
    /// Print a change's signals, oldest first.
    List {
        /// Change number.
        pr: i64,
        /// Print as JSON lines (the stored records).
        #[arg(long)]
        json: bool,
    },
    /// Synthesize `head` and `validated` signals from the decision records and item
    /// histories already in the state. Idempotent; focused tests and reviews are
    /// never invented.
    Backfill,
    /// Append signals not yet exported to DIR (one `<pr>.jsonl` per change there),
    /// remembering what was sent.
    Export {
        /// Durable directory to append to.
        #[arg(long, value_name = "DIR")]
        to: PathBuf,
    },
}

fn signal_command(state: &State, action: SignalCommand) -> Result<i32, String> {
    match action {
        SignalCommand::Add { pr, kind, json } => {
            if !matches!(kind.as_str(), "focused-tests" | "review" | "full-validate") {
                return Err(format!(
                    "signal kind {kind:?} is not an external kind; use focused-tests, \
                     review or full-validate (the queue writes head and validated itself)"
                ));
            }
            let fields: serde_json::Value = serde_json::from_str(&json)
                .map_err(|error| format!("--json is not a JSON object: {error}"))?;
            if !fields.is_object() {
                return Err("--json must be a JSON object".to_owned());
            }
            crate::signal::append(&state.root, pr, &kind, fields)?;
            println!("recorded {kind} for #{pr}");
            Ok(0)
        }
        SignalCommand::List { pr, json } => {
            let records = crate::signal::read(&state.root, pr)?;
            for record in records {
                if json {
                    println!(
                        "{}",
                        serde_json::to_string(&record).map_err(|e| e.to_string())?
                    );
                } else {
                    println!(
                        "{} {} {}",
                        record["at"].as_str().unwrap_or_default(),
                        record["kind"].as_str().unwrap_or_default(),
                        serde_json::to_string(&record).map_err(|e| e.to_string())?
                    );
                }
            }
            Ok(0)
        }
        SignalCommand::Backfill => {
            let added = crate::signal::backfill(&state.root)?;
            println!("backfilled {added} signal(s)");
            Ok(0)
        }
        SignalCommand::Export { to } => {
            let exported = crate::signal::export(&state.root, &to)?;
            println!("exported {exported} signal(s) to {}", to.display());
            Ok(0)
        }
    }
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
        /// Seconds to stop claiming after a job whose tools or merge-queue revision this host
        /// did not match (given back, or declined).
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
        /// The donor's validator probe (MQ_PROBE=1) passed after a host fault:
        /// clear the fault so this claim may lease again.
        #[arg(long)]
        probe_passed: bool,
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

/// `NAME=COMMAND` arguments of `flag`, by name.
fn name_commands(
    flag: &str,
    specs: &[String],
) -> Result<std::collections::BTreeMap<String, String>, String> {
    specs
        .iter()
        .map(|spec| {
            spec.split_once('=')
                .filter(|(name, command)| !name.is_empty() && !command.is_empty())
                .map(|(name, command)| (name.to_owned(), command.to_owned()))
                .ok_or_else(|| format!("{flag} {spec:?}: expected NAME=COMMAND"))
        })
        .collect()
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
    store.host_fault_escalate_after = config.donors.host_fault_escalate_after();
    if crate::state::Offload::parse(config.validator.offload.as_str())
        == Ok(crate::state::Offload::Decide)
    {
        store.compare_only = config.donors.compare_only.iter().cloned().collect();
    }
    store
}

/// Print one JSON reply, carrying any config warnings in a `warnings` field:
/// nothing else may reach stdout before the JSON (a route may merge stderr into it,
/// and the donor takes the last JSON line either way).
fn print_json_reply(value: &serde_json::Value) -> Result<(), String> {
    let warnings = crate::state::take_config_warnings();
    let mut value = value.clone();
    if !warnings.is_empty() {
        if let Some(object) = value.as_object_mut() {
            object.insert("warnings".to_owned(), serde_json::json!(warnings));
        }
    }
    println!(
        "{}",
        serde_json::to_string(&value).map_err(|error| error.to_string())?
    );
    Ok(())
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
            print_json_reply(&serde_json::to_value(&info).map_err(|error| error.to_string())?)?;
            Ok(0)
        }
        DonorCommand::Claim {
            donor,
            host,
            jobs,
            probe_passed,
        } => {
            if probe_passed {
                store.note_probe_pass(&donor);
            }
            let cap = config.donors.max_jobs_per_donor;
            let jobs = if cap > 0 { jobs.min(cap) } else { jobs };
            // Answer the tool identity now, so a job published before a tool moved is offered
            // under the tools this host would validate with (see `Store::current_tools`).
            let mut store = store;
            match crate::donor::tool_answers(&config.validator, &config.repo) {
                Ok(answers) => store.current_tools = Some(answers),
                Err(error) => {
                    eprintln!(
                        "merge-queue donor claim: jobs keep their published tool stamps: {error}"
                    )
                }
            }
            // Freshness before setup: never hand out a job whose candidate no longer
            // contains the current base. It is cancelled as stale here, not refused by
            // the donor after its expensive setup, and costs the donor no strike.
            {
                let git = crate::git::Git::new(&config.repo);
                if let Ok((base, _)) = crate::ops::refresh(&git, &config, &[]) {
                    let contains = |commit: &str| git.is_ancestor(&base, commit).unwrap_or(true);
                    match store.sweep_stale(&base, &contains) {
                        Ok(cancelled) if !cancelled.is_empty() => eprintln!(
                            "merge-queue donor claim: cancelled {} stale donor job(s) \
                             whose candidate does not contain {}",
                            cancelled.len(),
                            crate::plan::short(&base)
                        ),
                        Ok(_) => {}
                        Err(error) => {
                            eprintln!("merge-queue donor claim: stale sweep failed: {error}")
                        }
                    }
                }
            }
            let job = store.claim(&donor, &host, jobs)?;
            print_json_reply(
                &serde_json::to_value(&crate::donor::Claimed { job })
                    .map_err(|error| error.to_string())?,
            )?;
            Ok(0)
        }
        DonorCommand::Renew { job } => {
            let renewal = store.renew(&job, &read_token()?)?;
            print_json_reply(&serde_json::to_value(&renewal).map_err(|error| error.to_string())?)?;
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
            print_json_reply(&serde_json::json!({"accepted": true}))?;
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
            for alert in store.give_back_alerts(config.donors.give_back_alert())? {
                println!("{alert}");
            }
            for (donor, reason, since) in store.host_fault_list() {
                println!(
                    "ERROR: donor {donor}: host cannot validate: {reason} (since {since}); \
                     it is leased nothing until an admitted validation or a passing probe"
                );
            }
            for alert in store.host_fault_escalations() {
                println!("{alert}");
            }
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
        /// A conflict in one is settled on the target's side before the command runs (the
        /// target's lines win where both sides edited the same lines). Repeatable.
        #[arg(long, value_name = "PATTERN")]
        regen_path: Vec<String>,
        /// Shell command, run as `sh -c` at the root of the candidate checkout, that
        /// rewrites every generated file deterministically. Runs with `MQ_PR`,
        /// `MQ_REGEN_PATHS`, `MQ_CONFLICTED`, `MQ_HEAD` (the change's head), `MQ_PARENT` (the
        /// commit it was combined onto) and `MQ_CHANGE_BASE` (where the change's own edits
        /// start, past any change it is stacked on) set. A stdout line `merge-queue-note: TEXT`
        /// is recorded in the run's events and the change's history; on failure, its
        /// `merge-queue-conflict: TEXT` lines eject the change as a conflict.
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
        /// Failure class PATTERN=CLASS[=EVIDENCE]: failed tests whose ID (or step) matches
        /// PATTERN (exact or `*` wildcards) are labelled CLASS in ejections and reports. With
        /// EVIDENCE (text with `*` wildcards), only when a validation-log line tagged with the
        /// test's step (`[step] ...`) contains it, e.g.
        /// `test.*_count=count pin drift=expected * tests to execute, saw *`. Repeatable.
        #[arg(long, value_name = "PATTERN=CLASS[=EVIDENCE]")]
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
        /// conflicts as real. A pin a change introduces takes its head's value;
        /// a decrease lands only as the change's head commit declares it
        /// (`Count-Pin-Decrease: <pin> <amount> <reason>`). Off by default.
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
        /// Seconds one network command (`git fetch`, `ls-remote`, `push`, a `gh` call) may run
        /// before it is killed and fails; 0 means 300.
        #[arg(long, default_value_t = 0, value_name = "SECS")]
        net_timeout_secs: u64,
        /// Precheck: shell command every submitted head must pass, run on the head alone in a
        /// checkout the queue keeps, before the change is queued (a formatter check, say). A
        /// head that fails it is ejected at submission with the command's output. Empty: no
        /// precheck.
        #[arg(long, default_value = "", value_name = "COMMAND")]
        precheck: String,
        /// Seconds one precheck may run before it is killed and counts as one that could not
        /// run (the change is then queued without it); 0 means 300.
        #[arg(long, default_value_t = 0, value_name = "SECS")]
        precheck_timeout_secs: u64,
        /// Background precheck: shell command for checks too slow to wait for (a documentation
        /// build, a linter that compiles), run on every queued head, one at a time, by a worker
        /// beside the run. A head whose verdict is a failure is ejected when its batch is built;
        /// one not checked yet joins the batch. Empty: none.
        #[arg(long, default_value = "", value_name = "COMMAND")]
        precheck_background: String,
        /// Seconds one background precheck may run before it is killed and counts as one that
        /// could not run; 0 means 1800.
        #[arg(long, default_value_t = 0, value_name = "SECS")]
        precheck_background_timeout_secs: u64,
        /// A validation step the precheck commands reproduce (repeatable). When every failure of
        /// a red batch is such a step and a change in it failed its precheck, that change is
        /// ejected and the rest go back as one batch instead of one batch each.
        #[arg(long = "precheck-covers", value_name = "STEP")]
        precheck_covers: Vec<String>,
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
        /// retry) or an INFRA-BLOCKED one (the host refused to measure some tests on a run and
        /// its rerun) is held for a ruling before it is re-queued.
        #[arg(long, default_value_t = crate::state::DEFAULT_GREEN_FLAKY_HOLD_SECS)]
        green_flaky_hold_secs: u64,
        /// What a GREEN_FLAKY result does when its retried tests are not all
        /// on the known-flaky list: `hold` it for a ruling (the default) or
        /// `land` it at once with the retried tests recorded.
        #[arg(long, default_value = "hold", value_parser = ["hold", "land"])]
        green_flaky: String,
        /// Times a change may be deferred because validation did not run (refusals, a
        /// validator that cannot execute, a probe or donors that could not run) before it is
        /// escalated to its owner and held until `merge-queue retry`.
        #[arg(long, default_value_t = crate::state::DEFAULT_RETRY_BOUND)]
        max_deferrals: u32,
        /// A red validation naming failed tests whose follow-up probe cannot run: `eject`
        /// the change (the red run is the verdict) or `hold` it for its owner.
        #[arg(long, default_value = "eject", value_parser = ["eject", "hold"])]
        red_without_probe: String,
        /// Ask the runner to stop each candidate validation at its first red step
        /// (`$MQ_FAIL_FAST=1`; never for the complete run on the base). Steps it skips must be
        /// reported as `skip`.
        #[arg(long)]
        fail_fast: bool,
        /// Peripheral classifier: a shell command run in a checkout of each change head
        /// (with `$MQ_BASE`, `$MQ_HEAD`, `$MQ_PR`) whose first output line of exactly
        /// `peripheral` marks the change peripheral. Anything else, an error, or a timeout
        /// means binary-touching. Empty (the default) disables the peripheral lane.
        #[arg(long, default_value = "")]
        peripheral_command: String,
        /// Seconds one peripheral-classifier call may run (0: 30).
        #[arg(long, default_value_t = 0)]
        peripheral_timeout_secs: u64,
        /// Largest batch of only peripheral changes (0: twice --max-batch).
        #[arg(long, default_value_t = 0)]
        peripheral_max_batch: usize,
        /// Path in the repository of the test-footprints JSON (`{test id: [paths]}`) for
        /// the conflict prior. Empty (the default) disables the prior.
        #[arg(long, default_value = "")]
        conflict_footprints: String,
        /// Value-ordered scheduling: abort the deepest in-flight batch for a waiting
        /// batch with a better historical green rate.
        #[arg(long)]
        value_ordering: bool,
        /// Only start a disjoint second root when the red share of the last 20
        /// decided validations in the decision records is at least this percent
        /// (0, the default, always starts one when it fits).
        #[arg(long, default_value_t = 0)]
        disjoint_root_min_red_percent: u32,
        /// How many base-rooted batches may validate at once, each disjoint
        /// from everything in flight (default 1: today's behaviour).
        #[arg(long, default_value_t = 1)]
        roots: usize,
        /// Keep a rooted batch whose base moved instead of cancelling it:
        /// a green one is requeued at the front for a rebuild, a red one is
        /// decided as usual.
        #[arg(long)]
        keep_stale_roots: bool,
        /// Build on the stack top only when the recent pass rate is at
        /// least this percent (0, the default, always stacks).
        #[arg(long, default_value_t = 0)]
        stack_min_pass_percent: u32,
        /// Build batches wider than one only when the recent pass rate is
        /// at least this percent (0, the default, always allows width).
        #[arg(long, default_value_t = 0)]
        batch_min_pass_percent: u32,
        /// Confirm each red by validating the same commit again (`$MQ_RERUN_FROM`) and search a
        /// red batch with prefix probes. Off by default: the first red is the verdict, and a red
        /// batch of several changes goes back as single-change batches.
        #[arg(long)]
        confirm_reruns: bool,
        /// With --confirm-reruns, still split a red batch of several changes into single-change
        /// batches instead of searching it with prefix probes; each red single is confirmed by
        /// one same-commit rerun of its failed tests before its change is ejected.
        #[arg(long)]
        split_red_batches: bool,
        /// With confirmation off, hold a single change whose red run failed only on timeouts or
        /// runs with no verdict (as the validator shapes its results), run those tests again at
        /// its commit beside the run, and re-queue it if they pass or eject it if they fail
        /// again; once per head.
        #[arg(long)]
        suspect_flake_reruns: bool,
        /// Wall limit in seconds of a suspect-flake rerun (0: 1800).
        #[arg(long, default_value_t = 0)]
        suspect_flake_timeout_secs: u64,
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
        /// How a donor brings the tool NAME (one of --tool-identity) up to date, as
        /// NAME=COMMAND (repeatable), for example a fetch of the tool repository. A donor whose
        /// answer differs from the job's runs it once and asks again: an older donor then takes
        /// the job; one that still differs declines it. Without one, a donor whose answer
        /// differs gives the job back instead of declining it.
        #[arg(long, value_name = "NAME=COMMAND")]
        tool_refresh: Vec<String>,
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
        /// Extra attempts for a `push` landing after a transient remote-side rejection
        /// (not a non-fast-forward), with the backoff doubled per retry (0: 2 extra
        /// attempts, three tries in all).
        #[arg(long, default_value_t = 0)]
        push_max_retries: usize,
        /// Base backoff in seconds between push retries (0: 40, so the default
        /// retries span about two minutes).
        #[arg(long, default_value_t = 0)]
        push_retry_backoff_secs: u64,
        /// Command (and leading arguments) the `serve` daemon runs instead of `agentctl`
        /// to push state-change notifications, e.g. a wrapper or a test fake. Repeatable.
        /// The daemon runs `<command> send <name> --deliver-while-working --file <report>`.
        #[arg(long, value_name = "WORD", allow_hyphen_values = true)]
        agentctl: Vec<String>,
        /// agentctl registry the notifications address: the daemon runs
        /// `<command> --registry PATH send ...`. Stored as an absolute path. Default: none,
        /// leaving agentctl's own `.agentctl` in the daemon's working directory, which for a
        /// daemon started in a bare queue repository is no registry.
        #[arg(long, value_name = "PATH")]
        agentctl_registry: Option<PathBuf>,
        /// Overwrite an existing config.json (items and history are kept).
        #[arg(long)]
        force: bool,
    },
    /// Submit (or resubmit) a change as ready to land. While a run holds the writer lock the
    /// submission is recorded (after checking that the head resolves) and applied, in order,
    /// as soon as the lock is free: by the daemon's next cycle, a run, or the next command.
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
        /// Change number that must land first (repeatable). While one of them (or one that it
        /// must land after) is ejected, withdrawn or not in the queue, this change is held: it
        /// stays queued, is never validated or landed without it, and `list` and `status` show
        /// `held: waiting on #N (ejected)`. It is planned again once #N is queued and lands
        /// first; resubmit this change without that `--after` to drop the dependency.
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
        /// Queue the head without the configured precheck (`precheck.command`), for a refusal
        /// that is wrong. Recorded in the change's history.
        #[arg(long)]
        skip_precheck: bool,
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
        /// Most pull requests to list (each 100 costs one API request). A listing that reaches
        /// the limit may be cut short, so it withdraws nothing it did not list.
        #[arg(long, default_value_t = 300)]
        limit: u32,
        /// Report what would change without submitting or withdrawing anything.
        #[arg(long)]
        dry_run: bool,
        /// Print JSON.
        #[arg(long)]
        json: bool,
    },
    /// Withdraw a queued change. While a run holds the writer lock the withdrawal is recorded
    /// and applied as soon as the lock is free; the run meanwhile does not land a batch that
    /// holds the change. A change whose submission is itself still recorded can be withdrawn
    /// too: the withdrawal is applied right after that submission.
    Withdraw {
        /// Change number.
        pr: i64,
        /// Reason recorded in the change's history.
        #[arg(long, default_value = "withdrawn by submitter")]
        reason: String,
    },
    /// Return changes escalated to their owner (validation did not run the bounded number of
    /// times) to the queue: clears the escalation, the refusal count and the backoff. Run it
    /// once whatever kept validation from running is fixed. While a run holds the writer lock
    /// the retry is recorded and applied as soon as the lock is free.
    Retry {
        /// Change numbers.
        #[arg(required = true)]
        prs: Vec<i64>,
    },
    /// Accept the GREEN_FLAKY passes of a held batch as a ruling: a run lands exactly the
    /// commit that was validated if the base has not moved since, and otherwise validates the
    /// batch again. Name every change of the held batch. While a run holds the writer lock the
    /// ruling is recorded, and the run acts on it as soon as its window has room.
    AcceptFlaky {
        /// The held batch's change numbers.
        #[arg(required = true)]
        prs: Vec<i64>,
        /// Who gives the ruling.
        #[arg(long)]
        by: String,
        /// Why the retried tests' failures are not the batch's.
        #[arg(long)]
        reason: String,
    },
    /// The known-flaky list: tests whose flakiness is known and tracked. A GREEN_FLAKY result
    /// whose retried tests are all on it lands without a hold, the tests recorded as flaky.
    KnownFlaky {
        #[command(subcommand)]
        action: KnownFlakyCommand,
    },
    /// List changes and their status, then the requests recorded while a run held the queue
    /// (`#N pending-intake: ...`) until they are applied.
    List {
        /// Include landed, ejected and withdrawn changes.
        #[arg(long)]
        all: bool,
        /// Print JSON records instead of a table: the item records, then one row per recorded
        /// request, with `"status": "pending-intake"`.
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
        /// agentctl registry the notifications address, instead of the configured one
        /// (`notify.registry`): sends run `<command> --registry PATH send ...`. Supplement:
        /// MERGE_QUEUE_AGENTCTL_REGISTRY when this flag is not given. With none of the
        /// three, agentctl looks for `.agentctl` in the daemon's working directory.
        #[arg(long, value_name = "PATH")]
        agentctl_registry: Option<PathBuf>,
        /// Seconds one notification send may run; then its whole process group is
        /// killed and the send counts as a failed attempt (retried on a later
        /// cycle, at most 3 attempts per transition). Minimum 1.
        #[arg(long, default_value_t = 30, value_name = "SECS")]
        notify_timeout_secs: u64,
        /// Also ingest pull requests carrying LABEL, on a thread beside the
        /// daemon's loop: the same ingestion as `merge-queue sync --label
        /// LABEL` (one batched list call, approval at head when configured,
        /// withdrawal when the label goes away). Nothing waits for its list
        /// call: queued changes are planned meanwhile, and what it finds while
        /// a run holds the writer lock is recorded for that run. A failed sync
        /// is logged and recorded in the heartbeat and never stops validation.
        /// Needs `github.repo` configured.
        #[arg(long, value_name = "LABEL")]
        sync_label: Option<String>,
        /// Seconds between label syncs (with --sync-label; minimum 10). Each
        /// sync spends at least one hosting API request.
        #[arg(long, default_value_t = serve::DEFAULT_SYNC_EVERY_SECS, value_name = "SECS", requires = "sync_label")]
        sync_every: u64,
        /// Pull requests one label sync lists at most (with --sync-label). Each 100 costs one
        /// API request of the budget, three with approval at head. A sync that lists this many
        /// may have been cut short: it withdraws nothing it did not list, and says so.
        #[arg(long, default_value_t = serve::DEFAULT_SYNC_LIMIT, value_name = "N", requires = "sync_label", value_parser = clap::value_parser!(u32).range(1..))]
        sync_limit: u32,
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
    /// The per-PR signal log (the model's history): add external signals, read them,
    /// backfill from decisions and item histories, and export to a durable directory.
    Signal {
        #[command(subcommand)]
        action: SignalCommand,
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
    /// Let the serve daemon finish the validations in flight, start no new one, and exit 0, so
    /// a new binary can replace it without throwing a validation away. Replace the binary
    /// file first (the running daemon keeps the file it started from), then drain, then start
    /// the service again. Stopping the daemon instead cancels its validations, and the next
    /// daemon starts them again from scratch.
    DrainForUpgrade {
        /// Why: kept with the request and in the daemon's log.
        #[arg(long, default_value = "")]
        reason: String,
        /// Wait for the daemon to exit, checking every second, up to --timeout; then exit 0.
        #[arg(long)]
        wait: bool,
        /// Seconds --wait waits before giving up with exit 1 (the request stands).
        #[arg(long, default_value_t = 7200, value_name = "SECS", requires = "wait")]
        timeout: u64,
        /// Withdraw a pending drain request instead.
        #[arg(long, conflicts_with_all = ["reason", "wait"])]
        cancel: bool,
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
    let code = match dispatch(cli) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("merge-queue: {error}");
            1
        }
    };
    // A background precheck still running would outlive the process in its own process group.
    crate::precheck::stop_background();
    crate::suspect::stop();
    code
}

/// The item records by change number.
fn records_by_pr(state: &State) -> Result<BTreeMap<i64, Item>, String> {
    Ok(state
        .items()?
        .into_iter()
        .map(|item| (item.pr, item))
        .collect())
}

/// What `submit` says about a change held for a change it must land after.
fn held_note(pr: i64, waiting: &Waiting) -> String {
    let again = if waiting.status == "not in the queue" {
        ""
    } else {
        " again"
    };
    format!(
        "#{pr} is held, {}: it is not validated or landed until #{} is queued{again} and lands; \
         resubmit #{pr} without --after {} to drop the dependency",
        waiting.describe(),
        waiting.pr,
        waiting.named()
    )
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
            net_timeout_secs,
            precheck,
            precheck_timeout_secs,
            precheck_background,
            precheck_background_timeout_secs,
            precheck_covers,
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
            green_flaky,
            roots,
            keep_stale_roots,
            stack_min_pass_percent,
            batch_min_pass_percent,
            max_deferrals,
            red_without_probe,
            fail_fast,
            peripheral_command,
            peripheral_timeout_secs,
            peripheral_max_batch,
            conflict_footprints,
            value_ordering,
            disjoint_root_min_red_percent,
            confirm_reruns,
            split_red_batches,
            suspect_flake_reruns,
            suspect_flake_timeout_secs,
            offload,
            tool_identity,
            tool_refresh,
            clean_ignored,
            land,
            land_command,
            allow_batches,
            push_max_retries,
            push_retry_backoff_secs,
            agentctl,
            agentctl_registry,
            force,
        } => {
            if state.exists() && !force {
                return Err(format!(
                    "{} already exists (use --force to rewrite the config)",
                    state.config_path().display()
                ));
            }
            let tool_identity = name_commands("--tool-identity", &tool_identity)?;
            let tool_refresh = name_commands("--tool-refresh", &tool_refresh)?;
            if let Some(name) = tool_refresh
                .keys()
                .find(|name| !tool_identity.contains_key(*name))
            {
                return Err(format!(
                    "--tool-refresh {name}: no --tool-identity names that tool"
                ));
            }
            let config = Config {
                repo,
                remote,
                base,
                net_wrapper,
                net_timeout_secs,
                precheck: crate::state::PrecheckConfig {
                    command: precheck,
                    timeout_secs: precheck_timeout_secs,
                    background: precheck_background,
                    background_timeout_secs: precheck_background_timeout_secs,
                    covers: precheck_covers,
                },
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
                        let mut parts = spec.splitn(3, '=');
                        match (parts.next(), parts.next(), parts.next()) {
                            (Some(pattern), Some(class), evidence)
                                if !pattern.is_empty() && !class.is_empty() =>
                            {
                                Ok(ClassRule {
                                    pattern: pattern.to_owned(),
                                    class: class.to_owned(),
                                    evidence: evidence.unwrap_or_default().to_owned(),
                                })
                            }
                            _ => Err(format!(
                                "--classify {spec:?}: expected PATTERN=CLASS or PATTERN=CLASS=EVIDENCE"
                            )),
                        }
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
                    green_flaky: if green_flaky == "hold" {
                        String::new()
                    } else {
                        green_flaky
                    },
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
                    fail_fast,
                    disable_early_red: false,
                    confirm_reruns,
                    split_red_batches,
                    suspect_flake_reruns,
                    suspect_flake_timeout_secs,
                    offload: if offload == "off" {
                        String::new()
                    } else {
                        offload
                    },
                    tool_identity,
                    tool_refresh,
                },
                value_ordering,
                disjoint_root_min_red_percent,
                roots,
                keep_stale_roots,
                stack_min_pass_percent,
                batch_min_pass_percent,
                conflict_prior: crate::state::ConflictPriorConfig {
                    footprints: conflict_footprints,
                },
                peripheral: crate::state::PeripheralConfig {
                    command: peripheral_command,
                    timeout_secs: peripheral_timeout_secs,
                    max_batch: peripheral_max_batch,
                },
                land: LandConfig {
                    driver: land,
                    command: land_command,
                    allow_batches,
                    close_prs,
                    push_max_retries,
                    push_retry_backoff_secs,
                },
                notify: NotifyConfig {
                    agentctl,
                    registry: match agentctl_registry {
                        Some(path) => std::path::absolute(&path)
                            .map_err(|error| {
                                format!(
                                    "cannot resolve --agentctl-registry {}: {error}",
                                    path.display()
                                )
                            })?
                            .to_string_lossy()
                            .into_owned(),
                        None => String::new(),
                    },
                },
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
            skip_precheck,
        } => {
            let config = state.config()?;
            let claim = match tested_with {
                Some(text) => Some(ValidationClaim::new(
                    parse_selection(&text)?,
                    policy,
                    sha.clone(),
                )),
                None => None,
            };
            let submission = Submission {
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
                skip_precheck,
                recorded_at: String::new(),
            };
            let lock = match state.lock() {
                Ok(lock) => lock,
                Err(busy) => {
                    // A run holds the queue: check the head now, without touching the
                    // repository the run is using, and record the submission for it.
                    if submission.pr <= 0 {
                        return Err(format!("change number must be positive, got {pr}"));
                    }
                    let current = ops::peek_head(&state, &config, &submission.head_ref)?;
                    if !submission.head.is_empty() && !current.starts_with(&submission.head) {
                        return Err(format!(
                            "#{pr}: {:?} is at {current}, not the submitted {}",
                            submission.head_ref, submission.head
                        ));
                    }
                    let after = submission.after.clone();
                    let request = intake::Request::Submit(Box::new(submission));
                    let (entry, new) = intake::record(&state, request, &busy, &current)?;
                    if new {
                        println!(
                            "recorded #{pr} (head now {}) for the next cycle: {busy}; it is \
                             applied as soon as the writer lock is free",
                            short(&current)
                        );
                    } else {
                        println!(
                            "already recorded #{pr} at {} (head now {}), not recorded again: \
                             {busy}; it is applied as soon as the writer lock is free, and \
                             `merge-queue list` shows it as {} until then",
                            entry.recorded_at,
                            short(&current),
                            intake::PENDING_INTAKE
                        );
                    }
                    for waiting in waiting_on(pr, &after, &records_by_pr(&state)?) {
                        println!("{}", held_note(pr, &waiting));
                    }
                    return Ok(0);
                }
            };
            intake::report("merge-queue submit", intake::apply(&state, &config, &lock));
            let item = ops::submit(&state, &config, submission)?;
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
            for waiting in waiting_on(item.pr, &item.after, &records_by_pr(&state)?) {
                println!("{}", held_note(item.pr, &waiting));
            }
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
            if let Some(lock) = &lock {
                intake::report("merge-queue sync", intake::apply(&state, &config, lock));
            }
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
                if report.truncated {
                    println!(
                        "  listed the limit ({limit}), so nothing missing from the listing was \
                         withdrawn; raise --limit if more are ready"
                    );
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
            let config = state.config()?;
            let lock = match state.lock() {
                Ok(lock) => lock,
                Err(busy) => {
                    // A change whose submission is still waiting may have no item record yet:
                    // its withdrawal waits behind that submission, in recording order.
                    let waiting = intake::waiting_submission(&state, pr);
                    if waiting.is_none() {
                        let item = state
                            .item(pr)?
                            .ok_or_else(|| format!("#{pr} is not in the queue"))?;
                        if item.status == ItemStatus::Landed {
                            return Err(format!("#{pr} has already landed"));
                        }
                    }
                    let behind = waiting
                        .map(|submission| {
                            format!(
                                ", right after the submission of #{pr} recorded at {}",
                                submission.recorded_at
                            )
                        })
                        .unwrap_or_default();
                    let request = intake::Request::Withdraw { pr, reason };
                    let (entry, new) = intake::record(&state, request, &busy, "")?;
                    if new {
                        println!(
                            "recorded the withdrawal of #{pr} for the next cycle: {busy}; the \
                             run does not land a batch holding #{pr}, and the withdrawal is \
                             applied as soon as the writer lock is free{behind}"
                        );
                    } else {
                        println!(
                            "already recorded the withdrawal of #{pr} at {}, not recorded \
                             again: {busy}; the run does not land a batch holding #{pr}, and the \
                             withdrawal is applied as soon as the writer lock is free{behind}",
                            entry.recorded_at
                        );
                    }
                    return Ok(0);
                }
            };
            intake::report(
                "merge-queue withdraw",
                intake::apply(&state, &config, &lock),
            );
            let item = ops::withdraw(&state, pr, &reason)?;
            println!("withdrew #{}", item.pr);
            Ok(0)
        }
        Command::Retry { prs } => {
            let config = state.config()?;
            let lock = match state.lock() {
                Ok(lock) => lock,
                Err(busy) => {
                    for pr in &prs {
                        let item = state
                            .item(*pr)?
                            .ok_or_else(|| format!("#{pr} is not in the queue"))?;
                        if item.status != ItemStatus::Queued {
                            return Err(format!("#{pr} is {}, not queued", item.status.as_str()));
                        }
                    }
                    for pr in prs {
                        let (entry, new) =
                            intake::record(&state, intake::Request::Retry { pr }, &busy, "")?;
                        if new {
                            println!(
                                "recorded the retry of #{pr} for the next cycle: {busy}; it is \
                                 applied as soon as the writer lock is free"
                            );
                        } else {
                            println!(
                                "already recorded the retry of #{pr} at {}, not recorded again: \
                                 {busy}; it is applied as soon as the writer lock is free",
                                entry.recorded_at
                            );
                        }
                    }
                    return Ok(0);
                }
            };
            intake::report("merge-queue retry", intake::apply(&state, &config, &lock));
            for pr in prs {
                ops::retry(&state, pr)?;
                println!("#{pr} is back in the queue");
            }
            Ok(0)
        }
        Command::AcceptFlaky { prs, by, reason } => {
            let config = state.config()?;
            if by.trim().is_empty() || reason.trim().is_empty() {
                return Err("a ruling names who gives it (--by) and why (--reason)".to_owned());
            }
            let lock = match state.lock() {
                Ok(lock) => lock,
                Err(busy) => {
                    let (_, held) = ops::held_batch(&state, &prs)?;
                    let request = intake::Request::AcceptFlaky {
                        prs: prs.clone(),
                        by,
                        reason,
                    };
                    let what = request.describe();
                    let (entry, new) = intake::record(&state, request, &busy, "")?;
                    let when = if new {
                        "recorded".to_owned()
                    } else {
                        format!(
                            "already recorded at {}, not recorded again",
                            entry.recorded_at
                        )
                    };
                    println!(
                        "{what}: {when}: {busy}; the run acts on it as soon as its window has \
                         room (or when it ends): it lands {} if the base is still {}, and \
                         otherwise validates the batch again",
                        short(&held.tip),
                        short(&held.parent)
                    );
                    return Ok(0);
                }
            };
            intake::report(
                "merge-queue accept-flaky",
                intake::apply(&state, &config, &lock),
            );
            let held = ops::accept_flaky(&state, &prs, &by, &reason, "")?;
            println!(
                "accepted the {} passes of {}: the next run lands {} if the base is still {}, \
                 and otherwise validates the batch again",
                crate::validate::FLAKY_MARKER,
                held.prs()
                    .iter()
                    .map(|pr| format!("#{pr}"))
                    .collect::<Vec<_>>()
                    .join(", "),
                short(&held.tip),
                short(&held.parent)
            );
            Ok(0)
        }
        Command::KnownFlaky { action } => match action {
            KnownFlakyCommand::Add {
                test,
                issue,
                by,
                reason,
            } => {
                let entry = crate::known_flaky::add(&state.root, &test, &issue, &by, &reason)?;
                println!(
                    "{} is on the known-flaky list (tracked by {}): a GREEN_FLAKY result that \
                     retried only listed tests lands without a hold",
                    entry.test, entry.issue
                );
                Ok(0)
            }
            KnownFlakyCommand::Remove { test, by } => {
                let entry = crate::known_flaky::remove(&state.root, &test)?;
                println!(
                    "{} removed from the known-flaky list by {by} (it was tracked by {})",
                    entry.test, entry.issue
                );
                Ok(0)
            }
            KnownFlakyCommand::List { json } => {
                let list = crate::known_flaky::load(&state.root)?;
                if json {
                    let text = serde_json::to_string_pretty(&list).map_err(|e| e.to_string())?;
                    println!("{text}");
                } else if list.entries.is_empty() {
                    println!("the known-flaky list is empty");
                } else {
                    for entry in &list.entries {
                        let why = if entry.reason.is_empty() {
                            String::new()
                        } else {
                            format!(": {}", entry.reason)
                        };
                        println!(
                            "{} tracked by {} (added by {} at {}){why}",
                            entry.test, entry.issue, entry.by, entry.added_at
                        );
                    }
                }
                Ok(0)
            }
        },
        Command::List { all, json } => {
            let by_pr = records_by_pr(&state)?;
            let items: Vec<&Item> = by_pr
                .values()
                .filter(|item| all || item.status == ItemStatus::Queued)
                .collect();
            // A queued change held for a change it must land after (`--after`).
            let held = |item: &Item| {
                if item.status == ItemStatus::Queued {
                    waiting_on(item.pr, &item.after, &by_pr)
                } else {
                    Vec::new()
                }
            };
            // Requests recorded while a run held the writer lock are not in any item record yet.
            let pending = intake::pending(&state);
            if json {
                let mut rows = serde_json::to_value(&items).map_err(|e| e.to_string())?;
                if let Some(rows) = rows.as_array_mut() {
                    for (row, item) in rows.iter_mut().zip(&items) {
                        let waiting = held(item);
                        if !waiting.is_empty() {
                            row["waiting_on"] =
                                serde_json::to_value(&waiting).map_err(|e| e.to_string())?;
                        }
                    }
                    for request in &pending {
                        rows.push(serde_json::to_value(request).map_err(|e| e.to_string())?);
                    }
                }
                let text = serde_json::to_string_pretty(&rows).map_err(|e| e.to_string())?;
                println!("{text}");
            } else if items.is_empty() {
                if pending.is_empty() {
                    println!("queue is empty");
                } else {
                    println!("no queued item records");
                }
            } else {
                println!(
                    "{:<8} {:<10} {:<8} {:<10} {:<10} TITLE",
                    "PR", "STATUS", "PRIO", "HEAD", "TESTED"
                );
                for item in &items {
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
                    for waiting in held(item) {
                        println!("{:<8} held: {}", "", waiting.describe());
                    }
                }
            }
            if !json && !pending.is_empty() {
                println!(
                    "recorded while a run held the queue, applied as soon as the writer lock is \
                     free:"
                );
                for request in &pending {
                    println!("{}", request.line());
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
            let lock = state.lock()?;
            // Requests recorded while another writer held the queue go in before it is read
            // (a dry run changes nothing).
            if !dry_run {
                intake::report(
                    "merge-queue run",
                    intake::apply(&state, &state.config()?, &lock),
                );
            }
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
                    // A drain is for the serve daemon; a one-shot run never drains.
                    drain_pid: None,
                    // A one-shot run reports its own progress; no heartbeat shows a window.
                    window: None,
                    intake_poll: None,
                    // Its process ends with it: it waits for the work it left beside it.
                    wait_for_background: true,
                },
            )?;
            let report = runner.holding(&lock).run()?;
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
            agentctl_registry,
            notify_timeout_secs,
            sync_label,
            sync_every,
            sync_limit,
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
                agentctl_registry,
                notify_timeout: std::time::Duration::from_secs(notify_timeout_secs.max(1)),
                sync_label,
                sync_every: std::time::Duration::from_secs(sync_every.max(10)),
                sync_limit,
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
        Command::Signal { action } => signal_command(&state, action),
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
        Command::DrainForUpgrade {
            reason,
            wait,
            timeout,
            cancel,
        } => {
            if cancel {
                println!(
                    "{}",
                    if crate::drain::clear(&state)? {
                        "withdrew the drain request"
                    } else {
                        "no drain request to withdraw"
                    }
                );
                return Ok(0);
            }
            let (daemon, pid, _, _) = serve::liveness(&state, serve::DEFAULT_STALE_AFTER_SECS);
            let pid = match (daemon, pid) {
                (serve::Liveness::Running | serve::Liveness::Stale, Some(pid)) => pid,
                _ => {
                    return Err(format!(
                        "no serve daemon is running for {} ({}); there is nothing to drain",
                        state.root.display(),
                        daemon.as_str()
                    ))
                }
            };
            let request = crate::drain::request(&state, pid, &reason)?;
            println!(
                "asked the serve daemon (pid {pid}) to drain at {}: it finishes the validations in flight, starts no new one, and exits 0",
                request.requested_at
            );
            if !wait {
                return Ok(0);
            }
            let started = std::time::Instant::now();
            loop {
                if !serve::pid_alive(pid) || serve::serve_lock_held(&state) == Some(false) {
                    println!(
                        "the serve daemon (pid {pid}) exited {}s after the request",
                        started.elapsed().as_secs()
                    );
                    return Ok(0);
                }
                if started.elapsed() >= std::time::Duration::from_secs(timeout) {
                    eprintln!(
                        "merge-queue drain-for-upgrade: the serve daemon (pid {pid}) is still running after {timeout}s; the request stands (withdraw it with --cancel)"
                    );
                    return Ok(1);
                }
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
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
                    prior: crate::decisions::prior_rates(&state.root),
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
