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

/// The Python edition's query help, verbatim.
pub const QUERY_HELP: &str = r#"usage: wrkviz query [-h] --output OUTPUT [--format {json,jsonl,markdown,text}]
                    {list,show,search,prompts,messages,stats} ...

navigate a built timeline without starting the website

positional arguments:
  {list,show,search,prompts,messages,stats}
    list                list concise records and stable references
    show                resolve one stable reference
    search              search prompts, responses, full transcript text, and
                        summaries
    prompts             list verbatim human-authored prompts in global
                        timestamp order
    messages            list prompts and their mechanically associated
                        responses
    stats               count prompt, response, and generated-summary text

options:
  -h, --help            show this help message and exit
  --output OUTPUT       built single- or multi-team archive
  --format {json,jsonl,markdown,text}
                        response format (default: json)
"#;

/// The Python edition's query-list help, verbatim.
pub const QUERY_LIST_HELP: &str = r#"usage: wrkviz query list [-h] [--team TEAM] [--start-time START_TIME]
                         [--end-time END_TIME]
                         [--kind {hourly,daily,weekly,monthly,quarterly}]
                         [--agent AGENT]
                         {teams,agents,phases,rollups}

list concise records and stable references

positional arguments:
  {teams,agents,phases,rollups}

options:
  -h, --help            show this help message and exit
  --team TEAM           team slug to include; repeat to select multiple teams
  --start-time START_TIME
                        first RFC3339 instant to overlap (inclusive)
  --end-time END_TIME   exclusive RFC3339 overlap boundary
  --kind {hourly,daily,weekly,monthly,quarterly}
                        rollup kind to include; repeat as needed
  --agent AGENT         canonical agent:TEAM::ID reference to select
"#;

/// The Python edition's query-show help, verbatim.
pub const QUERY_SHOW_HELP: &str = r#"usage: wrkviz query show [-h] [--transcript] reference

resolve one stable reference

positional arguments:
  reference     team:, agent:, phase:, rollup:, message:, or tool: reference

options:
  -h, --help    show this help message and exit
  --transcript  include condensed transcript messages when showing a work
                phase
"#;

/// The Python edition's query-stats help, verbatim.
pub const QUERY_STATS_HELP: &str = r#"usage: wrkviz query stats [-h] [--team TEAM] [--start-time START_TIME]
                          [--end-time END_TIME]
                          [--kind {hourly,daily,weekly,monthly,quarterly}]

count records, whitespace-delimited words, and UTF-8 text bytes for attributed
and unattributed prompts, responses, and generated summaries; read-only and
zero-model

options:
  -h, --help            show this help message and exit
  --team TEAM           team slug; repeat as needed
  --start-time START_TIME
                        inclusive RFC3339 instant
  --end-time END_TIME   exclusive RFC3339 instant
  --kind {hourly,daily,weekly,monthly,quarterly}
                        rollup kind to count; repeat as needed
"#;

/// The Python edition's query-prompts help, verbatim.
pub const QUERY_PROMPTS_HELP: &str = r#"usage: wrkviz query prompts [-h] [--range ORDINAL_RANGE]
                            [--which {human,bot,all}] [--limit N] [--tail N]
                            [--verify] [--team TEAM] [--start-time START_TIME]
                            [--end-time END_TIME]

list verbatim human-authored prompts in global timestamp order

options:
  -h, --help            show this help message and exit
  --range ORDINAL_RANGE
                        one prompt ordinal or inclusive range, for example
                        200-300
  --which {human,bot,all}
                        select human, bot, or all prompt authorship (default:
                        human)
  --limit N             return at most N records from the start of the
                        selection
  --tail N              return the last N records of the selection
  --verify              re-read every consulted projection end to end and
                        reproduce its manifest digest, and resolve
                        prompt/response linkage by full scan rather than seek
  --team TEAM           team slug; repeat as needed
  --start-time START_TIME
                        inclusive RFC3339 instant
  --end-time END_TIME   exclusive RFC3339 instant
"#;

/// The Python edition's query-messages help, verbatim.
pub const QUERY_MESSAGES_HELP: &str = r#"usage: wrkviz query messages [-h] [--range ORDINAL_RANGE]
                             [--which {human,bot,all}] [--limit N] [--tail N]
                             [--verify] [--team TEAM]
                             [--start-time START_TIME] [--end-time END_TIME]

list prompts and their mechanically associated responses

options:
  -h, --help            show this help message and exit
  --range ORDINAL_RANGE
                        one prompt ordinal or inclusive range, for example
                        200-300
  --which {human,bot,all}
                        select human, bot, or all prompt authorship (default:
                        human)
  --limit N             return at most N records from the start of the
                        selection
  --tail N              return the last N records of the selection
  --verify              re-read every consulted projection end to end and
                        reproduce its manifest digest, and resolve
                        prompt/response linkage by full scan rather than seek
  --team TEAM           team slug; repeat as needed
  --start-time START_TIME
                        inclusive RFC3339 instant
  --end-time END_TIME   exclusive RFC3339 instant
"#;

const QUERY_PROMPTS_USAGE: &str = "usage: wrkviz query prompts [-h] [--range ORDINAL_RANGE]\n                            [--which {human,bot,all}] [--limit N] [--tail N]\n                            [--verify] [--team TEAM] [--start-time START_TIME]\n                            [--end-time END_TIME]";
const QUERY_MESSAGES_USAGE: &str = "usage: wrkviz query messages [-h] [--range ORDINAL_RANGE]\n                             [--which {human,bot,all}] [--limit N] [--tail N]\n                             [--verify] [--team TEAM]\n                             [--start-time START_TIME] [--end-time END_TIME]";

/// The Python edition's query-search help, verbatim.
pub const QUERY_SEARCH_HELP: &str = r#"usage: wrkviz query search [-h]
                           [--in {owner-prompts,agent-responses,all-transcript}]
                           [--scope {summaries,transcripts,all}]
                           [--match {smart,phrase,literal}]
                           [--sort {relevance,newest,oldest}]
                           [--prompt-author {any,owner,agent,unclassified}]
                           [--linkage {any,linked,unlinked}]
                           [--role {user,assistant,agent,system,external,goal,tool,event}]
                           [--case-sensitive] [--offset OFFSET]
                           [--limit LIMIT] [--team TEAM]
                           [--start-time START_TIME] [--end-time END_TIME]
                           [--kind {hourly,daily,weekly,monthly,quarterly}]
                           [--agent AGENT]
                           text

search the canonical transcript corpus with stable message references, or use
--scope for compatibility phase-transcript search

positional arguments:
  text                  text or pattern to find

options:
  -h, --help            show this help message and exit
  --in {owner-prompts,agent-responses,all-transcript}
                        search-v2 corpus: owner prompts, agent responses, or
                        all transcript
  --scope {summaries,transcripts,all}
                        compatibility search scope (default when --in is
                        absent: summaries)
  --match {smart,phrase,literal}
  --sort {relevance,newest,oldest}
  --prompt-author {any,owner,agent,unclassified}
  --linkage {any,linked,unlinked}
  --role {user,assistant,agent,system,external,goal,tool,event}
  --case-sensitive
  --offset OFFSET
  --limit LIMIT
  --team TEAM           team slug to include; repeat to select multiple teams
  --start-time START_TIME
                        first RFC3339 instant to overlap (inclusive)
  --end-time END_TIME   exclusive RFC3339 overlap boundary
  --kind {hourly,daily,weekly,monthly,quarterly}
                        rollup kind to include; repeat as needed
  --agent AGENT         canonical agent:TEAM::ID reference to select
