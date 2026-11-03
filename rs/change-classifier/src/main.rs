//! `change-classify`: classify a change's paths against a test-footprint policy.

#![forbid(unsafe_code)]

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use change_classifier::{
    changed_paths, check_vectors, classify, classify_known, glob_match, parse_known_nodes, Policy,
    VECTORS,
};
use clap::{Parser, Subcommand};

/// Classify a change's paths against a test-footprint policy (version 2): `skip` (every path
/// inert), `selective` (the selected nodes) or `full`, with the components touched and each
/// path's class.
#[derive(Parser)]
#[command(name = "change-classify", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Classify a change: the paths `--head` changes since its merge base with `--base`, or an
    /// explicit list.
    ///
    /// Examples:
    ///   change-classify classify --policy ci/test-footprints.json --base origin/main
    ///   git diff --name-only A B | change-classify classify --policy P --paths - --json
    ///   change-classify classify --policy P --base origin/main --known-nodes nodes.txt
    #[command(verbatim_doc_comment)]
    Classify {
        /// Policy file (a resolved footprint file, `version` 2).
        #[arg(long, value_name = "FILE")]
        policy: PathBuf,
        /// Base of the change: paths are those `--head` changed since its merge base with it
        /// (renames count at both ends).
        #[arg(long, value_name = "REV", conflicts_with = "paths")]
        base: Option<String>,
        /// Head of the change (with --base).
        #[arg(long, value_name = "REV", default_value = "HEAD")]
        head: String,
        /// Repository to diff in (with --base).
        #[arg(long, value_name = "DIR", default_value = ".")]
        repo: PathBuf,
        /// Changed paths; `-` reads them from standard input, one per line (a
        /// `git diff --name-status` listing is accepted too).
        #[arg(long, value_name = "PATH", num_args = 1..)]
        paths: Vec<String>,
        /// Known node names: refuse (exit status 3) when a matched rule names a node outside
        /// this list, naming each node and the rule, instead of classifying. FILE is a JSON
        /// array of strings or one node per line (blank lines and `#` comments skipped). Every
        /// matched rule is checked, whatever the verdict. Default: no check.
        #[arg(long, value_name = "FILE")]
        known_nodes: Option<PathBuf>,
        /// Print the classification as JSON (a refusal as `{"refused": "unknown-nodes",
        /// "unknown_nodes": [{"node", "rule"}, ...]}`).
        #[arg(long)]
        json: bool,
    },
    /// Whether PATH matches glob PATTERN: exit status 0 when it does, 1 when it does not.
    Glob {
        /// Glob (`**`, `*`, `?`, literal characters).
        pattern: String,
        /// Path, `/`-separated.
        path: String,
    },
    /// Check golden vectors (the shared contract for glob and rule-order semantics) against
    /// this build. Exit status 1 names each failing case.
    CheckVectors {
        /// Vectors file; default: the vectors this build ships.
        #[arg(value_name = "FILE")]
        file: Option<PathBuf>,
    },
    /// Print the getting-started guide.
    Quickstart,
    /// Print the complete user guide.
    Userguide,
}

/// Print to standard output; a reader that went away (`| head`) is not an error.
macro_rules! say {
    ($($arg:tt)*) => {{
        let _ = writeln!(std::io::stdout().lock(), $($arg)*);
    }};
}

fn run(cli: Cli) -> Result<ExitCode, String> {
    match cli.command {
        Command::Classify {
            policy,
            base,
            head,
            repo,
            paths,
            known_nodes,
            json,
        } => {
            let policy = Policy::load(&policy)?;
            let known = match &known_nodes {
                Some(file) => Some(
                    std::fs::read_to_string(file)
                        .map_err(|error| format!("cannot read {}: {error}", file.display()))
                        .and_then(|text| parse_known_nodes(&text))
                        .map_err(|error| format!("{}: {error}", file.display()))?,
                ),
                None => None,
            };
            let paths = match (base, paths.as_slice()) {
                (Some(base), _) => changed_paths(&repo, &base, &head)?,
                (None, [dash]) if dash == "-" => {
                    let mut text = String::new();
                    std::io::stdin()
                        .read_to_string(&mut text)
                        .map_err(|error| format!("cannot read standard input: {error}"))?;
                    change_classifier::parse_name_status(&text)
                }
                (None, []) => return Err("give --base REV or --paths PATH...".to_owned()),
                (None, list) => list.to_vec(),
            };
            let result = match &known {
                None => classify(&policy, &paths),
                Some(known) => match classify_known(&policy, &paths, known) {
                    Ok(result) => result,
                    Err(refusal) => {
                        if json {
                            let doc = serde_json::json!({
                                "refused": "unknown-nodes",
                                "unknown_nodes": refusal.unknown_nodes,
                            });
                            say!(
                                "{}",
                                serde_json::to_string_pretty(&doc)
                                    .map_err(|error| error.to_string())?
                            );
                        }
                        eprintln!("change-classify: {refusal}");
                        return Ok(ExitCode::from(3));
                    }
                },
            };
            if json {
                say!(
                    "{}",
                    serde_json::to_string_pretty(&result).map_err(|error| error.to_string())?
                );
            } else {
                say!("{}", result.verdict.as_str());
                if !result.components.is_empty() {
                    let components: Vec<&str> =
                        result.components.iter().map(String::as_str).collect();
                    say!("components: {}", components.join(", "));
                }
                if !result.nodes.is_empty() {
                    let nodes: Vec<&str> = result.nodes.iter().map(String::as_str).collect();
                    say!("nodes: {}", nodes.join(" "));
                }
                for reason in &result.reasons {
                    say!("  {reason}");
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Glob { pattern, path } => Ok(if glob_match(&pattern, &path) {
            ExitCode::SUCCESS
        } else {
            ExitCode::from(1)
        }),
        Command::CheckVectors { file } => {
            let text = match &file {
                Some(path) => std::fs::read_to_string(path)
                    .map_err(|error| format!("cannot read {}: {error}", path.display()))?,
                None => VECTORS.to_owned(),
            };
            let (checked, failures) = check_vectors(&text)?;
            for failure in &failures {
                say!("FAIL {}: {}", failure.case, failure.detail);
            }
            say!("{checked} case(s), {} failed", failures.len());
            Ok(if failures.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
        Command::Quickstart => {
            let _ = std::io::stdout().write_all(include_str!("embedded_quickstart.md").as_bytes());
            Ok(ExitCode::SUCCESS)
        }
        Command::Userguide => {
            let _ = std::io::stdout().write_all(include_str!("embedded_userguide.md").as_bytes());
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("change-classify: {error}");
            ExitCode::from(2)
        }
    }
}
