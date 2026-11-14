//! `local-ci-hub` front door (Rust edition).
//!
//! M1 surface is read-only: `receipt show` judges one exact commit by
//! tailing the git least-common-denominator store through the disposable
//! index, and `ledger rows` / `ledger digest-rows` expose the folded
//! history for measurement. Every answer names the read authority and
//! cursor it folded. Exit codes: 0 qualified, 1 not-qualified or refused,
//! 2 no-result or cannot-tell, 75 could-not-run.

use clap::{Parser, Subcommand};
use local_ci_hub::digest::receipt_digest;
use local_ci_hub::event::{check_events, is_fixture_event, union, Refusal};
use local_ci_hub::expand::{expand_line_with, ExpandConfig, FileEvidence, NoEvidence};
use local_ci_hub::fold::terminal_row;
use local_ci_hub::index::{Cursor, Index};
use local_ci_hub::pyjson::{dumps_sorted, PyValue};
use local_ci_hub::store::{GitStore, ReadLimits, Role, Store};
use local_ci_hub::verdict::{assess, Policy, Verdict};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command as ProcCommand;

/// Local CI hub: runs, ledger, receipts, remotes, health, and landing locks.
#[derive(Parser)]
#[command(name = "local-ci-hub", version, about)]
struct Cli {
    /// Git repository holding the ledger shards.
    #[arg(long, global = true, default_value = ".")]
    repo: PathBuf,
    /// Ref (or commit) whose git objects are the read authority.
    #[arg(long, global = true, default_value = "HEAD")]
    git_ref: String,
    /// Evidence archive root holding `run-evidence/` objects.
    #[arg(long, global = true)]
    evidence_root: Option<PathBuf>,
    /// `cell_results` field naming the validated tree for derived
    /// evidence bindings.
    #[arg(long, global = true, default_value = "tree")]
    binding_tree_field: String,
    /// Extra base-evidence field names the predicate requires from
    /// schema 5 on (repeatable).
    #[arg(long, global = true)]
    base_field: Vec<String>,
    /// Producer tool names whose events are admission fixtures and are
    /// excluded after the union (repeatable).
    #[arg(long, global = true)]
    fixture_producer: Vec<String>,
    /// Index file path (default: ephemeral in-memory index).
    #[arg(long, global = true)]
    index: Option<PathBuf>,
    /// Print JSON instead of the human summary.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Receipt verdicts for exact commits.
    Receipt {
        #[command(subcommand)]
        action: ReceiptAction,
    },
    /// Folded ledger history.
    Ledger {
        #[command(subcommand)]
        action: LedgerAction,
    },
}

#[derive(Subcommand)]
enum ReceiptAction {
    /// Show the verdict for COMMIT (exact id, or HEAD).
    Show {
        /// Exact commit id to judge.
        commit: String,
    },
}

#[derive(Subcommand)]
enum LedgerAction {
    /// Print folded rows, one sorted-keys JSON object per line. With
    /// `--terminals`, print sorted terminal-field lines instead.
    Rows {
        /// Print sorted terminal-field lines (run_id, commit, result,
        /// host, finished_at) instead of full rows.
        #[arg(long)]
        terminals: bool,
    },
    /// Print one `{"digest": ..., "run_id": ...}` object per folded row.
    DigestRows,
    /// Print one verdict object per distinct row commit, sorted by commit.
    Verdicts,
}

struct Loaded {
    index: Index,
    cursor: Cursor,
    authority: String,
}

fn refusal_exit(refusal: &Refusal) -> ! {
    eprintln!("local-ci-hub: refused: {refusal}");
    std::process::exit(1);
}