"#;

const QUERY_SEARCH_USAGE: &str = "usage: wrkviz query search [-h]\n                           [--in {owner-prompts,agent-responses,all-transcript}]\n                           [--scope {summaries,transcripts,all}]\n                           [--match {smart,phrase,literal}]\n                           [--sort {relevance,newest,oldest}]\n                           [--prompt-author {any,owner,agent,unclassified}]\n                           [--linkage {any,linked,unlinked}]\n                           [--role {user,assistant,agent,system,external,goal,tool,event}]\n                           [--case-sensitive] [--offset OFFSET]\n                           [--limit LIMIT] [--team TEAM]\n                           [--start-time START_TIME] [--end-time END_TIME]\n                           [--kind {hourly,daily,weekly,monthly,quarterly}]\n                           [--agent AGENT]\n                           text";

const QUERY_USAGE: &str = "usage: wrkviz query [-h] --output OUTPUT [--format {json,jsonl,markdown,text}]\n                    {list,show,search,prompts,messages,stats} ...";
const QUERY_STATS_USAGE: &str = "usage: wrkviz query stats [-h] [--team TEAM] [--start-time START_TIME]\n                          [--end-time END_TIME]\n                          [--kind {hourly,daily,weekly,monthly,quarterly}]";
const QUERY_SHOW_USAGE: &str = "usage: wrkviz query show [-h] [--transcript] reference";
const QUERY_LIST_USAGE: &str = "usage: wrkviz query list [-h] [--team TEAM] [--start-time START_TIME]\n                         [--end-time END_TIME]\n                         [--kind {hourly,daily,weekly,monthly,quarterly}]\n                         [--agent AGENT]\n                         {teams,agents,phases,rollups}";

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
        "query" => run_query(&args[1..]),
        "serve" => run_serve(&args[1..]),
        "quickstart" => run_quickstart(&args[1..]),
        "extract-transcripts" => run_extract_transcripts(&args[1..]),
        other => {
            eprintln!(
                "wrkviz: the Rust edition has not ported '{other}' yet; the Python edition remains the oracle for it"
            );
            2
        }
    }
}

/// The Python edition's quickstart help, verbatim.
pub const QUICKSTART_HELP: &str = r#"usage: wrkviz quickstart [-h]

print a short end-to-end example

options:
  -h, --help  show this help message and exit
"#;

const QUICKSTART_USAGE: &str = "usage: wrkviz quickstart [-h]";

fn run_quickstart(args: &[String]) -> u8 {
    let mut extras: Vec<String> = Vec::new();
    for arg in args {
        let help_plain = arg == "-h"
            || (arg.len() > 2
                && arg.starts_with("--")
                && !arg.contains('=')
                && "--help".starts_with(arg.as_str()));
        if help_plain {
            print!("{QUICKSTART_HELP}");
            return 0;
        }
        if let Some((head, value)) = arg.split_once('=') {
            if head.len() > 2 && head.starts_with("--") && "--help".starts_with(head) {
                return argparse_error(
                    QUICKSTART_USAGE,
                    "wrkviz quickstart",
                    &format!("argument -h/--help: ignored explicit argument '{value}'"),
                );
            }
        }
        extras.push(arg.clone());
    }
    if !extras.is_empty() {
        eprintln!(
            "usage: wrkviz [-h] [--version] [--userguide]\n              {{quickstart,ingest-project,ingest,ingest-claude,ingest-claude-history,ingest-muse,ingest-orc,summarize,build,extract-transcripts,export,refresh,github-metadata,refresh-claude,refresh-orc,serve,inspect,gc,migrate-build-state,migrate-snapshots,audit-glossary,audit-losslessness,query,responsiveness,discover-agents}}\n              ...\nwrkviz: error: unrecognized arguments: {}",
            extras.join(" ")
        );
        return 2;
    }
    let model = std::env::var("WRKVIZ_MODEL").unwrap_or_else(|_| "gpt-5.5".to_string());
    print!(
        "wrkviz v0.1.0\n\nBuild a version-controllable, multi-resolution website from an agent team's append-only logs.\n\n1. Ingest, summarize only cache misses, and build the site:\n\n   wrkviz refresh \\\n     --root-session SESSION_UUID --team example-team \\\n     --output ./timelines/example-team \\\n     --timezone America/New_York --model {model}\n\n2. Open it locally:\n\n   cd ./timelines/example-team\n   make serve\n   # http://127.0.0.1:8765/\n\n3. Rerun the same refresh later. New log records are normalized; content-addressed summaries are\n   reused; only changed time windows and calendar rollups spend model tokens.\n\n4. See what a summarize run would spend before it spends anything, and cap it:\n\n   wrkviz summarize --team example-team --output ./timelines/example-team      --price input=2,output=10 --plan\n   wrkviz summarize --team example-team --output ./timelines/example-team      --price input=2,output=10 --budget-usd 20\n\n   A run estimated over the budget refuses to start; one that reaches it anyway stops, marks\n   summary_data/summarize_state.json partial, and the next run resumes from the cache.\n\nThe stages are separate on purpose: `ingest`, `summarize`, and `build`. The `build` command never\ncalls a model, so formatting and UI changes can be regenerated for free.\n"
    );
    println!();
    0
}

/// The Python edition's extract-transcripts help, verbatim.
pub const EXTRACT_HELP: &str = r#"usage: wrkviz extract-transcripts [-h] --output OUTPUT [--team TEAM]

write append-only prompt/response JSONL from normalized logs, as seekable gzip
with a sidecar index; zero tokens. Run it once on an archive an older wrkviz
extracted: it migrates the plain .jsonl corpus in place, which prompts,
messages and stats queries need before they will read it.

options:
  -h, --help       show this help message and exit
  --output OUTPUT  durable archive directory
  --team TEAM      ingested team slug to include; repeat as needed (default:
                   all teams)
"#;

const EXTRACT_USAGE: &str = "usage: wrkviz extract-transcripts [-h] --output OUTPUT [--team TEAM]";

