//! Command-line interface.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::git::{CombineMode, Git};
use crate::model::ItemStatus;
use crate::ops::{self, Submission};
use crate::plan::{render, short};
use crate::run::{self, RunOptions, RunReport, Runner};
use crate::state::{
    Config, GitHubConfig, IdentityConfig, LandConfig, RegenerateConfig, State, ValidatorConfig,
    DEFAULT_DEPTH, DEFAULT_MAX_BATCH, DEFAULT_PRIORITY,
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
  merge-queue widen";

/// Local merge queue: order, batch, validate and land ready changes while keeping the base green.
#[derive(Debug, Parser)]
#[command(name = "merge-queue", version, after_help = AFTER_HELP)]
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
        /// Generated-file path pattern the queue regenerates instead of treating its conflicts
        /// as real (exact path, `dir/`, or `*` wildcards). Repeatable.
        #[arg(long, value_name = "PATTERN")]
        regen_path: Vec<String>,
        /// Shell command, run at a checkout's root, that rewrites every generated file.
        #[arg(long, default_value = "")]
        regen_command: String,
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
        /// Repeatable.
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
            regen_path,
            regen_command,
            regen_when,
            exact_sha,
            trust_base,
            identity_name,
            identity_email,
            peer,
            github_repo,
            gh,
            api_per_hour,
            api_budget_file,
            anonymous_fetch,
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
            clean_ignored,
            land,
            land_command,
            allow_batches,
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
                regenerate: RegenerateConfig {
                    paths: regen_path,
                    command: regen_command,
                    when: regen_when,
                },
                exact_sha,
                trust_base,
                identity: IdentityConfig {
                    name: identity_name,
                    email: identity_email,
                },
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
                },
                land: LandConfig {
                    driver: land,
                    command: land_command,
                    allow_batches,
                    close_prs,
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
            by,
            title,
            tested_with,
            requires,
            policy,
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
                    title,
                    after,
                    solo,
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
                "queued #{} at {} (priority {}){claim}",
                item.pr,
                short(&item.head),
                item.priority
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
            let _lock = if dry_run { None } else { Some(state.lock()?) };
            let report = crate::sync::sync(&state, &config, &source, limit, dry_run)?;
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
                for (pr, why) in &report.refused {
                    println!("  refused #{pr}: {why}");
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
                        item.priority,
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
        } => {
            let _lock = state.lock()?;
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
                },
            )?;
            let report = runner.run()?;
            print_report(&report, json)?;
            Ok(if report.stopped.is_empty() { 0 } else { 3 })
        }
        Command::Widen { json } => {
            let _lock = state.lock()?;
            let report = run::widen(&state)?;
            print_report(&report, json)?;
            let red = report.validations.iter().any(|v| !v.green);
            Ok(if red { 3 } else { 0 })
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
                        Some(d) if d.green => format!("discharged green at {}", short(&d.commit)),
                        Some(d) => format!(
                            "complete run RED at {}: {}",
                            short(&d.commit),
                            d.failed.summary()
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