fn load(cli: &Cli) -> Result<Loaded, String> {
    let authority = "git-lcd".to_owned();
    let mut store = GitStore::new(
        authority.clone(),
        Role::Record,
        cli.repo.clone(),
        &cli.git_ref,
    );
    if let Some(root) = &cli.evidence_root {
        store = store.with_evidence_root(root.clone());
    }
    let workspace_root = cli.repo.display().to_string();
    let mut index = match &cli.index {
        Some(path) => Index::open(path, &authority, &workspace_root).map_err(|e| format!("{e}"))?,
        None => Index::in_memory(&authority, &workspace_root).map_err(|e| format!("{e}"))?,
    };
    let start = index.cursor().map_err(|e| format!("{e}"))?;
    let config = ExpandConfig {
        binding_tree_field: cli.binding_tree_field.clone(),
    };
    let fixture_producers: BTreeSet<String> = cli.fixture_producer.iter().cloned().collect();
    let mut filtered_fixtures: usize = 0;
    let mut cursor = start;
    // Tail the authority page by page, admitting each page to the index
    // as it arrives (one page in memory at a time), then refold once.
    // Pages are expanded and envelope-checked before admission, so
    // refusals surface before their page is stored.
    loop {
        let page = store
            .read_from(&cursor, ReadLimits::default())
            .map_err(|e| format!("{e}"))?;
        if let Some(reason) = page.cursor_reset {
            eprintln!("local-ci-hub: cursor_reset {}", reason.as_wire());
        }
        let mut expanded = Vec::with_capacity(page.events.len());
        for shard_event in &page.events {
            let event = match &cli.evidence_root {
                Some(root) => {
                    expand_line_with(&shard_event.line, &FileEvidence::new(root.clone()), &config)
                }
                None => expand_line_with(&shard_event.line, &NoEvidence, &config),
            }
            .map_err(|r| format!("{r}"))?;
            expanded.push(event);
        }
        let mut unioned = union(expanded).map_err(|r| format!("{r}"))?;
        if !fixture_producers.is_empty() {
            let before = unioned.len();
            unioned.retain(|event| !is_fixture_event(event, &fixture_producers));
            filtered_fixtures += before - unioned.len();
        }
        if let Err(refusal) = check_events(&unioned) {
            refusal_exit(&refusal);
        }
        let next_cursor = page.next_cursor.clone();
        index
            .append_batch(&unioned, &next_cursor)
            .map_err(|e| format!("{e}"))?;
        cursor = next_cursor;
        if !page.more {
            break;
        }
    }
    if filtered_fixtures > 0 {
        eprintln!("local-ci-hub: filtered {filtered_fixtures} fixture events");
    }
    if index.needs_refold().map_err(|e| format!("{e}"))? {
        index.refold().map_err(|e| format!("{e}"))?;
    }
    Ok(Loaded {
        index,
        cursor,
        authority,
    })
}

fn resolve_commit(cli: &Cli, commit: &str) -> String {
    if commit != "HEAD" && commit != "@" {
        return commit.to_owned();
    }
    let output = ProcCommand::new("git")
        .arg("-C")
        .arg(&cli.repo)
        .args(["rev-parse", "HEAD"])
        .output();
    match output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim().to_owned(),
        _ => commit.to_owned(),
    }
}

fn policy(cli: &Cli) -> Policy {
    let mut policy = Policy::default();
    for field in &cli.base_field {
        if !policy.base_fields.contains(field) {
            policy.base_fields.push(field.clone());
        }
    }
    policy
}