fn run_extract_transcripts(args: &[String]) -> u8 {
    let mut output: Option<String> = None;
    let mut teams: Vec<String> = Vec::new();
    let mut extras: Vec<String> = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        let help_plain = arg == "-h"
            || (arg.len() > 2
                && arg.starts_with("--")
                && !arg.contains('=')
                && "--help".starts_with(arg.as_str()));
        if help_plain {
            print!("{EXTRACT_HELP}");
            return 0;
        }
        if let Some((head, value)) = arg.split_once('=') {
            if head.len() > 2 && head.starts_with("--") && "--help".starts_with(head) {
                return argparse_error(
                    EXTRACT_USAGE,
                    "wrkviz extract-transcripts",
                    &format!("argument -h/--help: ignored explicit argument '{value}'"),
                );
            }
        }
        let mut handled = true;
        for name in ["--output", "--team"] {
            match named_value(args, &mut index, name) {
                Ok(Some(value)) => {
                    if name == "--output" {
                        output = Some(value);
                    } else {
                        teams.push(value);
                    }
                    break;
                }
                Ok(None) => {}
                Err((_, message)) => {
                    return argparse_error(EXTRACT_USAGE, "wrkviz extract-transcripts", &message)
                }
            }
        }
        if !is_option_token(arg, &["--output", "--team"]) {
            handled = false;
        }
        if !handled {
            extras.push(arg.clone());
        }
        index += 1;
    }
    let Some(output) = output else {
        return argparse_error(
            EXTRACT_USAGE,
            "wrkviz extract-transcripts",
            "the following arguments are required: --output",
        );
    };
    if !extras.is_empty() {
        eprintln!(
            "usage: wrkviz [-h] [--version] [--userguide]\n              {{quickstart,ingest-project,ingest,ingest-claude,ingest-claude-history,ingest-muse,ingest-orc,summarize,build,extract-transcripts,export,refresh,github-metadata,refresh-claude,refresh-orc,serve,inspect,gc,migrate-build-state,migrate-snapshots,audit-glossary,audit-losslessness,query,responsiveness,discover-agents}}\n              ...\nwrkviz: error: unrecognized arguments: {}",
            extras.join(" ")
        );
        return 2;
    }
    let archive = cli_path(&output);
    let started = crate::pipeline::utc_now();
    let mut command: Vec<String> = vec!["wrkviz".to_string(), "extract-transcripts".to_string()];
    command.extend(args.iter().cloned());
    let selected: Result<Vec<String>, String> = (|| {
        if !teams.is_empty() {
            let unique: std::collections::BTreeSet<&String> = teams.iter().collect();
            if unique.len() != teams.len() {
                return Err("--team must not repeat a team slug".to_string());
            }
            return Ok(teams.clone());
        }
        let discovered = crate::build_store::ingested_team_slugs(&archive)?;
        if discovered.is_empty() {
            return Err(format!("no ingested teams found in {}", archive.display()));
        }
        Ok(discovered)
    })();
    let selected = match selected {
        Ok(selected) => selected,
        Err(error) => {
            eprintln!("wrkviz: {error}");
            return 2;
        }
    };
    let report = match crate::transcript_export::extract_transcripts_archive(&archive, &selected) {
        Ok(report) => report,
        Err(error) => {
            if let Ok(run_path) = crate::pipeline::record_run(
                &archive,
                &crate::pipeline::RunReceipt {
                    command: &command,
                    started_at: &started,
                    status: "failed",
                    team_slug: &selected[0],
                    error: Some(&error),
                    team_slugs: &selected,
                    mechanical: None,
                },
            ) {
                eprintln!("run metadata: {}", run_path.display());
            }
            eprintln!("wrkviz: {error}");
            return 2;
        }
    };
    let partiality = report.partiality_summary();
    let mechanical = {
        let mut value = serde_json::Map::new();
        value.insert("transcript_extraction".to_string(), report.to_json());
        serde_json::Value::Object(value)
    };
    let run_path = match crate::pipeline::record_run(
        &archive,
        &crate::pipeline::RunReceipt {
            command: &command,
            started_at: &started,
            status: if partiality.is_some() {
                "failed"
            } else {
                "completed"
            },
            team_slug: &selected[0],
            error: partiality.as_deref(),
            team_slugs: &selected,
            mechanical: Some(mechanical),
        },
    ) {
        Ok(path) => path,
        Err(error) => {
            eprintln!("wrkviz: {error}");
            return 2;
        }
    };
    for skip in &report.skipped_teams {
        eprintln!(
            "wrkviz: transcripts: team {} -- carried forward unchanged from its last good extraction; nothing since is projected",
            skip.summary()
        );
    }
    for dropped in &report.dropped_authorship_rules {
        eprintln!(
            "wrkviz: transcripts: prompt authorship rule {} -- not applied",
            dropped.summary()
        );
    }
    let skipped_note = if report.skipped_teams.is_empty() {
        String::new()
    } else {
        format!(
            " ({} further team(s) carried forward unread)",
            report.skipped_teams.len()
        )
    };
    println!(
        "transcripts: {} prompts, {} responses, {} system inputs across {} teams{}; {} historical records retained, {} source records reclassified, {} files changed",
        report.prompts,
        report.responses,
        report.system_inputs,
        report.teams,
        skipped_note,
        report.carried_forward,
        report.reclassified,
        report.files_changed
    );
    println!(
        "JSONL: {}",
        archive
            .join("extracted")
            .join("transcripts")
            .join("prompts.jsonl.gz")
            .display()
    );
    println!("run metadata: {}", run_path.display());
    if let Some(partiality) = partiality {
        eprintln!("wrkviz: {partiality}");
        return 2;
    }
    0
}

/// The Python edition's serve help, verbatim.
pub const SERVE_HELP: &str = r#"usage: wrkviz serve [-h] --output OUTPUT [--port PORT] [--open]

serve a built archive on localhost

options:
  -h, --help       show this help message and exit
  --output OUTPUT  built archive directory
  --port PORT      0 chooses an available port
  --open
"#;

const SERVE_USAGE: &str = "usage: wrkviz serve [-h] --output OUTPUT [--port PORT] [--open]";

fn run_serve(args: &[String]) -> u8 {
    let mut output: Option<String> = None;
    let mut port: i64 = 8765;
    let mut open_browser = false;
    let mut extras: Vec<String> = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        let help_plain = arg == "-h"
            || (arg.len() > 2
                && arg.starts_with("--")
                && !arg.contains('=')
                && "--help".starts_with(arg.as_str()));
        if help_plain {
            print!("{SERVE_HELP}");
            return 0;
        }
        if let Some((head, value)) = arg.split_once('=') {
            if head.len() > 2 && head.starts_with("--") && "--help".starts_with(head) {
                return argparse_error(
                    SERVE_USAGE,
                    "wrkviz serve",
                    &format!("argument -h/--help: ignored explicit argument '{value}'"),
                );
            }
        }
        {
            let head = arg.split_once('=').map_or(arg.as_str(), |(head, _)| head);
            if head.len() > 1 && head.starts_with("--") && "--open".starts_with(head) {
                if let Some((_, value)) = arg.split_once('=') {
                    return argparse_error(
                        SERVE_USAGE,
                        "wrkviz serve",
                        &format!("argument --open: ignored explicit argument '{value}'"),
                    );
                }
                open_browser = true;
                index += 1;
                continue;
            }
        }
        let mut handled = true;
        for (name, is_port) in [("--output", false), ("--port", true)] {
            match named_value(args, &mut index, name) {
                Ok(Some(value)) => {
                    if is_port {
                        let Some((parsed, _)) = parse_py_int(&value) else {
                            return argparse_error(
                                SERVE_USAGE,
                                "wrkviz serve",
                                &format!("argument --port: invalid int value: '{value}'"),
                            );
                        };
                        port = parsed;
                    } else {
                        output = Some(value);
                    }
                    break;
                }
                Ok(None) => {}
                Err((_, message)) => return argparse_error(SERVE_USAGE, "wrkviz serve", &message),
            }
        }
        if !is_option_token(arg, &["--output", "--port", "--open"]) {
            handled = false;
        }
        if !handled {
            extras.push(arg.clone());
        }
        index += 1;
    }
    let Some(output) = output else {
        return argparse_error(
            SERVE_USAGE,
            "wrkviz serve",
            "the following arguments are required: --output",
        );
    };
    if !extras.is_empty() {
        eprintln!(
            "usage: wrkviz [-h] [--version] [--userguide]\n              {{quickstart,ingest-project,ingest,ingest-claude,ingest-claude-history,ingest-muse,ingest-orc,summarize,build,extract-transcripts,export,refresh,github-metadata,refresh-claude,refresh-orc,serve,inspect,gc,migrate-build-state,migrate-snapshots,audit-glossary,audit-losslessness,query,responsiveness,discover-agents}}\n              ...\nwrkviz: error: unrecognized arguments: {}",
            extras.join(" ")
        );
        return 2;
    }
    if !(0..=65535).contains(&port) {
        eprintln!("wrkviz: port must be between 0 and 65535");
        return 2;
    }
    crate::serve::serve(&cli_path(&output), port as u16, open_browser)
}

