//! `local-ci-hub` front door (Rust edition).
//!
//! M1 surface is read-only: `receipt show` judges one exact commit by
//! tailing the git least-common-denominator store through the disposable
//! index, and `ledger rows` / `ledger digest-rows` expose the folded
//! history for measurement. Every answer names the read authority and
//! cursor it folded. Exit codes: 0 qualified, 1 not-qualified or refused,
//! 2 no-result or cannot-tell, 75 could-not-run.

use clap::{Parser, Subcommand};
use local_ci_hub::deployment::{
    DeploymentConfig, DEFAULT_BINDING_TREE_FIELD, DEFAULT_EVIDENCE_HEAD_FIELD, DEFAULT_GIT_REF,
    DEFAULT_REPO,
};
use local_ci_hub::digest::receipt_digest_with;
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
#[command(
    name = "local-ci-hub",
    version,
    about,
    after_help = "EXAMPLES:\n  local-ci-hub --config deployment.json ledger digest-rows\n  local-ci-hub --repo . --git-ref HEAD ledger rows --terminals\nSee rs/local-ci-hub/README.md for every deployment flag."
)]
struct Cli {
    /// Deployment config file (JSON; see deployment.example.json).
    /// Precedence: explicit flag, then config, then built-in default.
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    /// Git repository holding the ledger shards (default: `.`;
    /// config key `repo`).
    #[arg(long, global = true)]
    repo: Option<PathBuf>,
    /// Ref (or commit) whose git objects are the read authority
    /// (default: `HEAD`; config key `git_ref`).
    #[arg(long, global = true)]
    git_ref: Option<String>,
    /// Evidence archive root holding `run-evidence/` objects
    /// (default: unset, no evidence source; config key
    /// `evidence_root`).
    #[arg(long, global = true)]
    evidence_root: Option<PathBuf>,
    /// `cell_results` field naming the validated tree for derived
    /// evidence bindings (default: `tree`; config key
    /// `binding_tree_field`).
    #[arg(long, global = true)]
    binding_tree_field: Option<String>,
    /// Extra base-evidence field names the predicate requires from
    /// schema 5 on (repeatable; config key `base_fields` replaces
    /// the whole list when no flag is given).
    #[arg(long, global = true)]
    base_field: Vec<String>,
    /// Producer tool names whose events are admission fixtures and are
    /// excluded after the union (repeatable; config key
    /// `fixture_producers` replaces the whole list when no flag is
    /// given).
    #[arg(long, global = true)]
    fixture_producer: Vec<String>,
    /// `cell_results` field naming the validated head for typed
    /// evidence canonicalization (default: `validated_head_sha`;
    /// config key `evidence_head_field`).
    #[arg(long, global = true)]
    evidence_head_field: Option<String>,
    /// Index file path (default: ephemeral in-memory index; config
    /// key `index`, `null` keeps the default).
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

/// Flag values after `--config` resolution: explicit flag, then
/// config, then built-in default.
struct Effective {
    repo: PathBuf,
    git_ref: String,
    evidence_root: Option<PathBuf>,
    binding_tree_field: String,
    base_fields: Vec<String>,
    fixture_producers: Vec<String>,
    evidence_head_field: String,
    index: Option<PathBuf>,
    json: bool,
}

fn effective(cli: &Cli) -> Result<Effective, String> {
    let config = match &cli.config {
        Some(path) => DeploymentConfig::load(path)?,
        None => DeploymentConfig::default(),
    };
    let repo = cli
        .repo
        .clone()
        .or_else(|| config.repo.map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(DEFAULT_REPO));
    let git_ref = cli
        .git_ref
        .clone()
        .or(config.git_ref)
        .unwrap_or_else(|| DEFAULT_GIT_REF.to_owned());
    let evidence_root = cli
        .evidence_root
        .clone()
        .or_else(|| config.evidence_root.map(PathBuf::from));
    let binding_tree_field = cli
        .binding_tree_field
        .clone()
        .or(config.binding_tree_field)
        .unwrap_or_else(|| DEFAULT_BINDING_TREE_FIELD.to_owned());
    let base_fields = if cli.base_field.is_empty() {
        config.base_fields
    } else {
        cli.base_field.clone()
    };
    let fixture_producers = if cli.fixture_producer.is_empty() {
        config.fixture_producers
    } else {
        cli.fixture_producer.clone()
    };
    let evidence_head_field = cli
        .evidence_head_field
        .clone()
        .or(config.evidence_head_field)
        .unwrap_or_else(|| DEFAULT_EVIDENCE_HEAD_FIELD.to_owned());
    let index = cli
        .index
        .clone()
        .or_else(|| config.index.map(PathBuf::from));
    Ok(Effective {
        repo,
        git_ref,
        evidence_root,
        binding_tree_field,
        base_fields,
        fixture_producers,
        evidence_head_field,
        index,
        json: cli.json,
    })
}

fn refusal_exit(refusal: &Refusal) -> ! {
    eprintln!("local-ci-hub: refused: {refusal}");
    std::process::exit(1);
}

fn load(eff: &Effective) -> Result<Loaded, String> {
    let authority = "git-lcd".to_owned();
    let mut store = GitStore::new(
        authority.clone(),
        Role::Record,
        eff.repo.clone(),
        &eff.git_ref,
    );
    if let Some(root) = &eff.evidence_root {
        store = store.with_evidence_root(root.clone());
    }
    let workspace_root = eff.repo.display().to_string();
    let mut index = match &eff.index {
        Some(path) => Index::open(path, &authority, &workspace_root).map_err(|e| format!("{e}"))?,
        None => Index::in_memory(&authority, &workspace_root).map_err(|e| format!("{e}"))?,
    };
    let start = index.cursor().map_err(|e| format!("{e}"))?;
    let config = ExpandConfig {
        binding_tree_field: eff.binding_tree_field.clone(),
    };
    let fixture_producers: BTreeSet<String> = eff.fixture_producers.iter().cloned().collect();
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
            let event = match &eff.evidence_root {
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

fn resolve_commit(eff: &Effective, commit: &str) -> String {
    if commit != "HEAD" && commit != "@" {
        return commit.to_owned();
    }
    let output = ProcCommand::new("git")
        .arg("-C")
        .arg(&eff.repo)
        .args(["rev-parse", "HEAD"])
        .output();
    match output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim().to_owned(),
        _ => commit.to_owned(),
    }
}

fn policy(eff: &Effective) -> Policy {
    let mut policy = Policy::default();
    for field in &eff.base_fields {
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
    let eff = match effective(&cli) {
        Ok(eff) => eff,
        Err(error) => {
            eprintln!("local-ci-hub: {error}");
            std::process::exit(2);
        }
    };
    let loaded = match load(&eff) {
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
            let commit = resolve_commit(&eff, commit);
            let rows = match loaded.index.rows_for_commit(&commit) {
                Ok(rows) => rows,
                Err(error) => {
                    eprintln!("local-ci-hub: {error}");
                    std::process::exit(2);
                }
            };
            let assessment = assess(&rows, &commit, &policy(&eff));
            let verdict_text = match assessment.verdict {
                Verdict::Qualified => "qualified",
                Verdict::NotQualified => "not-qualified",
                Verdict::NoResult => "no-result",
            };
            if eff.json {
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
            let mut evidence_records: usize = 0;
            let mut head_field_seen = false;
            if let Err(error) = loaded.index.for_each_row(|row| {
                if let Some(cell_results) = row.get("cell_results") {
                    if cell_results.is_object() {
                        evidence_records += 1;
                        if cell_results.get(&eff.evidence_head_field).is_some() {
                            head_field_seen = true;
                        }
                    }
                }
                let object = PyValue::Object(vec![
                    (
                        "run_id".to_owned(),
                        row.get("run_id").cloned().unwrap_or(PyValue::Null),
                    ),
                    (
                        "digest".to_owned(),
                        match receipt_digest_with(&row, &eff.evidence_head_field) {
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
            if evidence_records > 0 && !head_field_seen {
                eprintln!(
                    "local-ci-hub: warning: evidence-head field '{}' absent from all \
                     {evidence_records} typed cell_results record(s); digests used the \
                     generic sorted form — check --evidence-head-field against the deployment",
                    eff.evidence_head_field
                );
            }
        }
        Command::Ledger {
            action: LedgerAction::Verdicts,
        } => {
            let active_policy = policy(&eff);
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
