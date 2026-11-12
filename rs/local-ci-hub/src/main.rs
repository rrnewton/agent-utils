//! `local-ci-hub` front door (Rust edition, M1: read-only receipt path
//! lands with the store slices; this binary currently exposes version and
//! help so the crate ships one executable from the first slice).

use clap::{Parser, Subcommand};

/// Local CI hub: runs, ledger, receipts, remotes, health, and landing locks.
#[derive(Parser)]
#[command(name = "local-ci-hub", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Show the receipt verdict for one exact commit.
    Receipt {
        #[command(subcommand)]
        action: ReceiptAction,
    },
}

#[derive(Subcommand)]
enum ReceiptAction {
    /// Print the verdict for COMMIT (exit 0 qualified, 1 not-qualified,
    /// 2 no-result).
    Show {
        /// Exact commit id to judge.
        commit: String,
    },
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Receipt {
            action: ReceiptAction::Show { commit },
        }) => {
            eprintln!(
                "local-ci-hub: receipt show for {commit} is not wired in this slice; \
                 the read path lands with the store connector"
            );
            std::process::exit(2);
        }
        None => {
            println!(
                "local-ci-hub {} (Rust core slice; see --help)",
                env!("CARGO_PKG_VERSION")
            );
        }
    }
}