fn argparse_error(usage: &str, prog: &str, message: &str) -> u8 {
    eprintln!("{usage}\n{prog}: error: {message}");
    2
}

fn named_value(
    args: &[String],
    index: &mut usize,
    name: &str,
) -> Result<Option<String>, (String, String)> {
    let arg = &args[*index];
    // An abbreviated long option may also carry `=value`
    // (`--te=alpha`): argparse matches the prefix before `=`.
    let (head, explicit) = match arg.split_once('=') {
        Some((head, value)) => (head, Some(value)),
        None => (arg.as_str(), None),
    };
    if head == name || (head.len() > 2 && head.starts_with("--") && name.starts_with(head)) {
        if let Some(value) = explicit {
            return Ok(Some(value.to_string()));
        }
    }
    if let Some(value) = arg.strip_prefix(&format!("{name}=")) {
        return Ok(Some(value.to_string()));
    }
    // argparse also accepts an unambiguous prefix of a long option.
    if arg == name || (arg.len() > 2 && arg.starts_with("--") && name.starts_with(arg.as_str())) {
        *index += 1;
        // argparse will not take an option-shaped token as the
        // value: `--team --kind` is a missing value, not a team
        // named `--kind`.
        let looks_like_option = |value: &str| {
            value.len() > 1
                && value.starts_with('-')
                && !value[1..]
                    .chars()
                    .next()
                    .is_some_and(|ch| ch.is_ascii_digit())
        };
        return match args.get(*index) {
            Some(value) if !looks_like_option(value) => Ok(Some(value.clone())),
            _ => Err((
                name.to_string(),
                format!("argument {name}: expected one argument"),
            )),
        };
    }
    Ok(None)
}

fn is_option_token(arg: &str, names: &[&str]) -> bool {
    let head = arg.split_once('=').map_or(arg, |(head, _)| head);
    names.iter().any(|name| {
        head == *name || (head.len() > 2 && head.starts_with("--") && name.starts_with(head))
    })
}

fn run_query(args: &[String]) -> u8 {
    let mut output: Option<String> = None;
    let mut format = "json".to_string();
    let mut action: Option<String> = None;
    let mut rest: Vec<String> = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        let help_plain = arg == "-h"
            || (arg.len() > 2
                && arg.starts_with("--")
                && !arg.contains('=')
                && "--help".starts_with(arg.as_str()));
        if help_plain {
            match action.as_deref() {
                None => {
                    print!("{QUERY_HELP}");
                    return 0;
                }
                Some("list") => {
                    print!("{QUERY_LIST_HELP}");
                    return 0;
                }
                Some("show") => {
                    print!("{QUERY_SHOW_HELP}");
                    return 0;
                }
                Some("stats") | Some("prompts") | Some("messages") | Some("search") => {
                    // These parsers validate their options in
                    // order; a `--help` after an invalid choice
                    // must not win over that error.
                    rest.push(arg.clone());
                    index += 1;
                    continue;
                }
                Some(other) => {
                    eprintln!(
                        "wrkviz: the Rust edition has not ported 'query {other}' yet; the Python edition remains the oracle for it"
                    );
                    return 2;
                }
            }
        }
        if action.is_some() {
            rest.push(arg.clone());
            index += 1;
            continue;
        }
        match named_value(args, &mut index, "--output") {
            Ok(Some(value)) => output = Some(value),
            Ok(None) => match named_value(args, &mut index, "--format") {
                Ok(Some(value)) => {
                    if !["json", "jsonl", "markdown", "text"].contains(&value.as_str()) {
                        return argparse_error(
                            QUERY_USAGE,
                            "wrkviz query",
                            &format!(
                                "argument --format: invalid choice: '{}' (choose from json, jsonl, markdown, text)",
                                value
                            ),
                        );
                    }
                    format = value;
                }
                Ok(None) => {
                    if arg.starts_with("--") {
                        return argparse_error(
                            QUERY_USAGE,
                            "wrkviz query",
                            &format!("unrecognized arguments: {arg}"),
                        );
                    }
                    if !["list", "show", "search", "prompts", "messages", "stats"]
                        .contains(&arg.as_str())
                    {
                        return argparse_error(
                            QUERY_USAGE,
                            "wrkviz query",
                            &format!(
                                "argument query_action: invalid choice: '{arg}' (choose from list, show, search, prompts, messages, stats)"
                            ),
                        );
                    }
                    action = Some(arg.clone());
                }
                Err((_, message)) => return argparse_error(QUERY_USAGE, "wrkviz query", &message),
            },
            Err((_, message)) => return argparse_error(QUERY_USAGE, "wrkviz query", &message),
        }
        index += 1;
    }
    // A `show` flag carrying an explicit `=value` is refused
    // during parsing, before argparse checks the query
    // parser's required arguments; let the show parser say so
    // even when --output is missing.
    if action.as_deref() == Some("show") && output.is_none() {
        let explicit = rest.iter().any(|arg| {
            let Some((name, _)) = arg.split_once('=') else {
                return false;
            };
            name.len() > 2
                && name.starts_with("--")
                && ("--help".starts_with(name) || "--transcript".starts_with(name))
        });
        if explicit {
            return run_query_show("", &format, &rest);
        }
    }
    if action.as_deref() == Some("stats") && output.is_none() {
        // The stats parser runs before the query parser's
        // required-argument check: its own errors (an invalid
        // --kind choice, a refused explicit help value) and
        // its help outrank a missing --output.
        let early = rest.iter().any(|arg| {
            let head = arg.split_once('=').map_or(arg.as_str(), |(head, _)| head);
            head == "-h"
                || head == "--help"
                || (head.len() > 1 && head.starts_with("--") && "--help".starts_with(head))
                || (head.len() > 2 && head.starts_with("--") && "--kind".starts_with(head))
                || (head.len() > 1
                    && !head.starts_with("--")
                    && head.starts_with('-')
                    && head[1..].starts_with('h'))
        });
        if early {
            return run_query_stats("", &format, &rest, true);
        }
    }
    if action.as_deref() == Some("search") && output.is_none() {
        return run_query_search("", &format, &rest, true);
    }
    if matches!(action.as_deref(), Some("prompts") | Some("messages")) && output.is_none() {
        // The subcommand parser runs before the query parser's
        // required-argument check: its own errors and help
        // outrank a missing --output, so always delegate.
        let action = action.clone().unwrap_or_default();
        return run_query_transcript(&action, "", &format, &rest, true);
    }
    let mut missing: Vec<&str> = Vec::new();
    if output.is_none() {
        missing.push("--output");
    }
    if action.is_none() {
        missing.push("query_action");
    }
    if !missing.is_empty() {
        return argparse_error(
            QUERY_USAGE,
            "wrkviz query",
            &format!(
                "the following arguments are required: {}",
                missing.join(", ")
            ),
        );
    }
    let output = output.unwrap_or_default();
    match action.as_deref() {
        Some("list") => run_query_list(&output, &format, &rest),
        Some("show") => run_query_show(&output, &format, &rest),
        Some("stats") => run_query_stats(&output, &format, &rest, false),
        Some(action @ ("prompts" | "messages")) => {
            run_query_transcript(action, &output, &format, &rest, false)
        }
        Some("search") => run_query_search(&output, &format, &rest, false),
        Some(other) => {
            eprintln!(
                "wrkviz: the Rust edition has not ported 'query {other}' yet; the Python edition remains the oracle for it"
            );
            2
        }
        None => 2,
    }
}