fn main() {
    let cli = Cli::parse();
    let Some(command) = &cli.command else {
        println!("local-ci-hub {} (see --help)", env!("CARGO_PKG_VERSION"));
        return;
    };
    let loaded = match load(&cli) {
        Ok(loaded) => loaded,
        Err(error) => {
            eprintln!("local-ci-hub: {error}");
            std::process::exit(2);
        }
    };
    match command {
        Command::Receipt {
            action: ReceiptAction::Show { commit },
        } => {
            let commit = resolve_commit(&cli, commit);
            let rows = match loaded.index.rows_for_commit(&commit) {
                Ok(rows) => rows,
                Err(error) => {
                    eprintln!("local-ci-hub: {error}");
                    std::process::exit(2);
                }
            };
            let assessment = assess(&rows, &commit, &policy(&cli));
            let verdict_text = match assessment.verdict {
                Verdict::Qualified => "qualified",
                Verdict::NotQualified => "not-qualified",
                Verdict::NoResult => "no-result",
            };
            if cli.json {
                let object = PyValue::Object(vec![
                    (
                        "schema".to_owned(),
                        PyValue::Str("local-ci-hub/receipt/v1".to_owned()),
                    ),
                    ("commit".to_owned(), PyValue::Str(commit.clone())),
                    ("verdict".to_owned(), PyValue::Str(verdict_text.to_owned())),
                    (
                        "qualified".to_owned(),
                        PyValue::Bool(assessment.qualified()),
                    ),
                    ("reason".to_owned(), PyValue::Str(assessment.reason.clone())),
                    (
                        "authority".to_owned(),
                        PyValue::Str(loaded.authority.clone()),
                    ),
                    ("cursor".to_owned(), loaded.cursor.to_pyvalue()),
                    ("rows".to_owned(), PyValue::Int(rows.len().to_string())),
                ]);
                println!("{}", dumps_sorted(&object));
            } else {
                println!("{commit}: {verdict_text} ({})", assessment.reason);
                println!(
                    "authority {} at head {} ({} row(s))",
                    loaded.authority,
                    loaded.cursor.head,
                    rows.len()
                );
            }
            std::process::exit(match assessment.verdict {
                Verdict::Qualified => 0,
                Verdict::NotQualified => 1,
                Verdict::NoResult => 2,
            });
        }
        Command::Ledger {
            action: LedgerAction::Rows { terminals },
        } => {
            if *terminals {
                // Terminal lines are the working set; rows stream past.
                let mut lines: Vec<String> = Vec::new();
                if let Err(error) = loaded.index.for_each_row(|row| {
                    lines.push(dumps_sorted(&terminal_row(&row)));
                    Ok(())
                }) {
                    eprintln!("local-ci-hub: {error}");
                    std::process::exit(2);
                }
                lines.sort();
                for line in &lines {
                    println!("{line}");
                }
            } else if let Err(error) = loaded.index.for_each_row_line(|line| {
                println!("{line}");
                Ok(())
            }) {
                eprintln!("local-ci-hub: {error}");
                std::process::exit(2);
            }
        }
        Command::Ledger {
            action: LedgerAction::DigestRows,
        } => {
            if let Err(error) = loaded.index.for_each_row(|row| {
                let object = PyValue::Object(vec![
                    (
                        "run_id".to_owned(),
                        row.get("run_id").cloned().unwrap_or(PyValue::Null),
                    ),
                    (
                        "digest".to_owned(),
                        match receipt_digest(&row) {
                            Some(digest) => PyValue::Str(digest),
                            None => PyValue::Null,
                        },
                    ),
                ]);
                println!("{}", dumps_sorted(&object));
                Ok(())
            }) {
                eprintln!("local-ci-hub: {error}");
                std::process::exit(2);
            }
        }
        Command::Ledger {
            action: LedgerAction::Verdicts,
        } => {
            let active_policy = policy(&cli);
            if let Err(error) = loaded.index.for_each_commit_rows(|commit, commit_rows| {
                let assessment = assess(&commit_rows, &commit, &active_policy);
                let verdict_text = match assessment.verdict {
                    Verdict::Qualified => "qualified",
                    Verdict::NotQualified => "not-qualified",
                    Verdict::NoResult => "no-result",
                };
                let object = PyValue::Object(vec![
                    ("commit".to_owned(), PyValue::Str(commit)),
                    (
                        "qualified".to_owned(),
                        PyValue::Bool(assessment.qualified()),
                    ),
                    ("reason".to_owned(), PyValue::Str(assessment.reason)),
                    ("verdict".to_owned(), PyValue::Str(verdict_text.to_owned())),
                ]);
                println!("{}", dumps_sorted(&object));
                Ok(())
            }) {
                eprintln!("local-ci-hub: {error}");
                std::process::exit(2);
            }
        }
    }
}
