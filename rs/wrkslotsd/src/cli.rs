//! Command-line interface for rebuilding and reading the observer index.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use serde::Serialize;

use crate::{read_index, rebuild_index, ReplaySummary};

#[derive(Parser)]
#[command(
    name = "wrkslotsd",
    version,
    about = "Validate slot event history and maintain a disposable read-only observer index",
    long_about = "Validate a hash-linked EVENTS.<machine> directory and materialize a disposable SQLite observer index. Replay covers path-independent record semantics; configuration-coupled checkout path, layout-default, and landed-ref policy remains with the originating registry. Accepted event JSON is limited to 16 MiB per event, 127 nested containers, Unicode scalar strings, exact i64/u64 integers, and finite binary64 floats; values outside that observer domain fail closed. Unknown event kinds, non-array imported holds, and non-object state evidence also fail closed even where the current Python reader ignores them; the Python writer always emits the stricter form. Initial publication requires a filesystem that supports renameat2(RENAME_NOREPLACE). This first slice has no daemon, socket, lease, cleanup, rescue, deletion, or repository mutation capability.",
    after_help = "Examples:\n  wrkslotsd rebuild --events-dir /srv/control/EVENTS.node-a --index /var/tmp/slot-observer.sqlite\n  wrkslotsd status --index /var/tmp/slot-observer.sqlite"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Strictly replay an event log and atomically replace the derived index
    Rebuild {
        /// Real EVENTS.<machine> directory to validate; it is never modified
        #[arg(long, value_name = "DIR")]
        events_dir: PathBuf,
        /// Disposable SQLite index path outside the event directory
        #[arg(long, value_name = "FILE")]
        index: PathBuf,
    },
    /// Read and cross-check summary counters from an existing derived index
    Status {
        /// Existing disposable SQLite index
        #[arg(long, value_name = "FILE")]
        index: PathBuf,
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
    let result = match command {
        Command::Rebuild { events_dir, index } => rebuild_index(&events_dir, &index),
        Command::Status { index } => read_index(&index),
    };
    match result {
        Ok(summary) => match serde_json::to_string(&CliSummary::from(&summary)) {
            Ok(encoded) => {
                println!("{encoded}");
                0
            }
            Err(error) => {
                eprintln!("wrkslotsd: cannot encode summary: {error}");
                1
            }
        },
        Err(error) => {
            eprintln!("wrkslotsd: {error}");
            1
        }
    }
}