fn run_query_show(output: &str, format: &str, args: &[String]) -> u8 {
    let mut reference: Option<String> = None;
    let mut transcript = false;
    let mut extras: Vec<String> = Vec::new();
    let mut after_separator = false;
    for arg in args {
        if !after_separator {
            if arg == "--" {
                after_separator = true;
                continue;
            }
            if let Some(long) = arg.strip_prefix("--") {
                // argparse long options: an exact name or an
                // unambiguous abbreviation, with an optional
                // `=value` a flag option refuses by name.
                let (name, value) = match long.split_once('=') {
                    Some((name, value)) => (name, Some(value)),
                    None => (long, None),
                };
                let spelled = format!("--{name}");
                if "--help".starts_with(spelled.as_str()) && name.len() > 1 {
                    if let Some(value) = value {
                        return argparse_error(
                            QUERY_SHOW_USAGE,
                            "wrkviz query show",
                            &format!("argument -h/--help: ignored explicit argument '{value}'"),
                        );
                    }
                    print!("{QUERY_SHOW_HELP}");
                    return 0;
                }
                if "--transcript".starts_with(spelled.as_str()) && name.len() > 1 {
                    if let Some(value) = value {
                        return argparse_error(
                            QUERY_SHOW_USAGE,
                            "wrkviz query show",
                            &format!("argument --transcript: ignored explicit argument '{value}'"),
                        );
                    }
                    transcript = true;
                    continue;
                }
                extras.push(arg.clone());
                continue;
            }
            if arg.len() > 1 && arg.starts_with('-') {
                if arg[1..].starts_with('h') {
                    print!("{QUERY_SHOW_HELP}");
                    return 0;
                }
                extras.push(arg.clone());
                continue;
            }
        }
        if reference.is_none() {
            reference = Some(arg.clone());
        } else {
            extras.push(arg.clone());
        }
    }
    let Some(reference) = reference else {
        return argparse_error(
            QUERY_SHOW_USAGE,
            "wrkviz query show",
            "the following arguments are required: reference",
        );
    };
    if !extras.is_empty() {
        // Once a subcommand parser is chosen, unknown
        // arguments are reported by the root parser with
        // the top-level usage (argparse's spelling here).
        eprintln!(
            "usage: wrkviz [-h] [--version] [--userguide]\n              {{quickstart,ingest-project,ingest,ingest-claude,ingest-claude-history,ingest-muse,ingest-orc,summarize,build,extract-transcripts,export,refresh,github-metadata,refresh-claude,refresh-orc,serve,inspect,gc,migrate-build-state,migrate-snapshots,audit-glossary,audit-losslessness,query,responsiveness,discover-agents}}\n              ...\nwrkviz: error: unrecognized arguments: {}",
            extras.join(" ")
        );
        return 2;
    }
    let timeline = match crate::query::Timeline::open(&cli_path(output)) {
        Ok(timeline) => timeline,
        Err(error) => {
            eprintln!("wrkviz: {error}");
            return 2;
        }
    };
    match timeline.show_record(&reference, transcript) {
        Ok(item) => {
            match crate::query::format_query(&format!("show {reference}"), &[item], format) {
                Ok(text) => {
                    print!("{text}");
                    0
                }
                Err(error) => {
                    eprintln!("wrkviz: {error}");
                    2
                }
            }
        }
        Err(error) => {
            eprintln!("wrkviz: {error}");
            2
        }
    }
}

fn run_query_transcript(
    action: &str,
    output: &str,
    format: &str,
    args: &[String],
    output_missing: bool,
) -> u8 {
    let (usage, help) = if action == "prompts" {
        (QUERY_PROMPTS_USAGE, QUERY_PROMPTS_HELP)
    } else {
        (QUERY_MESSAGES_USAGE, QUERY_MESSAGES_HELP)
    };
    let prog = format!("wrkviz query {action}");
    let mut range_raw: Option<String> = None;
    let mut which = "human".to_string();
    let mut limit: Option<i64> = None;
    let mut tail: Option<i64> = None;
    let mut verify = false;
    let mut filters = crate::query::Filters {
        teams: Vec::new(),
        start_time: None,
        end_time: None,
        kinds: Vec::new(),
        agent: None,
    };
    let mut extras: Vec<String> = Vec::new();
    let mut after_separator = false;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if after_separator {
            extras.push(arg.clone());
            index += 1;
            continue;
        }
        if arg == "--" {
            after_separator = true;
            extras.push(arg.clone());
            index += 1;
            continue;
        }
        {
            let (head, explicit) = match arg.split_once('=') {
                Some((head, value)) => (head, Some(value)),
                None => (arg.as_str(), None),
            };
            let long_help = head.len() > 1 && head.starts_with("--") && "--help".starts_with(head);
            let short_help = !head.starts_with("--")
                && head.len() > 1
                && head.starts_with('-')
                && head[1..].starts_with('h');
            if long_help || short_help {
                if let Some(value) = explicit {
                    return argparse_error(
                        usage,
                        &prog,
                        &format!("argument -h/--help: ignored explicit argument '{value}'"),
                    );
                }
                print!("{help}");
                return 0;
            }
            if head.len() > 1 && head.starts_with("--") && "--verify".starts_with(head) {
                if let Some(value) = explicit {
                    return argparse_error(
                        usage,
                        &prog,
                        &format!("argument --verify: ignored explicit argument '{value}'"),
                    );
                }
                verify = true;
                index += 1;
                continue;
            }
        }
        let mut handled = true;
        for (name, target) in [
            ("--range", 0),
            ("--which", 1),
            ("--limit", 2),
            ("--tail", 3),
            ("--team", 4),
            ("--start-time", 5),
            ("--end-time", 6),
        ] {
            match named_value(args, &mut index, name) {
                Ok(Some(value)) => {
                    match target {
                        0 => range_raw = Some(value),
                        1 => {
                            if !["human", "bot", "all"].contains(&value.as_str()) {
                                return argparse_error(
                                    usage,
                                    &prog,
                                    &format!(
                                        "argument --which: invalid choice: '{value}' (choose from human, bot, all)"
                                    ),
                                );
                            }
                            which = value;
                        }
                        2 | 3 => {
                            // Python ints are unbounded: a huge
                            // integer is a value, clamped here;
                            // anything else is argparse's error.
                            let int_shaped = {
                                let digits = value.strip_prefix('-').unwrap_or(&value);
                                !digits.is_empty()
                                    && digits.bytes().all(|byte| byte.is_ascii_digit())
                            };
                            let parsed = match value.parse::<i64>() {
                                Ok(parsed) => parsed,
                                Err(_) if int_shaped && value.starts_with('-') => i64::MIN + 1,
                                Err(_) if int_shaped => i64::MAX,
                                Err(_) => {
                                    return argparse_error(
                                        usage,
                                        &prog,
                                        &format!("argument {name}: invalid int value: '{value}'"),
                                    );
                                }
                            };
                            if target == 2 {
                                limit = Some(parsed);
                            } else {
                                tail = Some(parsed);
                            }
                        }
                        4 => filters.teams.push(value),
                        5 => filters.start_time = Some(value),
                        _ => filters.end_time = Some(value),
                    }
                    break;
                }
                Ok(None) => {}
                Err((_, message)) => return argparse_error(usage, &prog, &message),
            }
        }
        if !is_option_token(
            arg,
            &[
                "--range",
                "--which",
                "--limit",
                "--tail",
                "--verify",
                "--team",
                "--start-time",
                "--end-time",
            ],
        ) {
            handled = false;
        }
        if !handled {
            extras.push(arg.clone());
        }
        index += 1;
    }
    if extras.iter().any(|arg| arg != "--") {
        eprintln!(
            "usage: wrkviz [-h] [--version] [--userguide]\n              {{quickstart,ingest-project,ingest,ingest-claude,ingest-claude-history,ingest-muse,ingest-orc,summarize,build,extract-transcripts,export,refresh,github-metadata,refresh-claude,refresh-orc,serve,inspect,gc,migrate-build-state,migrate-snapshots,audit-glossary,audit-losslessness,query,responsiveness,discover-agents}}\n              ...\nwrkviz: error: unrecognized arguments: {}",
            extras.join(" ")
        );
        return 2;
    }
    if output_missing {
        return argparse_error(
            QUERY_USAGE,
            "wrkviz query",
            "the following arguments are required: --output",
        );
    }
    let root = cli_path(output);
    let transcripts = match crate::query::TranscriptQuery::open(&root) {
        Ok(transcripts) => transcripts,
        Err(error) => {
            eprintln!("wrkviz: {error}");
            return 2;
        }
    };
    if verify {
        if let Err(error) = transcripts.verify_all() {
            eprintln!("wrkviz: {error}");
            return 2;
        }
    }
    let ordinal_range = match &range_raw {
        None => None,
        Some(raw) => match crate::query::parse_ordinal_range(raw) {
            Ok(range) => Some(range),
            Err(error) => {
                eprintln!("wrkviz: {error}");
                return 2;
            }
        },
    };
    if let Err(error) = crate::query::check_window(&filters) {
        eprintln!("wrkviz: {error}");
        return 2;
    }
    let items = if action == "prompts" {
        transcripts.list_prompts(&filters, ordinal_range, &which, limit, tail)
    } else {
        transcripts.list_messages(&filters, ordinal_range, &which, limit, tail, verify)
    };
    match items {
        Ok(items) => match crate::query::format_query(action, &items, format) {
            Ok(text) => {
                print!("{text}");
                0
            }
            Err(error) => {
                eprintln!("wrkviz: {error}");
                2
            }
        },
        Err(error) => {
            eprintln!("wrkviz: {error}");
            2
        }
    }
}

