//! The CLI surface: `wrkviz inspect --output <archive>`.
//!
//! Hand-parsed, the way `wrkslotsd`'s surface is hand-matched to
//! its oracle: output and refusal spellings are the Python
//! edition's. Subcommands the Rust edition has not ported refuse
//! by name rather than falling back.

use crate::inspect::inspect;
use std::path::PathBuf;

/// The Python edition's top-level help, verbatim.
pub const TOP_HELP: &str = r#"usage: wrkviz [-h] [--version] [--userguide]
              {quickstart,ingest-project,ingest,ingest-claude,ingest-claude-history,ingest-muse,ingest-orc,summarize,build,extract-transcripts,export,refresh,github-metadata,refresh-claude,refresh-orc,serve,inspect,gc,migrate-build-state,migrate-snapshots,audit-glossary,audit-losslessness,query,responsiveness,discover-agents}
              ...

An agent timeline reconstruction, visualisation and summary tool. Turns
coordinator/subagent transcripts into a zoomable, multi-level summary site.

positional arguments:
  {quickstart,ingest-project,ingest,ingest-claude,ingest-claude-history,ingest-muse,ingest-orc,summarize,build,extract-transcripts,export,refresh,github-metadata,refresh-claude,refresh-orc,serve,inspect,gc,migrate-build-state,migrate-snapshots,audit-glossary,audit-losslessness,query,responsiveness,discover-agents}
    quickstart          print a short end-to-end example
    ingest-project      ingest registered project teams and extract
                        transcripts; zero tokens
    ingest              normalize Codex logs; do not call a model
    ingest-claude       normalize Claude logs; do not call a model
    ingest-claude-history
                        normalize the owner's Claude Code prompt history; do
                        not call a model
    ingest-muse         normalize a Muse session and its subagents; do not
                        call a model
    ingest-orc          snapshot and normalize Orc SQLite logs; do not call a
                        model
    summarize           fill structured summary cache misses
    build               regenerate Markdown/site from cached data; zero tokens
    extract-transcripts
                        write append-only prompt/response JSONL from
                        normalized logs, as seekable gzip with a sidecar
                        index; zero tokens
    export              build a zero-token website slice in a separate
                        directory
    refresh             idempotent ingest + summarize + build
    github-metadata     conditionally cache GitHub pull titles, then rebuild
                        the site
    refresh-claude      idempotent Claude ingest + summarize + build
    refresh-orc         idempotent Orc ingest + summarize + build
    serve               serve a built archive on localhost
    inspect             print archive/run/source counts as JSON
    gc                  report what a built archive no longer produces; delete
                        only when asked
    migrate-build-state
                        move intermediate build state out of the published
                        archive
    migrate-snapshots   move an archive's vendor source snapshots out of the
                        published tree
    audit-glossary      audit retired glossary candidates without publishing
                        links
    audit-losslessness  account for every vendor row against the archive; no
                        lock, no model call
    query               navigate a built timeline without starting the website
    responsiveness      measure how promptly an agentctl coordinator answers
                        its children; zero tokens
    discover-agents     find every agent session under a project's
                        coordinators, across harnesses; zero tokens

options:
  -h, --help            show this help message and exit
  --version             show program's version number and exit
  --userguide           print the complete embedded guide
"#;
/// The Python edition's inspect help, verbatim.
pub const INSPECT_HELP: &str = r#"usage: wrkviz inspect [-h] --output OUTPUT

print archive/run/source counts as JSON

options:
  -h, --help       show this help message and exit
  --output OUTPUT
"#;

const VERSION: &str = "wrkviz 0.1.0";

/// Expand a leading `~` the way `Path.expanduser` does (the
/// password database fallback is not modelled).
fn expand_user(raw: &str) -> PathBuf {
    if raw == "~" || raw.starts_with("~/") {
        if let Ok(home) = std::env::var("HOME") {
            if !home.is_empty() {
                let rest = raw.strip_prefix('~').unwrap_or("");
                let mut path = PathBuf::from(home);
                for part in rest.split('/').filter(|part| !part.is_empty()) {
                    path.push(part);
                }
                return path;
            }
        }
    }
    PathBuf::from(raw)
}

/// The path as Python's `Path(...)` spells it: leading `~`
/// expanded, `.` and empty components normalized away.
fn cli_path(raw: &str) -> PathBuf {
    let expanded = expand_user(raw);
    let mut out = PathBuf::new();
    for part in expanded.components() {
        match part {
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

/// Resolve an `--output`-style argument: `--name value`,
/// `--name=value`, or an unambiguous prefix of `--name`.
fn option_value(args: &[String], name: &str) -> Result<Option<String>, String> {
    let long = format!("--{name}");
    let mut found: Option<String> = None;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == &long {
            index += 1;
            match args.get(index) {
                Some(value) => found = Some(value.clone()),
                None => return Err(format!("argument --{name}: expected one argument")),
            }
        } else if let Some(value) = arg.strip_prefix(&format!("{long}=")) {
            found = Some(value.to_string());
        } else if arg.starts_with("--") && arg.len() > 2 && long.starts_with(arg.as_str()) {
            index += 1;
            match args.get(index) {
                Some(value) => found = Some(value.clone()),
                None => return Err(format!("argument --{name}: expected one argument")),
            }
        } else {
            return Err(format!("unrecognized argument: {arg}"));
        }
        index += 1;
    }
    Ok(found)
}

/// Run the CLI and return the process exit code.
pub fn run(args: &[String]) -> u8 {
    let Some(first) = args.first() else {
        print!("{TOP_HELP}");
        return 0;
    };
    match first.as_str() {
        "--help" | "-h" => {
            print!("{TOP_HELP}");
            0
        }
        "--version" => {
            println!("{VERSION}");
            0
        }
        "--userguide" => {
            eprintln!(
                "wrkviz: the Rust edition has not ported '--userguide' yet; the Python edition remains the oracle for it"
            );
            2
        }
        "inspect" => run_inspect(&args[1..]),
        other => {
            eprintln!(
                "wrkviz: the Rust edition has not ported '{other}' yet; the Python edition remains the oracle for it"
            );
            2
        }
    }
}

fn run_inspect(args: &[String]) -> u8 {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print!("{INSPECT_HELP}");
        return 0;
    }
    let output = match option_value(args, "output") {
        Ok(value) => value,
        Err(error) => {
            eprintln!("wrkviz: {error}");
            return 2;
        }
    };
    let Some(output) = output else {
        eprintln!("wrkviz: the following arguments are required: --output");
        return 2;
    };
    match inspect(&cli_path(&output)) {
        Ok(text) => {
            println!("{text}");
            0
        }
        Err(error) => {
            eprintln!("wrkviz: {error}");
            2
        }
    }
}