/// CPython `int()` grammar for `--offset`/`--limit`: optional
/// surrounding whitespace and sign, digits with single
/// underscores between them. Returns the clamped value and
/// the canonical spelling (Python ints are unbounded).
fn parse_py_int(value: &str) -> Option<(i64, String)> {
    let trimmed = value.trim_matches([' ', '\t', '\n', '\r', '\u{b}', '\u{c}']);
    let (negative, digits) = match trimmed.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, trimmed.strip_prefix('+').unwrap_or(trimmed)),
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
        || !digits
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'_')
    {
        return None;
    }
    let cleaned: String = digits.chars().filter(|ch| *ch != '_').collect();
    let magnitude = cleaned.trim_start_matches('0');
    let magnitude = if magnitude.is_empty() { "0" } else { magnitude };
    let canonical = if negative && magnitude != "0" {
        format!("-{magnitude}")
    } else {
        magnitude.to_string()
    };
    let clamped =
        canonical
            .parse::<i64>()
            .unwrap_or(if negative { i64::MIN + 1 } else { i64::MAX });
    Some((clamped, canonical))
}

fn looks_like_negative_number(arg: &str) -> bool {
    let Some(rest) = arg.strip_prefix('-') else {
        return false;
    };
    if rest.starts_with('-') {
        return false;
    }
    let (whole, fraction) = match rest.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (rest, None),
    };
    match fraction {
        None => !whole.is_empty() && whole.bytes().all(|byte| byte.is_ascii_digit()),
        Some(fraction) => {
            (whole.is_empty() || whole.bytes().all(|byte| byte.is_ascii_digit()))
                && !fraction.is_empty()
                && fraction.bytes().all(|byte| byte.is_ascii_digit())
        }
    }
}

fn run_query_search(output: &str, format: &str, args: &[String], output_missing: bool) -> u8 {
    let prog = "wrkviz query search";
    let mut text: Option<String> = None;
    let mut corpus: Option<String> = None;
    let mut scope: Option<String> = None;
    let mut match_mode: Option<String> = None;
    let mut sort: Option<String> = None;
    let mut prompt_author: Option<String> = None;
    let mut linkage: Option<String> = None;
    let mut roles: Vec<String> = Vec::new();
    let mut case_sensitive = false;
    let mut offset: Option<i64> = None;
    let mut offset_display: Option<String> = None;
    let mut limit: i64 = 50;
    let mut filters = crate::query::Filters {
        teams: Vec::new(),
        start_time: None,
        end_time: None,
        kinds: Vec::new(),
        agent: None,
    };
    let mut extras: Vec<String> = Vec::new();
    let mut after_separator = false;
    let mut index = 0;
    let choice = |name: &str, value: &str, choices: &[&str], rendered: &str| -> Option<u8> {
        if choices.contains(&value) {
            return None;
        }
        Some(argparse_error(
            QUERY_SEARCH_USAGE,
            prog,
            &format!("argument {name}: invalid choice: '{value}' (choose from {rendered})"),
        ))
    };
    while index < args.len() {
        let arg = &args[index];
        if after_separator {
            if text.is_none() {
                text = Some(arg.clone());
            } else {
                extras.push(arg.clone());
            }
            index += 1;
            continue;
        }
        if arg == "--" {
            after_separator = true;
            index += 1;
            continue;
        }
        {
            let (head, explicit) = match arg.split_once('=') {
                Some((head, value)) => (head, Some(value)),
                None => (arg.as_str(), None),
            };
            let long_help = head.len() > 1 && head.starts_with("--") && "--help".starts_with(head);
            let short_help = !head.starts_with("--")
                && head.len() > 1
                && head.starts_with('-')
                && head[1..].starts_with('h');
            if long_help || short_help {
                if let Some(value) = explicit {
                    return argparse_error(
                        QUERY_SEARCH_USAGE,
                        prog,
                        &format!("argument -h/--help: ignored explicit argument '{value}'"),
                    );
                }
                print!("{QUERY_SEARCH_HELP}");
                return 0;
            }
            if head.len() > 1 && head.starts_with("--") && "--case-sensitive".starts_with(head) {
                if let Some(value) = explicit {
                    return argparse_error(
                        QUERY_SEARCH_USAGE,
                        prog,
                        &format!("argument --case-sensitive: ignored explicit argument '{value}'"),
                    );
                }
                case_sensitive = true;
                index += 1;
                continue;
            }
        }
        let mut handled = true;
        for (name, target) in [
            ("--in", 0),
            ("--scope", 1),
            ("--match", 2),
            ("--sort", 3),
            ("--prompt-author", 4),
            ("--linkage", 5),
            ("--role", 6),
            ("--offset", 7),
            ("--limit", 8),
            ("--team", 9),
            ("--start-time", 10),
            ("--end-time", 11),
            ("--kind", 12),
            ("--agent", 13),
        ] {
            match named_value(args, &mut index, name) {
                Ok(Some(value)) => {
                    match target {
                        0 => {
                            if let Some(code) = choice(
                                name,
                                &value,
                                &["owner-prompts", "agent-responses", "all-transcript"],
                                "owner-prompts, agent-responses, all-transcript",
                            ) {
                                return code;
                            }
                            corpus = Some(value);
                        }
                        1 => {
                            if let Some(code) = choice(
                                name,
                                &value,
                                &["summaries", "transcripts", "all"],
                                "summaries, transcripts, all",
                            ) {
                                return code;
                            }
                            scope = Some(value);
                        }
                        2 => {
                            if let Some(code) = choice(
                                name,
                                &value,
                                &["smart", "phrase", "literal"],
                                "smart, phrase, literal",
                            ) {
                                return code;
                            }
                            match_mode = Some(value);
                        }
                        3 => {
                            if let Some(code) = choice(
                                name,
                                &value,
                                &["relevance", "newest", "oldest"],
                                "relevance, newest, oldest",
                            ) {
                                return code;
                            }
                            sort = Some(value);
                        }
                        4 => {
                            if let Some(code) = choice(
                                name,
                                &value,
                                &["any", "owner", "agent", "unclassified"],
                                "any, owner, agent, unclassified",
                            ) {
                                return code;
                            }
                            prompt_author = Some(value);
                        }
                        5 => {
                            if let Some(code) = choice(
                                name,
                                &value,
                                &["any", "linked", "unlinked"],
                                "any, linked, unlinked",
                            ) {
                                return code;
                            }
                            linkage = Some(value);
                        }
                        6 => {
                            if let Some(code) = choice(
                                name,
                                &value,
                                &[
                                    "user",
                                    "assistant",
                                    "agent",
                                    "system",
                                    "external",
                                    "goal",
                                    "tool",
                                    "event",
                                ],
                                "user, assistant, agent, system, external, goal, tool, event",
                            ) {
                                return code;
                            }
                            roles.push(value);
                        }
                        7 | 8 => {
                            let Some((parsed, canonical)) = parse_py_int(&value) else {
                                return argparse_error(
                                    QUERY_SEARCH_USAGE,
                                    prog,
                                    &format!("argument {name}: invalid int value: '{value}'"),
                                );
                            };
                            if target == 7 {
                                offset = Some(parsed);
                                offset_display = Some(canonical);
                            } else {
                                limit = parsed;
                            }
                        }
                        9 => filters.teams.push(value),
                        10 => filters.start_time = Some(value),
                        11 => filters.end_time = Some(value),
                        12 => {
                            if let Some(code) = choice(
                                name,
                                &value,
                                &["hourly", "daily", "weekly", "monthly", "quarterly"],
                                "hourly, daily, weekly, monthly, quarterly",
                            ) {
                                return code;
                            }
                            filters.kinds.push(value);
                        }
                        _ => filters.agent = Some(value),
                    }
                    break;
                }
                Ok(None) => {}
                Err((_, message)) => return argparse_error(QUERY_SEARCH_USAGE, prog, &message),
            }
        }
        if !is_option_token(
            arg,
            &[
                "--in",
                "--scope",
                "--match",
                "--sort",
                "--prompt-author",
                "--linkage",
                "--role",
                "--case-sensitive",
                "--offset",
                "--limit",
                "--team",
                "--start-time",
                "--end-time",
                "--kind",
                "--agent",
            ],
        ) {
            handled = false;
        }
        if !handled {
            if text.is_none()
                && (!arg.starts_with('-') || arg == "-" || looks_like_negative_number(arg))
            {
                text = Some(arg.clone());
            } else {
                extras.push(arg.clone());
            }
        }
        index += 1;
    }
    let Some(needle) = text else {
        return argparse_error(
            QUERY_SEARCH_USAGE,
            prog,
            "the following arguments are required: text",
        );
    };
    if extras.iter().any(|arg| arg != "--") {
        eprintln!(
            "usage: wrkviz [-h] [--version] [--userguide]\n              {{quickstart,ingest-project,ingest,ingest-claude,ingest-claude-history,ingest-muse,ingest-orc,summarize,build,extract-transcripts,export,refresh,github-metadata,refresh-claude,refresh-orc,serve,inspect,gc,migrate-build-state,migrate-snapshots,audit-glossary,audit-losslessness,query,responsiveness,discover-agents}}\n              ...\nwrkviz: error: unrecognized arguments: {}",
            extras.join(" ")
        );
        return 2;
    }
    if output_missing {
        return argparse_error(
            QUERY_USAGE,
            "wrkviz query",
            "the following arguments are required: --output",
        );
    }
    let timeline = match crate::query::Timeline::open(&cli_path(output)) {
        Ok(timeline) => timeline,
        Err(error) => {
            eprintln!("wrkviz: {error}");
            return 2;
        }
    };
    let command = format!("search {needle}");
    if let Some(corpus) = &corpus {
        if scope.is_some() {
            eprintln!("wrkviz: --in and --scope cannot be combined");
            return 2;
        }
        if let Err(error) = crate::query::check_window(&filters) {
            eprintln!("wrkviz: {error}");
            return 2;
        }
        return match timeline.search_v2(
            &needle,
            corpus,
            &filters,
            case_sensitive,
            match_mode.as_deref().unwrap_or("smart"),
            sort.as_deref().unwrap_or("relevance"),
            prompt_author.as_deref().unwrap_or("any"),
            linkage.as_deref().unwrap_or("any"),
            &roles,
            offset.unwrap_or(0),
            limit,
        ) {
            Ok(mut results) => {
                if let Some(display) = &offset_display {
                    results.offset_display = display.clone();
                }
                match crate::query::format_search_results(&results, &command, format) {
                    Ok(text) => {
                        print!("{text}");
                        0
                    }
                    Err(error) => {
                        eprintln!("wrkviz: {error}");
                        2
                    }
                }
            }
            Err(error) => {
                eprintln!("wrkviz: {error}");
                2
            }
        };
    }
    if match_mode.is_some()
        || sort.is_some()
        || prompt_author.is_some()
        || linkage.is_some()
        || offset.is_some()
        || !roles.is_empty()
    {
        eprintln!("wrkviz: search-v2 options require --in");
        return 2;
    }
    if let Err(error) = crate::query::check_window(&filters) {
        eprintln!("wrkviz: {error}");
        return 2;
    }
    match timeline.search(
        &needle,
        scope.as_deref().unwrap_or("summaries"),
        &filters,
        case_sensitive,
        limit,
    ) {
        Ok(items) => match crate::query::format_query(&command, &items, format) {
            Ok(text) => {
                print!("{text}");
                0
            }
            Err(error) => {
                eprintln!("wrkviz: {error}");
                2
            }
        },
        Err(error) => {
            eprintln!("wrkviz: {error}");
            2
        }
    }
}

fn run_query_stats(output: &str, format: &str, args: &[String], output_missing: bool) -> u8 {
    let mut filters = crate::query::Filters {
        teams: Vec::new(),
        start_time: None,
        end_time: None,
        kinds: Vec::new(),
        agent: None,
    };
    let mut extras: Vec<String> = Vec::new();
    let mut after_separator = false;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if after_separator {
            extras.push(arg.clone());
            index += 1;
            continue;
        }
        if arg == "--" {
            after_separator = true;
            // argparse names the separator itself among the
            // unrecognized arguments when anything follows it.
            extras.push(arg.clone());
            index += 1;
            continue;
        }
        // Help, in its plain and short-cluster spellings,
        // fires when argparse reaches it -- after the options
        // before it have been validated, before the rest.
        {
            let (head, explicit) = match arg.split_once('=') {
                Some((head, value)) => (head, Some(value)),
                None => (arg.as_str(), None),
            };
            let long_help = head.len() > 1 && head.starts_with("--") && "--help".starts_with(head);
            let short_help = !head.starts_with("--")
                && head.len() > 1
                && head.starts_with('-')
                && head[1..].starts_with('h');
            if long_help || short_help {
                if let Some(value) = explicit {
                    return argparse_error(
                        QUERY_STATS_USAGE,
                        "wrkviz query stats",
                        &format!("argument -h/--help: ignored explicit argument '{value}'"),
                    );
                }
                print!("{QUERY_STATS_HELP}");
                return 0;
            }
        }
        let mut handled = true;
        for (name, target) in [
            ("--team", 0),
            ("--start-time", 1),
            ("--end-time", 2),
            ("--kind", 3),
        ] {
            match named_value(args, &mut index, name) {
                Ok(Some(value)) => {
                    match target {
                        0 => filters.teams.push(value),
                        1 => filters.start_time = Some(value),
                        2 => filters.end_time = Some(value),
                        _ => {
                            if !["hourly", "daily", "weekly", "monthly", "quarterly"]
                                .contains(&value.as_str())
                            {
                                return argparse_error(
                                    QUERY_STATS_USAGE,
                                    "wrkviz query stats",
                                    &format!(
                                        "argument --kind: invalid choice: '{value}' (choose from hourly, daily, weekly, monthly, quarterly)"
                                    ),
                                );
                            }
                            filters.kinds.push(value);
                        }
                    }
                    break;
                }
                Ok(None) => {}
                Err((_, message)) => {
                    return argparse_error(QUERY_STATS_USAGE, "wrkviz query stats", &message)
                }
            }
        }
        if !is_option_token(arg, &["--team", "--start-time", "--end-time", "--kind"]) {
            handled = false;
        }
        if !handled {
            extras.push(arg.clone());
        }
        index += 1;
    }
    if extras.iter().any(|arg| arg != "--") {
        // Once a subcommand parser is chosen, unknown
        // arguments are reported by the root parser with
        // the top-level usage (argparse's spelling here).
        eprintln!(
            "usage: wrkviz [-h] [--version] [--userguide]\n              {{quickstart,ingest-project,ingest,ingest-claude,ingest-claude-history,ingest-muse,ingest-orc,summarize,build,extract-transcripts,export,refresh,github-metadata,refresh-claude,refresh-orc,serve,inspect,gc,migrate-build-state,migrate-snapshots,audit-glossary,audit-losslessness,query,responsiveness,discover-agents}}\n              ...\nwrkviz: error: unrecognized arguments: {}",
            extras.join(" ")
        );
        return 2;
    }
    if output_missing {
        return argparse_error(
            QUERY_USAGE,
            "wrkviz query",
            "the following arguments are required: --output",
        );
    }
    let root = cli_path(output);
    match crate::query::archive_stats(&root, &filters) {
        Ok(stats) => match crate::query::format_stats(&stats, format) {
            Ok(text) => {
                print!("{text}");
                0
            }
            Err(error) => {
                eprintln!("wrkviz: {error}");
                2
            }
        },
        Err(error) => {
            eprintln!("wrkviz: {error}");
            2
        }
    }
}

fn run_query_list(output: &str, format: &str, args: &[String]) -> u8 {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print!("{QUERY_LIST_HELP}");
        return 0;
    }
    let mut resource: Option<String> = None;
    let mut filters = crate::query::Filters {
        teams: Vec::new(),
        start_time: None,
        end_time: None,
        kinds: Vec::new(),
        agent: None,
    };
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        let mut handled = true;
        for (name, target) in [
            ("--team", 0),
            ("--start-time", 1),
            ("--end-time", 2),
            ("--kind", 3),
            ("--agent", 4),
        ] {
            match named_value(args, &mut index, name) {
                Ok(Some(value)) => {
                    match target {
                        0 => filters.teams.push(value),
                        1 => filters.start_time = Some(value),
                        2 => filters.end_time = Some(value),
                        3 => {
                            if !["hourly", "daily", "weekly", "monthly", "quarterly"]
                                .contains(&value.as_str())
                            {
                                return argparse_error(
                                    QUERY_LIST_USAGE,
                                    "wrkviz query list",
                                    &format!(
                                        "argument --kind: invalid choice: '{value}' (choose from hourly, daily, weekly, monthly, quarterly)"
                                    ),
                                );
                            }
                            filters.kinds.push(value);
                        }
                        _ => filters.agent = Some(value),
                    }
                    break;
                }
                Ok(None) => {}
                Err((_, message)) => {
                    return argparse_error(QUERY_LIST_USAGE, "wrkviz query list", &message)
                }
            }
            let _ = target;
        }
        if !is_option_token(
            arg,
            &["--team", "--start-time", "--end-time", "--kind", "--agent"],
        ) {
            handled = false;
        }
        if !handled {
            if arg.starts_with("--") {
                return argparse_error(
                    QUERY_LIST_USAGE,
                    "wrkviz query list",
                    &format!("unrecognized arguments: {arg}"),
                );
            }
            if resource.is_some() {
                return argparse_error(
                    QUERY_LIST_USAGE,
                    "wrkviz query list",
                    &format!("unrecognized arguments: {arg}"),
                );
            }
            if !["teams", "agents", "phases", "rollups"].contains(&arg.as_str()) {
                return argparse_error(
                    QUERY_LIST_USAGE,
                    "wrkviz query list",
                    &format!(
                        "argument resource: invalid choice: '{arg}' (choose from teams, agents, phases, rollups)"
                    ),
                );
            }
            resource = Some(arg.clone());
        }
        index += 1;
    }
    let Some(resource) = resource else {
        return argparse_error(
            QUERY_LIST_USAGE,
            "wrkviz query list",
            "the following arguments are required: resource",
        );
    };
    let timeline = match crate::query::Timeline::open(&cli_path(output)) {
        Ok(timeline) => timeline,
        Err(error) => {
            eprintln!("wrkviz: {error}");
            return 2;
        }
    };
    match timeline.list_records(&resource, &filters) {
        Ok(items) => {
            match crate::query::format_query(&format!("list {resource}"), &items, format) {
                Ok(text) => {
                    print!("{text}");
                    0
                }
                Err(error) => {
                    eprintln!("wrkviz: {error}");
                    2
                }
            }
        }
        Err(error) => {
            eprintln!("wrkviz: {error}");
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
