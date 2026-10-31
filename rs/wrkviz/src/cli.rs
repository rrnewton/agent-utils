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
        "ingest-claude-history" => run_ingest_claude_history(&args[1..]),
        "ingest-claude" => run_ingest_session_claude(&args[1..]),
        "ingest-muse" => run_ingest_session_muse(&args[1..]),
        "ingest-orc" => run_ingest_session_orc(&args[1..]),
        "ingest-project" => run_ingest_project(&args[1..]),
        "ingest" => run_ingest(&args[1..]),
        "summarize" => run_summarize(&args[1..]),
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
                    ingest: None,
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
            ingest: None,
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

/// The Python edition's ingest help, verbatim.
pub const INGEST_HELP: &str = r#"usage: wrkviz ingest [-h] --output OUTPUT --team TEAM [--snapshot-root DIR]
                     [--sessions-root SESSIONS_ROOT] --root-session
                     ROOT_SESSION [--continuation-session UUID]
                     [--timezone TIMEZONE]
                     [--start-date START_DATE | --start-time START_TIME]
                     [--end-date END_DATE | --end-time END_TIME]
                     [--project LABEL=REPOSITORY_URL] [--source-host HOSTNAME]

normalize Codex logs; do not call a model

options:
  -h, --help            show this help message and exit
  --output OUTPUT       version-controllable archive directory
  --team TEAM           stable team slug, for example example-team
  --snapshot-root DIR   where to keep the vendor source snapshots, which are
                        ingest input rather than published output and are the
                        largest thing here (default: <output>.sources, a
                        sibling of the archive; an archive that already keeps
                        them inside itself keeps doing so until `migrate-
                        snapshots` is run)
  --sessions-root SESSIONS_ROOT
                        Codex rollout tree (default: ~/.codex/sessions)
  --root-session ROOT_SESSION
                        coordinator thread UUID
  --continuation-session UUID
                        explicit successor coordinator session; repeat in
                        chronological order
  --timezone TIMEZONE   IANA display/calendar timezone (UTC instants remain
                        canonical)
  --start-date START_DATE
                        first local calendar date to include (YYYY-MM-DD,
                        inclusive)
  --start-time START_TIME
                        first instant to include (RFC3339 with offset or Z,
                        inclusive)
  --end-date END_DATE   local calendar boundary to stop at (YYYY-MM-DD,
                        exclusive)
  --end-time END_TIME   instant to stop at (RFC3339 with offset or Z,
                        exclusive)
  --project LABEL=REPOSITORY_URL
                        project/repository identity; repeat for multi-repo
                        work (the first is primary); Codex git metadata is
                        used when omitted
  --source-host HOSTNAME
                        execution hostname captured by the source data; repeat
                        for multi-host work"
"#;

const INGEST_USAGE: &str = "usage: wrkviz ingest [-h] --output OUTPUT --team TEAM [--snapshot-root DIR]\n                     [--sessions-root SESSIONS_ROOT] --root-session\n                     ROOT_SESSION [--continuation-session UUID]\n                     [--timezone TIMEZONE]\n                     [--start-date START_DATE | --start-time START_TIME]\n                     [--end-date END_DATE | --end-time END_TIME]\n                     [--project LABEL=REPOSITORY_URL] [--source-host HOSTNAME]";

const INGEST_OPTIONS: [&str; 13] = [
    "--output",
    "--team",
    "--snapshot-root",
    "--sessions-root",
    "--root-session",
    "--continuation-session",
    "--timezone",
    "--start-date",
    "--start-time",
    "--end-date",
    "--end-time",
    "--project",
    "--source-host",
];

#[allow(clippy::too_many_arguments)]
fn run_ingest(args: &[String]) -> u8 {
    run_ingest_impl(args)
}

fn run_ingest_impl(args: &[String]) -> u8 {
    let mut output: Option<String> = None;
    let mut team: Option<String> = None;
    let mut snapshot_root: Option<String> = None;
    let mut sessions_root: Option<String> = None;
    let mut root_session: Option<String> = None;
    let mut continuations: Vec<String> = Vec::new();
    let mut timezone = "America/New_York".to_string();
    let mut start_date: Option<String> = None;
    let mut start_time: Option<String> = None;
    let mut end_date: Option<String> = None;
    let mut end_time: Option<String> = None;
    let mut projects: Vec<String> = Vec::new();
    let mut hosts: Vec<String> = Vec::new();
    let mut extras: Vec<String> = Vec::new();
    let mut sep_extras: Vec<String> = Vec::new();
    let mut separated = false;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if separated {
            sep_extras.push(arg.clone());
            index += 1;
            continue;
        }
        if arg == "--" {
            separated = true;
            index += 1;
            continue;
        }
        let head = arg.split_once('=').map_or(arg.as_str(), |(head, _)| head);
        if arg == "-h" {
            print!("{INGEST_HELP}");
            return 0;
        }
        if head.len() > 2
            && head.starts_with("--")
            && head != "--help"
            && !INGEST_OPTIONS.contains(&head)
        {
            let mut candidates: Vec<&str> = vec!["--help"];
            candidates.extend(INGEST_OPTIONS.iter().copied());
            let matches: Vec<&str> = candidates
                .into_iter()
                .filter(|name| name.starts_with(head))
                .collect();
            if matches.len() > 1 {
                return argparse_error(
                    INGEST_USAGE,
                    "wrkviz ingest",
                    &format!(
                        "ambiguous option: {head} could match {}",
                        matches.join(", ")
                    ),
                );
            }
            if matches == ["--help"] {
                if let Some((_, value)) = arg.split_once('=') {
                    return argparse_error(
                        INGEST_USAGE,
                        "wrkviz ingest",
                        &format!("argument -h/--help: ignored explicit argument '{value}'"),
                    );
                }
                print!("{INGEST_HELP}");
                return 0;
            }
        }
        if head == "--help" {
            if let Some((_, value)) = arg.split_once('=') {
                return argparse_error(
                    INGEST_USAGE,
                    "wrkviz ingest",
                    &format!("argument -h/--help: ignored explicit argument '{value}'"),
                );
            }
            print!("{INGEST_HELP}");
            return 0;
        }
        let mut handled = true;
        let canonical: Option<&'static str> = if INGEST_OPTIONS.contains(&head) {
            INGEST_OPTIONS.iter().copied().find(|name| *name == head)
        } else if head.len() > 2 && head.starts_with("--") {
            let matches: Vec<&'static str> = INGEST_OPTIONS
                .iter()
                .copied()
                .filter(|name| name.starts_with(head))
                .collect();
            matches.first().copied()
        } else {
            None
        };
        let ordered: Vec<&'static str> = canonical
            .into_iter()
            .chain(
                INGEST_OPTIONS
                    .iter()
                    .copied()
                    .filter(|name| Some(*name) != canonical),
            )
            .collect();
        for name in ordered {
            match named_value(args, &mut index, name) {
                Ok(Some(value)) => {
                    let conflict = match name {
                        "--start-time" if start_date.is_some() => Some("--start-date"),
                        "--start-date" if start_time.is_some() => Some("--start-time"),
                        "--end-time" if end_date.is_some() => Some("--end-date"),
                        "--end-date" if end_time.is_some() => Some("--end-time"),
                        _ => None,
                    };
                    if let Some(other) = conflict {
                        return argparse_error(
                            INGEST_USAGE,
                            "wrkviz ingest",
                            &format!("argument {name}: not allowed with argument {other}"),
                        );
                    }
                    match name {
                        "--output" => output = Some(value),
                        "--team" => team = Some(value),
                        "--snapshot-root" => snapshot_root = Some(value),
                        "--sessions-root" => sessions_root = Some(value),
                        "--root-session" => root_session = Some(value),
                        "--continuation-session" => continuations.push(value),
                        "--timezone" => timezone = value,
                        "--start-date" => start_date = Some(value),
                        "--start-time" => start_time = Some(value),
                        "--end-date" => end_date = Some(value),
                        "--end-time" => end_time = Some(value),
                        "--project" => projects.push(value),
                        "--source-host" => hosts.push(value),
                        _ => {}
                    }
                    break;
                }
                Ok(None) => {}
                Err((_, message)) => {
                    return argparse_error(INGEST_USAGE, "wrkviz ingest", &message)
                }
            }
        }
        if !is_option_token(arg, &INGEST_OPTIONS) {
            handled = false;
        }
        if !handled {
            extras.push(arg.clone());
        }
        index += 1;
    }
    let mut required: Vec<&str> = Vec::new();
    if output.is_none() {
        required.push("--output");
    }
    if team.is_none() {
        required.push("--team");
    }
    if root_session.is_none() {
        required.push("--root-session");
    }
    if !required.is_empty() {
        return argparse_error(
            INGEST_USAGE,
            "wrkviz ingest",
            &format!(
                "the following arguments are required: {}",
                required.join(", ")
            ),
        );
    }
    if !extras.is_empty() || !sep_extras.is_empty() {
        let mut reported = extras.clone();
        if !sep_extras.is_empty() {
            reported.push("--".to_string());
            reported.extend(sep_extras.iter().cloned());
        }
        eprintln!(
            "usage: wrkviz [-h] [--version] [--userguide]\n              {{quickstart,ingest-project,ingest,ingest-claude,ingest-claude-history,ingest-muse,ingest-orc,summarize,build,extract-transcripts,export,refresh,github-metadata,refresh-claude,refresh-orc,serve,inspect,gc,migrate-build-state,migrate-snapshots,audit-glossary,audit-losslessness,query,responsiveness,discover-agents}}\n              ...\nwrkviz: error: unrecognized arguments: {}",
            reported.join(" ")
        );
        return 2;
    }
    let output = output.unwrap();
    let team_slug = team.unwrap();
    let root_session = root_session.unwrap();
    let archive = cli_path(&output);
    let started = crate::pipeline::utc_now();
    let mut command: Vec<String> = vec!["wrkviz".to_string(), "ingest".to_string()];
    command.extend(args.iter().cloned());
    let outcome: Result<crate::ingest::IngestReport, String> = (|| {
        let timezone_explicit = args
            .iter()
            .any(|item| item == "--timezone" || item.starts_with("--timezone="));
        let (parsed_projects, parsed_hosts) =
            crate::identity::parse_identity_overrides(&projects, &hosts)?;
        let overrides = crate::identity::IdentityOverrides {
            projects: parsed_projects,
            hosts: parsed_hosts,
            display_timezone_source: if timezone_explicit {
                "explicit".to_string()
            } else {
                "default".to_string()
            },
        };
        let date_window = crate::window::parse_date_window(
            start_date.as_deref(),
            end_date.as_deref(),
            &timezone,
            start_time.as_deref(),
            end_time.as_deref(),
        )?;
        let sessions = match &sessions_root {
            Some(raw) => cli_path(raw),
            None => cli_path("~/.codex/sessions"),
        };
        let (_, report) = crate::ingest::ingest_codex(
            &archive,
            &sessions,
            &root_session,
            &team_slug,
            &timezone,
            date_window.as_ref(),
            &overrides,
            &continuations,
            snapshot_root.as_deref().map(cli_path).as_deref(),
        )?;
        Ok(report)
    })();
    let report = match outcome {
        Ok(report) => report,
        Err(error) => {
            if let Ok(run_path) = crate::pipeline::record_run(
                &archive,
                &crate::pipeline::RunReceipt {
                    command: &command,
                    started_at: &started,
                    status: "failed",
                    team_slug: &team_slug,
                    error: Some(&error),
                    team_slugs: &[],
                    mechanical: None,
                    ingest: None,
                },
            ) {
                eprintln!("run metadata: {}", run_path.display());
            }
            eprintln!("wrkviz: {error}");
            return 2;
        }
    };
    print_ingest_receipt(&report, &archive);
    let ingest_json = report.to_json();
    match crate::pipeline::record_run(
        &archive,
        &crate::pipeline::RunReceipt {
            command: &command,
            started_at: &started,
            status: "completed",
            team_slug: &team_slug,
            error: None,
            team_slugs: &[],
            mechanical: None,
            ingest: Some(ingest_json),
        },
    ) {
        Ok(run_path) => println!("run metadata: {}", run_path.display()),
        Err(error) => {
            eprintln!("wrkviz: {error}");
            return 2;
        }
    }
    0
}

fn print_ingest_receipt(report: &crate::ingest::IngestReport, archive: &std::path::Path) {
    println!(
        "ingest: {} agents, {} messages/events, {} outer tools, {} edges from {} source files ({:.1} MiB); {} archive files changed",
        report.agents,
        report.events,
        report.tool_calls,
        report.edges,
        report.sources,
        report.source_bytes as f64 / (1024.0 * 1024.0),
        report.files_changed
    );
    if report.snapshot_root_established {
        let where_ = crate::json::resolve_path(&archive.join(&report.snapshot_root));
        if report.snapshot_root_layout == "in-archive-legacy" {
            eprintln!(
                "wrkviz: {}: this archive keeps its vendor source snapshots inside itself, at {}; they are ingest input rather than published output, and they are reachable over HTTP from `make serve` until they are moved. `wrkviz migrate-snapshots --output {}` says what moving them would do",
                report.team_slug,
                where_.display(),
                archive.display()
            );
        } else {
            eprintln!(
                "wrkviz: {}: vendor source snapshots are being kept in {}, outside the published archive, because they are ingest input and are usually the largest thing here. The archive records the location; back it up with the archive if you want incremental ingest to keep working",
                report.team_slug,
                where_.display()
            );
        }
    }
    if report.history_unmatched_prompts != 0 || report.history_covered_prompts != 0 {
        let mut parts: Vec<String> = Vec::new();
        if report.history_unmatched_prompts != 0 {
            parts.push(format!(
                "{} prompt(s) typed outside the project pattern",
                report.history_unmatched_prompts
            ));
        }
        if report.history_covered_prompts != 0 {
            parts.push(format!(
                "{} prompt(s) from {} session(s) whose full transcript is archived as its own team",
                report.history_covered_prompts, report.history_covered_sessions
            ));
        }
        eprintln!(
            "wrkviz: {}: left out {}",
            report.team_slug,
            parts.join("; ")
        );
    }
    if report.retired_message_projections != 0 {
        eprintln!(
            "wrkviz: {}: removed {} retired raw/messages/<thread-id>.json file(s) ({:.1} MiB); the tool no longer writes that per-thread projection, which nothing read and which raw/team.json still fully describes",
            report.team_slug,
            report.retired_message_projections,
            report.retired_message_projection_bytes as f64 / (1024.0 * 1024.0)
        );
    }
    if report.newly_promoted_task_notes != 0 {
        eprintln!(
            "wrkviz: {}: promoted {} task note(s) into raw/task-notes.jsonl ({} total); the archive now keeps this text under version control instead of only inside gitignored source_snapshots/",
            report.team_slug, report.newly_promoted_task_notes, report.task_notes
        );
        if report.task_notes_upstream_deleted != 0 {
            eprintln!(
                "wrkviz: {}: {} of those notes no longer exist in the upstream task table; for those this archive is the only copy",
                report.team_slug, report.task_notes_upstream_deleted
            );
        }
    }
    for shard in &report.damaged_payload_shards {
        eprintln!("wrkviz: {}: payload store: {shard}", report.team_slug);
    }
    if !report.pruned_payload_shards.is_empty() {
        eprintln!(
            "wrkviz: {}: {} payload shard(s) recorded by the previous ingest are no longer in the tree; the manifest no longer claims them",
            report.team_slug,
            report.pruned_payload_shards.len()
        );
    }
    if report.newly_stored_tool_payloads != 0 {
        eprintln!(
            "wrkviz: {}: stored {} tool payload(s) ({:.1} MiB) into gitignored teams/{}/payloads/ ({} total); the archive now keeps command arguments and output instead of discarding them and deferring to the vendor logs",
            report.team_slug,
            report.newly_stored_tool_payloads,
            report.newly_stored_tool_payload_bytes as f64 / (1024.0 * 1024.0),
            report.team_slug,
            report.tool_payloads
        );
    }
}

/// The Python edition's ingest-claude-history help, verbatim.
pub const INGEST_HISTORY_HELP: &str = r#"usage: wrkviz ingest-claude-history [-h] --output OUTPUT --team TEAM
                                    [--snapshot-root DIR] --history-file
                                    HISTORY_FILE --project-pattern REGEX
                                    [--covered-session SESSION_UUID]
                                    [--timezone TIMEZONE]
                                    [--start-date START_DATE | --start-time START_TIME]
                                    [--end-date END_DATE | --end-time END_TIME]
                                    [--project LABEL=REPOSITORY_URL]
                                    [--source-host HOSTNAME]

normalize ~/.claude/history.jsonl into one prompt-only team, so prompts whose
transcripts Claude Code has since deleted still reach the timeline and the
prompt projection; does not call a model. Example: wrkviz ingest-claude-
history --history-file ~/.claude/history.jsonl --project-pattern
'/work/widget' --team claude-history-host01 --output ./timelines/widget

options:
  -h, --help            show this help message and exit
  --output OUTPUT       version-controllable archive directory
  --team TEAM           stable team slug, for example example-team
  --snapshot-root DIR   where to keep the vendor source snapshots, which are
                        ingest input rather than published output and are the
                        largest thing here (default: <output>.sources, a
                        sibling of the archive; an archive that already keeps
                        them inside itself keeps doing so until `migrate-
                        snapshots` is run)
  --history-file HISTORY_FILE
                        Claude Code prompt history, normally
                        ~/.claude/history.jsonl; one line per prompt the owner
                        submitted, kept by Claude Code after the transcript
                        itself is cleaned up
  --project-pattern REGEX
                        Python regular expression searched against each
                        prompt's recorded working directory; only matching
                        prompts join this team, for example
                        '/work/widget|/worktrees/widget-'
  --covered-session SESSION_UUID
                        a Claude session whose full transcript is archived as
                        its own team; its prompts are left out of this team so
                        they are not projected twice; repeatable. ingest-
                        project derives this list from the manifest's claude
                        teams by itself
  --timezone TIMEZONE   IANA display/calendar timezone (UTC instants remain
                        canonical)
  --start-date START_DATE
                        first local calendar date to include (YYYY-MM-DD,
                        inclusive)
  --start-time START_TIME
                        first instant to include (RFC3339 with offset or Z,
                        inclusive)
  --end-date END_DATE   local calendar boundary to stop at (YYYY-MM-DD,
                        exclusive)
  --end-time END_TIME   instant to stop at (RFC3339 with offset or Z,
                        exclusive)
  --project LABEL=REPOSITORY_URL
                        project/repository identity; repeat for multi-repo
                        work (the first is primary); Codex git metadata is
                        used when omitted
  --source-host HOSTNAME
                        execution hostname captured by the source data; repeat
                        for multi-host work
"#;

const INGEST_HISTORY_USAGE: &str = "usage: wrkviz ingest-claude-history [-h] --output OUTPUT --team TEAM\n                                    [--snapshot-root DIR] --history-file\n                                    HISTORY_FILE --project-pattern REGEX\n                                    [--covered-session SESSION_UUID]\n                                    [--timezone TIMEZONE]\n                                    [--start-date START_DATE | --start-time START_TIME]\n                                    [--end-date END_DATE | --end-time END_TIME]\n                                    [--project LABEL=REPOSITORY_URL]\n                                    [--source-host HOSTNAME]";

const INGEST_HISTORY_OPTIONS: [&str; 13] = [
    "--output",
    "--team",
    "--snapshot-root",
    "--history-file",
    "--project-pattern",
    "--covered-session",
    "--timezone",
    "--start-date",
    "--start-time",
    "--end-date",
    "--end-time",
    "--project",
    "--source-host",
];

fn run_ingest_claude_history(args: &[String]) -> u8 {
    let mut output: Option<String> = None;
    let mut team: Option<String> = None;
    let mut snapshot_root: Option<String> = None;
    let mut history_file: Option<String> = None;
    let mut project_pattern: Option<String> = None;
    let mut covered: Vec<String> = Vec::new();
    let mut timezone = "America/New_York".to_string();
    let mut start_date: Option<String> = None;
    let mut start_time: Option<String> = None;
    let mut end_date: Option<String> = None;
    let mut end_time: Option<String> = None;
    let mut projects: Vec<String> = Vec::new();
    let mut hosts: Vec<String> = Vec::new();
    let mut extras: Vec<String> = Vec::new();
    let mut sep_extras: Vec<String> = Vec::new();
    let mut seen: Vec<&'static str> = Vec::new();
    let mut separated = false;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if separated {
            sep_extras.push(arg.clone());
            index += 1;
            continue;
        }
        if arg == "--" {
            separated = true;
            index += 1;
            continue;
        }
        // argparse resolution: an exact long name wins, else
        // the prefix must name exactly one candidate, with
        // `--help` itself among the candidates.
        let head = arg.split_once('=').map_or(arg.as_str(), |(head, _)| head);
        if arg == "-h" {
            print!("{INGEST_HISTORY_HELP}");
            return 0;
        }
        if head.len() > 2
            && head.starts_with("--")
            && head != "--help"
            && !INGEST_HISTORY_OPTIONS.contains(&head)
        {
            let mut candidates: Vec<&str> = vec!["--help"];
            candidates.extend(INGEST_HISTORY_OPTIONS.iter().copied());
            let matches: Vec<&str> = candidates
                .into_iter()
                .filter(|name| name.starts_with(head))
                .collect();
            if matches.len() > 1 {
                return argparse_error(
                    INGEST_HISTORY_USAGE,
                    "wrkviz ingest-claude-history",
                    &format!(
                        "ambiguous option: {head} could match {}",
                        matches.join(", ")
                    ),
                );
            }
            if matches == ["--help"] {
                if let Some((_, value)) = arg.split_once('=') {
                    return argparse_error(
                        INGEST_HISTORY_USAGE,
                        "wrkviz ingest-claude-history",
                        &format!("argument -h/--help: ignored explicit argument '{value}'"),
                    );
                }
                print!("{INGEST_HISTORY_HELP}");
                return 0;
            }
        }
        if head == "--help" {
            if let Some((_, value)) = arg.split_once('=') {
                return argparse_error(
                    INGEST_HISTORY_USAGE,
                    "wrkviz ingest-claude-history",
                    &format!("argument -h/--help: ignored explicit argument '{value}'"),
                );
            }
            print!("{INGEST_HISTORY_HELP}");
            return 0;
        }
        let mut handled = true;
        let canonical: Option<&'static str> = if INGEST_HISTORY_OPTIONS.contains(&head) {
            INGEST_HISTORY_OPTIONS
                .iter()
                .copied()
                .find(|name| *name == head)
        } else if head.len() > 2 && head.starts_with("--") {
            let matches: Vec<&'static str> = INGEST_HISTORY_OPTIONS
                .iter()
                .copied()
                .filter(|name| name.starts_with(head))
                .collect();
            matches.first().copied()
        } else {
            None
        };
        let ordered: Vec<&'static str> = canonical
            .into_iter()
            .chain(
                INGEST_HISTORY_OPTIONS
                    .iter()
                    .copied()
                    .filter(|name| Some(*name) != canonical),
            )
            .collect();
        for name in ordered {
            match named_value(args, &mut index, name) {
                Ok(Some(value)) => {
                    seen.push(name);
                    // argparse validates the mutually exclusive
                    // bounds once the second value is in hand.
                    let conflict = match name {
                        "--start-time" if start_date.is_some() => Some("--start-date"),
                        "--start-date" if start_time.is_some() => Some("--start-time"),
                        "--end-time" if end_date.is_some() => Some("--end-date"),
                        "--end-date" if end_time.is_some() => Some("--end-time"),
                        _ => None,
                    };
                    if let Some(other) = conflict {
                        return argparse_error(
                            INGEST_HISTORY_USAGE,
                            "wrkviz ingest-claude-history",
                            &format!("argument {name}: not allowed with argument {other}"),
                        );
                    }
                    match name {
                        "--output" => output = Some(value),
                        "--team" => team = Some(value),
                        "--snapshot-root" => snapshot_root = Some(value),
                        "--history-file" => history_file = Some(value),
                        "--project-pattern" => project_pattern = Some(value),
                        "--covered-session" => covered.push(value),
                        "--timezone" => timezone = value,
                        "--start-date" => start_date = Some(value),
                        "--start-time" => start_time = Some(value),
                        "--end-date" => end_date = Some(value),
                        "--end-time" => end_time = Some(value),
                        "--project" => projects.push(value),
                        "--source-host" => hosts.push(value),
                        _ => {}
                    }
                    break;
                }
                Ok(None) => {}
                Err((_, message)) => {
                    return argparse_error(
                        INGEST_HISTORY_USAGE,
                        "wrkviz ingest-claude-history",
                        &message,
                    )
                }
            }
        }
        if !is_option_token(arg, &INGEST_HISTORY_OPTIONS) {
            handled = false;
        }
        if !handled {
            extras.push(arg.clone());
        }
        index += 1;
    }
    let mut required: Vec<&str> = Vec::new();
    if output.is_none() {
        required.push("--output");
    }
    if team.is_none() {
        required.push("--team");
    }
    if history_file.is_none() {
        required.push("--history-file");
    }
    if project_pattern.is_none() {
        required.push("--project-pattern");
    }
    if !required.is_empty() {
        return argparse_error(
            INGEST_HISTORY_USAGE,
            "wrkviz ingest-claude-history",
            &format!(
                "the following arguments are required: {}",
                required.join(", ")
            ),
        );
    }
    if !extras.is_empty() || !sep_extras.is_empty() {
        let mut reported = extras.clone();
        if !sep_extras.is_empty() {
            reported.push("--".to_string());
            reported.extend(sep_extras.iter().cloned());
        }
        eprintln!(
            "usage: wrkviz [-h] [--version] [--userguide]\n              {{quickstart,ingest-project,ingest,ingest-claude,ingest-claude-history,ingest-muse,ingest-orc,summarize,build,extract-transcripts,export,refresh,github-metadata,refresh-claude,refresh-orc,serve,inspect,gc,migrate-build-state,migrate-snapshots,audit-glossary,audit-losslessness,query,responsiveness,discover-agents}}\n              ...\nwrkviz: error: unrecognized arguments: {}",
            reported.join(" ")
        );
        return 2;
    }
    let output = output.unwrap();
    let team_slug = team.unwrap();
    let history_file = history_file.unwrap();
    let project_pattern = project_pattern.unwrap();
    let archive = cli_path(&output);
    let started = crate::pipeline::utc_now();
    let mut command: Vec<String> = vec!["wrkviz".to_string(), "ingest-claude-history".to_string()];
    command.extend(args.iter().cloned());
    let outcome: Result<crate::ingest::IngestReport, String> = (|| {
        let timezone_explicit = args
            .iter()
            .any(|item| item == "--timezone" || item.starts_with("--timezone="));
        let (parsed_projects, parsed_hosts) =
            crate::identity::parse_identity_overrides(&projects, &hosts)?;
        let overrides = crate::identity::IdentityOverrides {
            projects: parsed_projects,
            hosts: parsed_hosts,
            display_timezone_source: if timezone_explicit {
                "explicit".to_string()
            } else {
                "default".to_string()
            },
        };
        let date_window = crate::window::parse_date_window(
            start_date.as_deref(),
            end_date.as_deref(),
            &timezone,
            start_time.as_deref(),
            end_time.as_deref(),
        )?;
        let (_, report) = crate::ingest::ingest_claude_history(
            &archive,
            &cli_path(&history_file),
            &team_slug,
            &timezone,
            &project_pattern,
            &covered,
            date_window.as_ref(),
            &overrides,
            snapshot_root.as_deref().map(cli_path).as_deref(),
        )?;
        Ok(report)
    })();
    let report = match outcome {
        Ok(report) => report,
        Err(error) => {
            if let Ok(run_path) = crate::pipeline::record_run(
                &archive,
                &crate::pipeline::RunReceipt {
                    command: &command,
                    started_at: &started,
                    status: "failed",
                    team_slug: &team_slug,
                    error: Some(&error),
                    team_slugs: &[],
                    mechanical: None,
                    ingest: None,
                },
            ) {
                eprintln!("run metadata: {}", run_path.display());
            }
            eprintln!("wrkviz: {error}");
            return 2;
        }
    };
    println!(
        "ingest: {} agents, {} messages/events, {} outer tools, {} edges from {} source files ({:.1} MiB); {} archive files changed",
        report.agents,
        report.events,
        report.tool_calls,
        report.edges,
        report.sources,
        report.source_bytes as f64 / (1024.0 * 1024.0),
        report.files_changed
    );
    if report.snapshot_root_established {
        let where_ = crate::json::resolve_path(&archive.join(&report.snapshot_root));
        if report.snapshot_root_layout == "in-archive-legacy" {
            eprintln!(
                "wrkviz: {}: this archive keeps its vendor source snapshots inside itself, at {}; they are ingest input rather than published output, and they are reachable over HTTP from `make serve` until they are moved. `wrkviz migrate-snapshots --output {}` says what moving them would do",
                report.team_slug,
                where_.display(),
                archive.display()
            );
        } else {
            eprintln!(
                "wrkviz: {}: vendor source snapshots are being kept in {}, outside the published archive, because they are ingest input and are usually the largest thing here. The archive records the location; back it up with the archive if you want incremental ingest to keep working",
                report.team_slug,
                where_.display()
            );
        }
    }
    if report.history_unmatched_prompts != 0 || report.history_covered_prompts != 0 {
        let mut parts: Vec<String> = Vec::new();
        if report.history_unmatched_prompts != 0 {
            parts.push(format!(
                "{} prompt(s) typed outside the project pattern",
                report.history_unmatched_prompts
            ));
        }
        if report.history_covered_prompts != 0 {
            parts.push(format!(
                "{} prompt(s) from {} session(s) whose full transcript is archived as its own team",
                report.history_covered_prompts, report.history_covered_sessions
            ));
        }
        eprintln!(
            "wrkviz: {}: left out {}",
            report.team_slug,
            parts.join("; ")
        );
    }
    if report.retired_message_projections != 0 {
        eprintln!(
            "wrkviz: {}: removed {} retired raw/messages/<thread-id>.json file(s) ({:.1} MiB); the tool no longer writes that per-thread projection, which nothing read and which raw/team.json still fully describes",
            report.team_slug,
            report.retired_message_projections,
            report.retired_message_projection_bytes as f64 / (1024.0 * 1024.0)
        );
    }
    if report.newly_promoted_task_notes != 0 {
        eprintln!(
            "wrkviz: {}: promoted {} task note(s) into raw/task-notes.jsonl ({} total); the archive now keeps this text under version control instead of only inside gitignored source_snapshots/",
            report.team_slug, report.newly_promoted_task_notes, report.task_notes
        );
        if report.task_notes_upstream_deleted != 0 {
            eprintln!(
                "wrkviz: {}: {} of those notes no longer exist in the upstream task table; for those this archive is the only copy",
                report.team_slug, report.task_notes_upstream_deleted
            );
        }
    }
    for shard in &report.damaged_payload_shards {
        eprintln!("wrkviz: {}: payload store: {shard}", report.team_slug);
    }
    if !report.pruned_payload_shards.is_empty() {
        eprintln!(
            "wrkviz: {}: {} payload shard(s) recorded by the previous ingest are no longer in the tree; the manifest no longer claims them",
            report.team_slug,
            report.pruned_payload_shards.len()
        );
    }
    if report.newly_stored_tool_payloads != 0 {
        eprintln!(
            "wrkviz: {}: stored {} tool payload(s) ({:.1} MiB) into gitignored teams/{}/payloads/ ({} total); the archive now keeps command arguments and output instead of discarding them and deferring to the vendor logs",
            report.team_slug,
            report.newly_stored_tool_payloads,
            report.newly_stored_tool_payload_bytes as f64 / (1024.0 * 1024.0),
            report.team_slug,
            report.tool_payloads
        );
    }
    let ingest_json = report.to_json();
    match crate::pipeline::record_run(
        &archive,
        &crate::pipeline::RunReceipt {
            command: &command,
            started_at: &started,
            status: "completed",
            team_slug: &team_slug,
            error: None,
            team_slugs: &[],
            mechanical: None,
            ingest: Some(ingest_json),
        },
    ) {
        Ok(run_path) => println!("run metadata: {}", run_path.display()),
        Err(error) => {
            eprintln!("wrkviz: {error}");
            return 2;
        }
    }
    0
}

/// The Python edition's serve help, verbatim.
/// The Python edition's ingest-claude help, verbatim.
pub const INGEST_CLAUDE_HELP: &str = r#"usage: wrkviz ingest-claude [-h] --output OUTPUT --team TEAM
                            [--snapshot-root DIR] --session-file SESSION_FILE
                            [--timezone TIMEZONE]
                            [--start-date START_DATE | --start-time START_TIME]
                            [--end-date END_DATE | --end-time END_TIME]
                            [--project LABEL=REPOSITORY_URL]
                            [--source-host HOSTNAME]

normalize Claude logs; do not call a model

options:
  -h, --help            show this help message and exit
  --output OUTPUT       version-controllable archive directory
  --team TEAM           stable team slug, for example example-team
  --snapshot-root DIR   where to keep the vendor source snapshots, which are
                        ingest input rather than published output and are the
                        largest thing here (default: <output>.sources, a
                        sibling of the archive; an archive that already keeps
                        them inside itself keeps doing so until `migrate-
                        snapshots` is run)
  --session-file SESSION_FILE
                        Claude coordinator JSONL; nested subagents are
                        discovered beside it
  --timezone TIMEZONE   IANA display/calendar timezone (UTC instants remain
                        canonical)
  --start-date START_DATE
                        first local calendar date to include (YYYY-MM-DD,
                        inclusive)
  --start-time START_TIME
                        first instant to include (RFC3339 with offset or Z,
                        inclusive)
  --end-date END_DATE   local calendar boundary to stop at (YYYY-MM-DD,
                        exclusive)
  --end-time END_TIME   instant to stop at (RFC3339 with offset or Z,
                        exclusive)
  --project LABEL=REPOSITORY_URL
                        project/repository identity; repeat for multi-repo
                        work (the first is primary); Codex git metadata is
                        used when omitted
  --source-host HOSTNAME
                        execution hostname captured by the source data; repeat
                        for multi-host work
"#;

const INGEST_CLAUDE_USAGE: &str = "usage: wrkviz ingest-claude [-h] --output OUTPUT --team TEAM\n                            [--snapshot-root DIR] --session-file SESSION_FILE\n                            [--timezone TIMEZONE]\n                            [--start-date START_DATE | --start-time START_TIME]\n                            [--end-date END_DATE | --end-time END_TIME]\n                            [--project LABEL=REPOSITORY_URL]\n                            [--source-host HOSTNAME]";

const INGEST_CLAUDE_OPTIONS: [&str; 11] = [
    "--output",
    "--team",
    "--snapshot-root",
    "--session-file",
    "--timezone",
    "--start-date",
    "--start-time",
    "--end-date",
    "--end-time",
    "--project",
    "--source-host",
];

/// The Python edition's ingest-orc help, verbatim.
pub const INGEST_ORC_HELP: &str = r#"usage: wrkviz ingest-orc [-h] --output OUTPUT --team TEAM
                         [--snapshot-root DIR] --source-root SOURCE_ROOT
                         --root-session ROOT_SESSION
                         [--continuation-session SESSION_OR_JSON]
                         [--accept-orc-prefix-rewrite SESSION]
                         [--timezone TIMEZONE]
                         [--start-date START_DATE | --start-time START_TIME]
                         [--end-date END_DATE | --end-time END_TIME]
                         [--project LABEL=REPOSITORY_URL]
                         [--source-host HOSTNAME]

snapshot and normalize Orc SQLite logs; do not call a model

options:
  -h, --help            show this help message and exit
  --output OUTPUT       version-controllable archive directory
  --team TEAM           stable team slug, for example example-team
  --snapshot-root DIR   where to keep the vendor source snapshots, which are
                        ingest input rather than published output and are the
                        largest thing here (default: <output>.sources, a
                        sibling of the archive; an archive that already keeps
                        them inside itself keeps doing so until `migrate-
                        snapshots` is run)
  --source-root SOURCE_ROOT
                        project root containing Orc .orc/ and task .tg/
                        directories
  --root-session ROOT_SESSION
                        Orc coordinator session UUID
  --continuation-session SESSION_OR_JSON
                        explicit successor coordinator session; use a session
                        id for the whole session or compact JSON with
                        session_id and start_message_id for a reused session;
                        repeat in chronological order
  --accept-orc-prefix-rewrite SESSION
                        operator override: accept this one session's recorded
                        append prefix having been rewritten in place, re-
                        baselining its digest and marking the source degraded;
                        repeat per session; the changed rows and columns are
                        printed and recorded; a rewritten session not named
                        here still refuses, and rows that disappeared from or
                        appeared inside the prefix are refused for every
                        session
  --timezone TIMEZONE   IANA display/calendar timezone (UTC instants remain
                        canonical)
  --start-date START_DATE
                        first local calendar date to include (YYYY-MM-DD,
                        inclusive)
  --start-time START_TIME
                        first instant to include (RFC3339 with offset or Z,
                        inclusive)
  --end-date END_DATE   local calendar boundary to stop at (YYYY-MM-DD,
                        exclusive)
  --end-time END_TIME   instant to stop at (RFC3339 with offset or Z,
                        exclusive)
  --project LABEL=REPOSITORY_URL
                        project/repository identity; repeat for multi-repo
                        work (the first is primary); Codex git metadata is
                        used when omitted
  --source-host HOSTNAME
                        execution hostname captured by the source data; repeat
                        for multi-host work
"#;

const INGEST_ORC_USAGE: &str = "usage: wrkviz ingest-orc [-h] --output OUTPUT --team TEAM\n                         [--snapshot-root DIR] --source-root SOURCE_ROOT\n                         --root-session ROOT_SESSION\n                         [--continuation-session SESSION_OR_JSON]\n                         [--accept-orc-prefix-rewrite SESSION]\n                         [--timezone TIMEZONE]\n                         [--start-date START_DATE | --start-time START_TIME]\n                         [--end-date END_DATE | --end-time END_TIME]\n                         [--project LABEL=REPOSITORY_URL]\n                         [--source-host HOSTNAME]";

const INGEST_ORC_OPTIONS: [&str; 14] = [
    "--output",
    "--team",
    "--snapshot-root",
    "--source-root",
    "--root-session",
    "--continuation-session",
    "--accept-orc-prefix-rewrite",
    "--timezone",
    "--start-date",
    "--start-time",
    "--end-date",
    "--end-time",
    "--project",
    "--source-host",
];

/// The Python edition's ingest-project help, verbatim.
pub const INGEST_PROJECT_HELP: &str = r#"usage: wrkviz ingest-project [-h] --config CONFIG [--team TEAM]
                             [--accept-orc-prefix-rewrite TEAM:SESSION]

ingest teams from a strict JSON manifest, then refresh the global transcript
JSONL; does not call a model or build the website

options:
  -h, --help            show this help message and exit
  --config CONFIG       schema-v1 project ingest JSON file
  --team TEAM           registered team slug to ingest; repeat as needed
                        (default: every team); transcript extraction still
                        covers every normalized archive team
  --accept-orc-prefix-rewrite TEAM:SESSION
                        operator override: accept an in-place append-prefix
                        rewrite for this one session of this one Orc team, re-
                        baselining its digest and marking the source degraded;
                        repeat per session; the slug must be an Orc team this
                        run ingests and the session id is the one that team's
                        refusal named
"#;

const INGEST_PROJECT_USAGE: &str = "usage: wrkviz ingest-project [-h] --config CONFIG [--team TEAM]\n                             [--accept-orc-prefix-rewrite TEAM:SESSION]";

const INGEST_PROJECT_OPTIONS: [&str; 3] = ["--config", "--team", "--accept-orc-prefix-rewrite"];

fn run_ingest_project(args: &[String]) -> u8 {
    let mut config: Option<String> = None;
    let mut teams: Vec<String> = Vec::new();
    let mut accepts: Vec<String> = Vec::new();
    let mut extras: Vec<String> = Vec::new();
    let mut sep_extras: Vec<String> = Vec::new();
    let mut seen: Vec<&'static str> = Vec::new();
    let mut separated = false;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if separated {
            sep_extras.push(arg.clone());
            index += 1;
            continue;
        }
        if arg == "--" {
            separated = true;
            index += 1;
            continue;
        }
        // argparse resolution: an exact long name wins, else
        // the prefix must name exactly one candidate, with
        // `--help` itself among the candidates.
        let head = arg.split_once('=').map_or(arg.as_str(), |(head, _)| head);
        if arg == "-h" || (arg.len() > 2 && arg.starts_with("-h") && !arg.starts_with("--")) {
            // argparse fires the `-h` action at the head of a
            // single-dash cluster (`-help`) too.
            print!("{INGEST_PROJECT_HELP}");
            return 0;
        }
        if head.len() > 2
            && head.starts_with("--")
            && head != "--help"
            && !INGEST_PROJECT_OPTIONS.contains(&head)
        {
            let mut candidates: Vec<&str> = vec!["--help"];
            candidates.extend(INGEST_PROJECT_OPTIONS.iter().copied());
            let matches: Vec<&str> = candidates
                .into_iter()
                .filter(|name| name.starts_with(head))
                .collect();
            if matches.len() > 1 {
                return argparse_error(
                    INGEST_PROJECT_USAGE,
                    "wrkviz ingest-project",
                    &format!(
                        "ambiguous option: {head} could match {}",
                        matches.join(", ")
                    ),
                );
            }
            if matches == ["--help"] {
                if let Some((_, value)) = arg.split_once('=') {
                    return argparse_error(
                        INGEST_PROJECT_USAGE,
                        "wrkviz ingest-project",
                        &format!("argument -h/--help: ignored explicit argument '{value}'"),
                    );
                }
                print!("{INGEST_PROJECT_HELP}");
                return 0;
            }
        }
        if head == "--help" {
            if let Some((_, value)) = arg.split_once('=') {
                return argparse_error(
                    INGEST_PROJECT_USAGE,
                    "wrkviz ingest-project",
                    &format!("argument -h/--help: ignored explicit argument '{value}'"),
                );
            }
            print!("{INGEST_PROJECT_HELP}");
            return 0;
        }
        let mut handled = true;
        let canonical: Option<&'static str> = if INGEST_PROJECT_OPTIONS.contains(&head) {
            INGEST_PROJECT_OPTIONS
                .iter()
                .copied()
                .find(|name| *name == head)
        } else if head.len() > 2 && head.starts_with("--") {
            let matches: Vec<&'static str> = INGEST_PROJECT_OPTIONS
                .iter()
                .copied()
                .filter(|name| name.starts_with(head))
                .collect();
            matches.first().copied()
        } else {
            None
        };
        let ordered: Vec<&'static str> = canonical
            .into_iter()
            .chain(
                INGEST_PROJECT_OPTIONS
                    .iter()
                    .copied()
                    .filter(|name| Some(*name) != canonical),
            )
            .collect();
        for name in ordered {
            match named_value(args, &mut index, name) {
                Ok(Some(value)) => {
                    seen.push(name);
                    // argparse validates the mutually exclusive
                    // bounds once the second value is in hand.
                    let conflict: Option<&str> = None;
                    if let Some(other) = conflict {
                        return argparse_error(
                            INGEST_PROJECT_USAGE,
                            "wrkviz ingest-project",
                            &format!("argument {name}: not allowed with argument {other}"),
                        );
                    }
                    match name {
                        "--config" => config = Some(value),
                        "--team" => teams.push(value),
                        "--accept-orc-prefix-rewrite" => accepts.push(value),
                        _ => {}
                    }
                    break;
                }
                Ok(None) => {}
                Err((_, message)) => {
                    return argparse_error(INGEST_PROJECT_USAGE, "wrkviz ingest-project", &message)
                }
            }
        }
        if !is_option_token(arg, &INGEST_PROJECT_OPTIONS) {
            handled = false;
        }
        if !handled {
            extras.push(arg.clone());
        }
        index += 1;
    }
    let mut required: Vec<&str> = Vec::new();
    if config.is_none() {
        required.push("--config");
    }
    if !required.is_empty() {
        return argparse_error(
            INGEST_PROJECT_USAGE,
            "wrkviz ingest-project",
            &format!(
                "the following arguments are required: {}",
                required.join(", ")
            ),
        );
    }
    if !extras.is_empty() || !sep_extras.is_empty() {
        let mut reported = extras.clone();
        if !sep_extras.is_empty() {
            reported.push("--".to_string());
            reported.extend(sep_extras.iter().cloned());
        }
        eprintln!(
            "usage: wrkviz [-h] [--version] [--userguide]\n              {{quickstart,ingest-project,ingest,ingest-claude,ingest-claude-history,ingest-muse,ingest-orc,summarize,build,extract-transcripts,export,refresh,github-metadata,refresh-claude,refresh-orc,serve,inspect,gc,migrate-build-state,migrate-snapshots,audit-glossary,audit-losslessness,query,responsiveness,discover-agents}}\n              ...\nwrkviz: error: unrecognized arguments: {}",
            reported.join(" ")
        );
        return 2;
    }
    let config_raw = config.unwrap();
    let started = crate::pipeline::utc_now();
    let mut command: Vec<String> = vec!["wrkviz".to_string(), "ingest-project".to_string()];
    command.extend(args.iter().cloned());
    let loaded: Result<(crate::project_config::ProjectIngestConfig, Vec<String>), String> =
        (|| {
            let config = crate::project_config::load_project_ingest_config(&cli_path(&config_raw))?;
            let selected: Vec<String> = config
                .select_teams(&teams)?
                .iter()
                .map(|team| team.slug.clone())
                .collect();
            Ok((config, selected))
        })();
    let (config, selected_slugs) = match loaded {
        Ok(pair) => pair,
        Err(error) => {
            eprintln!("wrkviz: {error}");
            return 2;
        }
    };
    let archive = config.output.clone();
    let minimal_receipt = |config: &crate::project_config::ProjectIngestConfig,
                           selected: &[String],
                           error: &str|
     -> Option<PathBuf> {
        let mut inner = serde_json::Map::new();
        inner.insert("schema_version".into(), 1.into());
        inner.insert(
            "config_sha256".into(),
            serde_json::Value::String(config.config_sha256.clone()),
        );
        inner.insert("model_calls".into(), 0.into());
        inner.insert("model_tokens".into(), 0.into());
        inner.insert(
            "website_build_performed".into(),
            serde_json::Value::Bool(false),
        );
        let mut mechanical = serde_json::Map::new();
        mechanical.insert("project_ingest".into(), serde_json::Value::Object(inner));
        crate::pipeline::record_run(
            &config.output,
            &crate::pipeline::RunReceipt {
                command: &command,
                started_at: &started,
                status: "failed",
                team_slug: &selected[0],
                error: Some(error),
                team_slugs: selected,
                mechanical: Some(serde_json::Value::Object(mechanical)),
                ingest: None,
            },
        )
        .ok()
    };
    let report = match crate::project_config::ingest_project(&config, &selected_slugs, &accepts) {
        Ok(report) => report,
        Err(error) => {
            if let Some(run_path) = minimal_receipt(&config, &selected_slugs, &error) {
                eprintln!("run metadata: {}", run_path.display());
            }
            eprintln!("wrkviz: {error}");
            return 2;
        }
    };
    let failure_summary = report.failure_summary();
    let mut mechanical = serde_json::Map::new();
    mechanical.insert("project_ingest".into(), report.to_json());
    let run_path = match crate::pipeline::record_run(
        &archive,
        &crate::pipeline::RunReceipt {
            command: &command,
            started_at: &started,
            status: if report.failed() {
                "failed"
            } else {
                "completed"
            },
            team_slug: &selected_slugs[0],
            error: failure_summary.as_deref(),
            team_slugs: &selected_slugs,
            mechanical: Some(serde_json::Value::Object(mechanical)),
            ingest: None,
        },
    ) {
        Ok(path) => path,
        Err(error) => {
            if let Some(run_path) = minimal_receipt(&config, &selected_slugs, &error) {
                eprintln!("run metadata: {}", run_path.display());
            }
            eprintln!("wrkviz: {error}");
            return 2;
        }
    };
    for team in &report.teams {
        print!("{} ({}): ", team.team_slug, team.provider);
        std::io::Write::flush(&mut std::io::stdout()).ok();
        print_ingest_receipt(&team.ingest, &archive);
        for override_ in &team.ingest.orc_prefix_overrides {
            for line in override_.describe() {
                eprintln!("wrkviz: {}: {line}", team.ingest.team_slug);
            }
        }
    }
    for failure in &report.failures {
        eprintln!("wrkviz: team {}", failure.summary());
    }
    println!(
        "teams: {} succeeded, {} failed",
        report.teams.len(),
        report.failures.len()
    );
    match &report.transcripts {
        None => {
            eprintln!(
                "wrkviz: transcript extraction failed: {}",
                report.transcript_error.as_deref().unwrap_or_default()
            );
            println!("transcripts: not extracted");
        }
        Some(transcripts) => {
            for skip in &transcripts.skipped_teams {
                eprintln!(
                    "wrkviz: transcripts: team {} -- carried forward unchanged from its last good extraction; nothing since is projected",
                    skip.summary()
                );
            }
            for dropped in &transcripts.dropped_authorship_rules {
                eprintln!(
                    "wrkviz: transcripts: prompt authorship rule {} -- not applied",
                    dropped.summary()
                );
            }
            let skipped_note = if transcripts.skipped_teams.is_empty() {
                String::new()
            } else {
                format!(
                    " ({} further team(s) carried forward unread)",
                    transcripts.skipped_teams.len()
                )
            };
            println!(
                "transcripts: {} prompts, {} responses, {} system inputs across {} normalized archive teams{}; {} files changed",
                transcripts.prompts,
                transcripts.responses,
                transcripts.system_inputs,
                transcripts.teams,
                skipped_note,
                transcripts.files_changed
            );
            println!(
                "JSONL: {}",
                archive
                    .join("extracted")
                    .join("transcripts")
                    .join("prompts.jsonl.gz")
                    .display()
            );
        }
    }
    println!("website: not built (run `wrkviz build` separately)");
    println!("run metadata: {}", run_path.display());
    if report.failed() {
        eprintln!("wrkviz: {}", failure_summary.unwrap_or_default());
        return 2;
    }
    0
}

fn run_ingest_session_orc(args: &[String]) -> u8 {
    let mut output: Option<String> = None;
    let mut team: Option<String> = None;
    let mut snapshot_root: Option<String> = None;
    let mut source_root: Option<String> = None;
    let mut root_session: Option<String> = None;
    let mut continuation_sessions: Vec<String> = Vec::new();
    let mut accept_rewrites: Vec<String> = Vec::new();

    let mut timezone = "America/New_York".to_string();
    let mut start_date: Option<String> = None;
    let mut start_time: Option<String> = None;
    let mut end_date: Option<String> = None;
    let mut end_time: Option<String> = None;
    let mut projects: Vec<String> = Vec::new();
    let mut hosts: Vec<String> = Vec::new();
    let mut extras: Vec<String> = Vec::new();
    let mut sep_extras: Vec<String> = Vec::new();
    let mut seen: Vec<&'static str> = Vec::new();
    let mut separated = false;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if separated {
            sep_extras.push(arg.clone());
            index += 1;
            continue;
        }
        if arg == "--" {
            separated = true;
            index += 1;
            continue;
        }
        // argparse resolution: an exact long name wins, else
        // the prefix must name exactly one candidate, with
        // `--help` itself among the candidates.
        let head = arg.split_once('=').map_or(arg.as_str(), |(head, _)| head);
        if arg == "-h" || (arg.len() > 2 && arg.starts_with("-h") && !arg.starts_with("--")) {
            // argparse fires the `-h` action at the head of a
            // single-dash cluster (`-help`) too.
            print!("{INGEST_ORC_HELP}");
            return 0;
        }
        if head.len() > 2
            && head.starts_with("--")
            && head != "--help"
            && !INGEST_ORC_OPTIONS.contains(&head)
        {
            let mut candidates: Vec<&str> = vec!["--help"];
            candidates.extend(INGEST_ORC_OPTIONS.iter().copied());
            let matches: Vec<&str> = candidates
                .into_iter()
                .filter(|name| name.starts_with(head))
                .collect();
            if matches.len() > 1 {
                return argparse_error(
                    INGEST_ORC_USAGE,
                    "wrkviz ingest-orc",
                    &format!(
                        "ambiguous option: {head} could match {}",
                        matches.join(", ")
                    ),
                );
            }
            if matches == ["--help"] {
                if let Some((_, value)) = arg.split_once('=') {
                    return argparse_error(
                        INGEST_ORC_USAGE,
                        "wrkviz ingest-orc",
                        &format!("argument -h/--help: ignored explicit argument '{value}'"),
                    );
                }
                print!("{INGEST_ORC_HELP}");
                return 0;
            }
        }
        if head == "--help" {
            if let Some((_, value)) = arg.split_once('=') {
                return argparse_error(
                    INGEST_ORC_USAGE,
                    "wrkviz ingest-orc",
                    &format!("argument -h/--help: ignored explicit argument '{value}'"),
                );
            }
            print!("{INGEST_ORC_HELP}");
            return 0;
        }
        let mut handled = true;
        let canonical: Option<&'static str> = if INGEST_ORC_OPTIONS.contains(&head) {
            INGEST_ORC_OPTIONS
                .iter()
                .copied()
                .find(|name| *name == head)
        } else if head.len() > 2 && head.starts_with("--") {
            let matches: Vec<&'static str> = INGEST_ORC_OPTIONS
                .iter()
                .copied()
                .filter(|name| name.starts_with(head))
                .collect();
            matches.first().copied()
        } else {
            None
        };
        let ordered: Vec<&'static str> = canonical
            .into_iter()
            .chain(
                INGEST_ORC_OPTIONS
                    .iter()
                    .copied()
                    .filter(|name| Some(*name) != canonical),
            )
            .collect();
        for name in ordered {
            match named_value(args, &mut index, name) {
                Ok(Some(value)) => {
                    seen.push(name);
                    // argparse validates the mutually exclusive
                    // bounds once the second value is in hand.
                    let conflict = match name {
                        "--start-time" if start_date.is_some() => Some("--start-date"),
                        "--start-date" if start_time.is_some() => Some("--start-time"),
                        "--end-time" if end_date.is_some() => Some("--end-date"),
                        "--end-date" if end_time.is_some() => Some("--end-time"),
                        _ => None,
                    };
                    if let Some(other) = conflict {
                        return argparse_error(
                            INGEST_ORC_USAGE,
                            "wrkviz ingest-orc",
                            &format!("argument {name}: not allowed with argument {other}"),
                        );
                    }
                    match name {
                        "--output" => output = Some(value),
                        "--team" => team = Some(value),
                        "--snapshot-root" => snapshot_root = Some(value),
                        "--source-root" => source_root = Some(value),
                        "--root-session" => root_session = Some(value),
                        "--continuation-session" => continuation_sessions.push(value),
                        "--accept-orc-prefix-rewrite" => accept_rewrites.push(value),

                        "--timezone" => timezone = value,
                        "--start-date" => start_date = Some(value),
                        "--start-time" => start_time = Some(value),
                        "--end-date" => end_date = Some(value),
                        "--end-time" => end_time = Some(value),
                        "--project" => projects.push(value),
                        "--source-host" => hosts.push(value),
                        _ => {}
                    }
                    break;
                }
                Ok(None) => {}
                Err((_, message)) => {
                    return argparse_error(INGEST_ORC_USAGE, "wrkviz ingest-orc", &message)
                }
            }
        }
        if !is_option_token(arg, &INGEST_ORC_OPTIONS) {
            handled = false;
        }
        if !handled {
            extras.push(arg.clone());
        }
        index += 1;
    }
    let mut required: Vec<&str> = Vec::new();
    if output.is_none() {
        required.push("--output");
    }
    if team.is_none() {
        required.push("--team");
    }
    if source_root.is_none() {
        required.push("--source-root");
    }
    if root_session.is_none() {
        required.push("--root-session");
    }
    if !required.is_empty() {
        return argparse_error(
            INGEST_ORC_USAGE,
            "wrkviz ingest-orc",
            &format!(
                "the following arguments are required: {}",
                required.join(", ")
            ),
        );
    }
    if !extras.is_empty() || !sep_extras.is_empty() {
        let mut reported = extras.clone();
        if !sep_extras.is_empty() {
            reported.push("--".to_string());
            reported.extend(sep_extras.iter().cloned());
        }
        eprintln!(
            "usage: wrkviz [-h] [--version] [--userguide]\n              {{quickstart,ingest-project,ingest,ingest-claude,ingest-claude-history,ingest-muse,ingest-orc,summarize,build,extract-transcripts,export,refresh,github-metadata,refresh-claude,refresh-orc,serve,inspect,gc,migrate-build-state,migrate-snapshots,audit-glossary,audit-losslessness,query,responsiveness,discover-agents}}\n              ...\nwrkviz: error: unrecognized arguments: {}",
            reported.join(" ")
        );
        return 2;
    }
    let output = output.unwrap();
    let team_slug = team.unwrap();
    let source_root = source_root.unwrap();
    let root_session = root_session.unwrap();
    let archive = cli_path(&output);
    let started = crate::pipeline::utc_now();
    let mut command: Vec<String> = vec!["wrkviz".to_string(), "ingest-orc".to_string()];
    command.extend(args.iter().cloned());
    let outcome: Result<crate::ingest::IngestReport, String> = (|| {
        let timezone_explicit = args
            .iter()
            .any(|item| item == "--timezone" || item.starts_with("--timezone="));
        let (parsed_projects, parsed_hosts) =
            crate::identity::parse_identity_overrides(&projects, &hosts)?;
        let overrides = crate::identity::IdentityOverrides {
            projects: parsed_projects,
            hosts: parsed_hosts,
            display_timezone_source: if timezone_explicit {
                "explicit".to_string()
            } else {
                "default".to_string()
            },
        };
        let date_window = crate::window::parse_date_window(
            start_date.as_deref(),
            end_date.as_deref(),
            &timezone,
            start_time.as_deref(),
            end_time.as_deref(),
        )?;
        let mut continuations: Vec<crate::orc::ContinuationSpec> = Vec::new();
        for raw in &continuation_sessions {
            continuations.push(orc_continuation_arg(raw)?);
        }
        let (_, report) = crate::ingest::ingest_orc(
            &archive,
            &cli_path(&source_root),
            &root_session,
            &team_slug,
            &timezone,
            date_window.as_ref(),
            &overrides,
            &continuations,
            &accept_rewrites,
            snapshot_root.as_deref().map(cli_path).as_deref(),
        )?;
        Ok(report)
    })();
    let report = match outcome {
        Ok(report) => report,
        Err(error) => {
            if let Ok(run_path) = crate::pipeline::record_run(
                &archive,
                &crate::pipeline::RunReceipt {
                    command: &command,
                    started_at: &started,
                    status: "failed",
                    team_slug: &team_slug,
                    error: Some(&error),
                    team_slugs: &[],
                    mechanical: None,
                    ingest: None,
                },
            ) {
                eprintln!("run metadata: {}", run_path.display());
            }
            eprintln!("wrkviz: {error}");
            return 2;
        }
    };
    print_ingest_receipt(&report, &archive);
    for override_ in &report.orc_prefix_overrides {
        for line in override_.describe() {
            eprintln!("wrkviz: {}: {line}", report.team_slug);
        }
    }
    let ingest_json = report.to_json();
    match crate::pipeline::record_run(
        &archive,
        &crate::pipeline::RunReceipt {
            command: &command,
            started_at: &started,
            status: "completed",
            team_slug: &team_slug,
            error: None,
            team_slugs: &[],
            mechanical: None,
            ingest: Some(ingest_json),
        },
    ) {
        Ok(run_path) => println!("run metadata: {}", run_path.display()),
        Err(error) => {
            eprintln!("wrkviz: {error}");
            return 2;
        }
    }
    0
}

fn orc_continuation_arg(raw: &str) -> Result<crate::orc::ContinuationSpec, String> {
    let value: serde_json::Value = if raw.trim_start().starts_with('{') {
        serde_json::from_str(raw)
            .map_err(|error| format!("invalid --continuation-session JSON: {error}"))?
    } else {
        serde_json::Value::String(raw.to_string())
    };
    let is_string = value.as_str().is_some();
    let spec = crate::orc::ContinuationSpec::from_value(&value, "--continuation-session")?;
    if !is_string && spec.start_message_id.is_none() {
        return Err(
            "--continuation-session.start_message_id: expected a non-empty string".to_string(),
        );
    }
    Ok(spec)
}

fn run_ingest_session_claude(args: &[String]) -> u8 {
    let mut output: Option<String> = None;
    let mut team: Option<String> = None;
    let mut snapshot_root: Option<String> = None;
    let mut session_file: Option<String> = None;

    let mut timezone = "America/New_York".to_string();
    let mut start_date: Option<String> = None;
    let mut start_time: Option<String> = None;
    let mut end_date: Option<String> = None;
    let mut end_time: Option<String> = None;
    let mut projects: Vec<String> = Vec::new();
    let mut hosts: Vec<String> = Vec::new();
    let mut extras: Vec<String> = Vec::new();
    let mut sep_extras: Vec<String> = Vec::new();
    let mut seen: Vec<&'static str> = Vec::new();
    let mut separated = false;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if separated {
            sep_extras.push(arg.clone());
            index += 1;
            continue;
        }
        if arg == "--" {
            separated = true;
            index += 1;
            continue;
        }
        // argparse resolution: an exact long name wins, else
        // the prefix must name exactly one candidate, with
        // `--help` itself among the candidates.
        let head = arg.split_once('=').map_or(arg.as_str(), |(head, _)| head);
        if arg == "-h" || (arg.len() > 2 && arg.starts_with("-h") && !arg.starts_with("--")) {
            // argparse fires the `-h` action at the head of a
            // single-dash cluster (`-help`) too.
            print!("{INGEST_CLAUDE_HELP}");
            return 0;
        }
        if head.len() > 2
            && head.starts_with("--")
            && head != "--help"
            && !INGEST_CLAUDE_OPTIONS.contains(&head)
        {
            let mut candidates: Vec<&str> = vec!["--help"];
            candidates.extend(INGEST_CLAUDE_OPTIONS.iter().copied());
            let matches: Vec<&str> = candidates
                .into_iter()
                .filter(|name| name.starts_with(head))
                .collect();
            if matches.len() > 1 {
                return argparse_error(
                    INGEST_CLAUDE_USAGE,
                    "wrkviz ingest-claude",
                    &format!(
                        "ambiguous option: {head} could match {}",
                        matches.join(", ")
                    ),
                );
            }
            if matches == ["--help"] {
                if let Some((_, value)) = arg.split_once('=') {
                    return argparse_error(
                        INGEST_CLAUDE_USAGE,
                        "wrkviz ingest-claude",
                        &format!("argument -h/--help: ignored explicit argument '{value}'"),
                    );
                }
                print!("{INGEST_CLAUDE_HELP}");
                return 0;
            }
        }
        if head == "--help" {
            if let Some((_, value)) = arg.split_once('=') {
                return argparse_error(
                    INGEST_CLAUDE_USAGE,
                    "wrkviz ingest-claude",
                    &format!("argument -h/--help: ignored explicit argument '{value}'"),
                );
            }
            print!("{INGEST_CLAUDE_HELP}");
            return 0;
        }
        let mut handled = true;
        let canonical: Option<&'static str> = if INGEST_CLAUDE_OPTIONS.contains(&head) {
            INGEST_CLAUDE_OPTIONS
                .iter()
                .copied()
                .find(|name| *name == head)
        } else if head.len() > 2 && head.starts_with("--") {
            let matches: Vec<&'static str> = INGEST_CLAUDE_OPTIONS
                .iter()
                .copied()
                .filter(|name| name.starts_with(head))
                .collect();
            matches.first().copied()
        } else {
            None
        };
        let ordered: Vec<&'static str> = canonical
            .into_iter()
            .chain(
                INGEST_CLAUDE_OPTIONS
                    .iter()
                    .copied()
                    .filter(|name| Some(*name) != canonical),
            )
            .collect();
        for name in ordered {
            match named_value(args, &mut index, name) {
                Ok(Some(value)) => {
                    seen.push(name);
                    // argparse validates the mutually exclusive
                    // bounds once the second value is in hand.
                    let conflict = match name {
                        "--start-time" if start_date.is_some() => Some("--start-date"),
                        "--start-date" if start_time.is_some() => Some("--start-time"),
                        "--end-time" if end_date.is_some() => Some("--end-date"),
                        "--end-date" if end_time.is_some() => Some("--end-time"),
                        _ => None,
                    };
                    if let Some(other) = conflict {
                        return argparse_error(
                            INGEST_CLAUDE_USAGE,
                            "wrkviz ingest-claude",
                            &format!("argument {name}: not allowed with argument {other}"),
                        );
                    }
                    match name {
                        "--output" => output = Some(value),
                        "--team" => team = Some(value),
                        "--snapshot-root" => snapshot_root = Some(value),
                        "--session-file" => session_file = Some(value),

                        "--timezone" => timezone = value,
                        "--start-date" => start_date = Some(value),
                        "--start-time" => start_time = Some(value),
                        "--end-date" => end_date = Some(value),
                        "--end-time" => end_time = Some(value),
                        "--project" => projects.push(value),
                        "--source-host" => hosts.push(value),
                        _ => {}
                    }
                    break;
                }
                Ok(None) => {}
                Err((_, message)) => {
                    return argparse_error(INGEST_CLAUDE_USAGE, "wrkviz ingest-claude", &message)
                }
            }
        }
        if !is_option_token(arg, &INGEST_CLAUDE_OPTIONS) {
            handled = false;
        }
        if !handled {
            extras.push(arg.clone());
        }
        index += 1;
    }
    let mut required: Vec<&str> = Vec::new();
    if output.is_none() {
        required.push("--output");
    }
    if team.is_none() {
        required.push("--team");
    }
    if session_file.is_none() {
        required.push("--session-file");
    }
    if !required.is_empty() {
        return argparse_error(
            INGEST_CLAUDE_USAGE,
            "wrkviz ingest-claude",
            &format!(
                "the following arguments are required: {}",
                required.join(", ")
            ),
        );
    }
    if !extras.is_empty() || !sep_extras.is_empty() {
        let mut reported = extras.clone();
        if !sep_extras.is_empty() {
            reported.push("--".to_string());
            reported.extend(sep_extras.iter().cloned());
        }
        eprintln!(
            "usage: wrkviz [-h] [--version] [--userguide]\n              {{quickstart,ingest-project,ingest,ingest-claude,ingest-claude-history,ingest-muse,ingest-orc,summarize,build,extract-transcripts,export,refresh,github-metadata,refresh-claude,refresh-orc,serve,inspect,gc,migrate-build-state,migrate-snapshots,audit-glossary,audit-losslessness,query,responsiveness,discover-agents}}\n              ...\nwrkviz: error: unrecognized arguments: {}",
            reported.join(" ")
        );
        return 2;
    }
    let output = output.unwrap();
    let team_slug = team.unwrap();
    let session_file = session_file.unwrap();
    let archive = cli_path(&output);
    let started = crate::pipeline::utc_now();
    let mut command: Vec<String> = vec!["wrkviz".to_string(), "ingest-claude".to_string()];
    command.extend(args.iter().cloned());
    let outcome: Result<crate::ingest::IngestReport, String> = (|| {
        let timezone_explicit = args
            .iter()
            .any(|item| item == "--timezone" || item.starts_with("--timezone="));
        let (parsed_projects, parsed_hosts) =
            crate::identity::parse_identity_overrides(&projects, &hosts)?;
        let overrides = crate::identity::IdentityOverrides {
            projects: parsed_projects,
            hosts: parsed_hosts,
            display_timezone_source: if timezone_explicit {
                "explicit".to_string()
            } else {
                "default".to_string()
            },
        };
        let date_window = crate::window::parse_date_window(
            start_date.as_deref(),
            end_date.as_deref(),
            &timezone,
            start_time.as_deref(),
            end_time.as_deref(),
        )?;
        let (_, report) = crate::ingest::ingest_claude(
            &archive,
            &cli_path(&session_file),
            &team_slug,
            &timezone,
            date_window.as_ref(),
            &overrides,
            snapshot_root.as_deref().map(cli_path).as_deref(),
        )?;
        Ok(report)
    })();
    let report = match outcome {
        Ok(report) => report,
        Err(error) => {
            if let Ok(run_path) = crate::pipeline::record_run(
                &archive,
                &crate::pipeline::RunReceipt {
                    command: &command,
                    started_at: &started,
                    status: "failed",
                    team_slug: &team_slug,
                    error: Some(&error),
                    team_slugs: &[],
                    mechanical: None,
                    ingest: None,
                },
            ) {
                eprintln!("run metadata: {}", run_path.display());
            }
            eprintln!("wrkviz: {error}");
            return 2;
        }
    };
    println!(
        "ingest: {} agents, {} messages/events, {} outer tools, {} edges from {} source files ({:.1} MiB); {} archive files changed",
        report.agents,
        report.events,
        report.tool_calls,
        report.edges,
        report.sources,
        report.source_bytes as f64 / (1024.0 * 1024.0),
        report.files_changed
    );
    if report.snapshot_root_established {
        let where_ = crate::json::resolve_path(&archive.join(&report.snapshot_root));
        if report.snapshot_root_layout == "in-archive-legacy" {
            eprintln!(
                "wrkviz: {}: this archive keeps its vendor source snapshots inside itself, at {}; they are ingest input rather than published output, and they are reachable over HTTP from `make serve` until they are moved. `wrkviz migrate-snapshots --output {}` says what moving them would do",
                report.team_slug,
                where_.display(),
                archive.display()
            );
        } else {
            eprintln!(
                "wrkviz: {}: vendor source snapshots are being kept in {}, outside the published archive, because they are ingest input and are usually the largest thing here. The archive records the location; back it up with the archive if you want incremental ingest to keep working",
                report.team_slug,
                where_.display()
            );
        }
    }
    if report.history_unmatched_prompts != 0 || report.history_covered_prompts != 0 {
        let mut parts: Vec<String> = Vec::new();
        if report.history_unmatched_prompts != 0 {
            parts.push(format!(
                "{} prompt(s) typed outside the project pattern",
                report.history_unmatched_prompts
            ));
        }
        if report.history_covered_prompts != 0 {
            parts.push(format!(
                "{} prompt(s) from {} session(s) whose full transcript is archived as its own team",
                report.history_covered_prompts, report.history_covered_sessions
            ));
        }
        eprintln!(
            "wrkviz: {}: left out {}",
            report.team_slug,
            parts.join("; ")
        );
    }
    if report.retired_message_projections != 0 {
        eprintln!(
            "wrkviz: {}: removed {} retired raw/messages/<thread-id>.json file(s) ({:.1} MiB); the tool no longer writes that per-thread projection, which nothing read and which raw/team.json still fully describes",
            report.team_slug,
            report.retired_message_projections,
            report.retired_message_projection_bytes as f64 / (1024.0 * 1024.0)
        );
    }
    if report.newly_promoted_task_notes != 0 {
        eprintln!(
            "wrkviz: {}: promoted {} task note(s) into raw/task-notes.jsonl ({} total); the archive now keeps this text under version control instead of only inside gitignored source_snapshots/",
            report.team_slug, report.newly_promoted_task_notes, report.task_notes
        );
        if report.task_notes_upstream_deleted != 0 {
            eprintln!(
                "wrkviz: {}: {} of those notes no longer exist in the upstream task table; for those this archive is the only copy",
                report.team_slug, report.task_notes_upstream_deleted
            );
        }
    }
    for shard in &report.damaged_payload_shards {
        eprintln!("wrkviz: {}: payload store: {shard}", report.team_slug);
    }
    if !report.pruned_payload_shards.is_empty() {
        eprintln!(
            "wrkviz: {}: {} payload shard(s) recorded by the previous ingest are no longer in the tree; the manifest no longer claims them",
            report.team_slug,
            report.pruned_payload_shards.len()
        );
    }
    if report.newly_stored_tool_payloads != 0 {
        eprintln!(
            "wrkviz: {}: stored {} tool payload(s) ({:.1} MiB) into gitignored teams/{}/payloads/ ({} total); the archive now keeps command arguments and output instead of discarding them and deferring to the vendor logs",
            report.team_slug,
            report.newly_stored_tool_payloads,
            report.newly_stored_tool_payload_bytes as f64 / (1024.0 * 1024.0),
            report.team_slug,
            report.tool_payloads
        );
    }
    let ingest_json = report.to_json();
    match crate::pipeline::record_run(
        &archive,
        &crate::pipeline::RunReceipt {
            command: &command,
            started_at: &started,
            status: "completed",
            team_slug: &team_slug,
            error: None,
            team_slugs: &[],
            mechanical: None,
            ingest: Some(ingest_json),
        },
    ) {
        Ok(run_path) => println!("run metadata: {}", run_path.display()),
        Err(error) => {
            eprintln!("wrkviz: {error}");
            return 2;
        }
    }
    0
}

/// The Python edition's serve help, verbatim.
/// The Python edition's ingest-muse help, verbatim.
pub const INGEST_MUSE_HELP: &str = r#"usage: wrkviz ingest-muse [-h] --output OUTPUT --team TEAM
                          [--snapshot-root DIR] --session-dir SESSION_DIR
                          [--timezone TIMEZONE]
                          [--start-date START_DATE | --start-time START_TIME]
                          [--end-date END_DATE | --end-time END_TIME]
                          [--project LABEL=REPOSITORY_URL]
                          [--source-host HOSTNAME]

snapshot and normalize one Muse session log and the logs of the subagents it
spawned; does not call a model. Example: wrkviz ingest-muse --session-dir
~/.local/share/muse/sessions/2026/10/20/SESSION-ID --team muse-lane --output
./timelines/widget

options:
  -h, --help            show this help message and exit
  --output OUTPUT       version-controllable archive directory
  --team TEAM           stable team slug, for example example-team
  --snapshot-root DIR   where to keep the vendor source snapshots, which are
                        ingest input rather than published output and are the
                        largest thing here (default: <output>.sources, a
                        sibling of the archive; an archive that already keeps
                        them inside itself keeps doing so until `migrate-
                        snapshots` is run)
  --session-dir SESSION_DIR
                        Muse session directory, the one holding session.jsonl
                        (its name is the session id); subagents it spawned are
                        read from its subagent/ directory
  --timezone TIMEZONE   IANA display/calendar timezone (UTC instants remain
                        canonical)
  --start-date START_DATE
                        first local calendar date to include (YYYY-MM-DD,
                        inclusive)
  --start-time START_TIME
                        first instant to include (RFC3339 with offset or Z,
                        inclusive)
  --end-date END_DATE   local calendar boundary to stop at (YYYY-MM-DD,
                        exclusive)
  --end-time END_TIME   instant to stop at (RFC3339 with offset or Z,
                        exclusive)
  --project LABEL=REPOSITORY_URL
                        project/repository identity; repeat for multi-repo
                        work (the first is primary); Codex git metadata is
                        used when omitted
  --source-host HOSTNAME
                        execution hostname captured by the source data; repeat
                        for multi-host work
"#;

const INGEST_MUSE_USAGE: &str = "usage: wrkviz ingest-muse [-h] --output OUTPUT --team TEAM\n                          [--snapshot-root DIR] --session-dir SESSION_DIR\n                          [--timezone TIMEZONE]\n                          [--start-date START_DATE | --start-time START_TIME]\n                          [--end-date END_DATE | --end-time END_TIME]\n                          [--project LABEL=REPOSITORY_URL]\n                          [--source-host HOSTNAME]";

const INGEST_MUSE_OPTIONS: [&str; 11] = [
    "--output",
    "--team",
    "--snapshot-root",
    "--session-dir",
    "--timezone",
    "--start-date",
    "--start-time",
    "--end-date",
    "--end-time",
    "--project",
    "--source-host",
];

fn run_ingest_session_muse(args: &[String]) -> u8 {
    let mut output: Option<String> = None;
    let mut team: Option<String> = None;
    let mut snapshot_root: Option<String> = None;
    let mut session_dir: Option<String> = None;

    let mut timezone = "America/New_York".to_string();
    let mut start_date: Option<String> = None;
    let mut start_time: Option<String> = None;
    let mut end_date: Option<String> = None;
    let mut end_time: Option<String> = None;
    let mut projects: Vec<String> = Vec::new();
    let mut hosts: Vec<String> = Vec::new();
    let mut extras: Vec<String> = Vec::new();
    let mut sep_extras: Vec<String> = Vec::new();
    let mut seen: Vec<&'static str> = Vec::new();
    let mut separated = false;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if separated {
            sep_extras.push(arg.clone());
            index += 1;
            continue;
        }
        if arg == "--" {
            separated = true;
            index += 1;
            continue;
        }
        // argparse resolution: an exact long name wins, else
        // the prefix must name exactly one candidate, with
        // `--help` itself among the candidates.
        let head = arg.split_once('=').map_or(arg.as_str(), |(head, _)| head);
        if arg == "-h" || (arg.len() > 2 && arg.starts_with("-h") && !arg.starts_with("--")) {
            // argparse fires the `-h` action at the head of a
            // single-dash cluster (`-help`) too.
            print!("{INGEST_MUSE_HELP}");
            return 0;
        }
        if head.len() > 2
            && head.starts_with("--")
            && head != "--help"
            && !INGEST_MUSE_OPTIONS.contains(&head)
        {
            let mut candidates: Vec<&str> = vec!["--help"];
            candidates.extend(INGEST_MUSE_OPTIONS.iter().copied());
            let matches: Vec<&str> = candidates
                .into_iter()
                .filter(|name| name.starts_with(head))
                .collect();
            if matches.len() > 1 {
                return argparse_error(
                    INGEST_MUSE_USAGE,
                    "wrkviz ingest-muse",
                    &format!(
                        "ambiguous option: {head} could match {}",
                        matches.join(", ")
                    ),
                );
            }
            if matches == ["--help"] {
                if let Some((_, value)) = arg.split_once('=') {
                    return argparse_error(
                        INGEST_MUSE_USAGE,
                        "wrkviz ingest-muse",
                        &format!("argument -h/--help: ignored explicit argument '{value}'"),
                    );
                }
                print!("{INGEST_MUSE_HELP}");
                return 0;
            }
        }
        if head == "--help" {
            if let Some((_, value)) = arg.split_once('=') {
                return argparse_error(
                    INGEST_MUSE_USAGE,
                    "wrkviz ingest-muse",
                    &format!("argument -h/--help: ignored explicit argument '{value}'"),
                );
            }
            print!("{INGEST_MUSE_HELP}");
            return 0;
        }
        let mut handled = true;
        let canonical: Option<&'static str> = if INGEST_MUSE_OPTIONS.contains(&head) {
            INGEST_MUSE_OPTIONS
                .iter()
                .copied()
                .find(|name| *name == head)
        } else if head.len() > 2 && head.starts_with("--") {
            let matches: Vec<&'static str> = INGEST_MUSE_OPTIONS
                .iter()
                .copied()
                .filter(|name| name.starts_with(head))
                .collect();
            matches.first().copied()
        } else {
            None
        };
        let ordered: Vec<&'static str> = canonical
            .into_iter()
            .chain(
                INGEST_MUSE_OPTIONS
                    .iter()
                    .copied()
                    .filter(|name| Some(*name) != canonical),
            )
            .collect();
        for name in ordered {
            match named_value(args, &mut index, name) {
                Ok(Some(value)) => {
                    seen.push(name);
                    // argparse validates the mutually exclusive
                    // bounds once the second value is in hand.
                    let conflict = match name {
                        "--start-time" if start_date.is_some() => Some("--start-date"),
                        "--start-date" if start_time.is_some() => Some("--start-time"),
                        "--end-time" if end_date.is_some() => Some("--end-date"),
                        "--end-date" if end_time.is_some() => Some("--end-time"),
                        _ => None,
                    };
                    if let Some(other) = conflict {
                        return argparse_error(
                            INGEST_MUSE_USAGE,
                            "wrkviz ingest-muse",
                            &format!("argument {name}: not allowed with argument {other}"),
                        );
                    }
                    match name {
                        "--output" => output = Some(value),
                        "--team" => team = Some(value),
                        "--snapshot-root" => snapshot_root = Some(value),
                        "--session-dir" => session_dir = Some(value),

                        "--timezone" => timezone = value,
                        "--start-date" => start_date = Some(value),
                        "--start-time" => start_time = Some(value),
                        "--end-date" => end_date = Some(value),
                        "--end-time" => end_time = Some(value),
                        "--project" => projects.push(value),
                        "--source-host" => hosts.push(value),
                        _ => {}
                    }
                    break;
                }
                Ok(None) => {}
                Err((_, message)) => {
                    return argparse_error(INGEST_MUSE_USAGE, "wrkviz ingest-muse", &message)
                }
            }
        }
        if !is_option_token(arg, &INGEST_MUSE_OPTIONS) {
            handled = false;
        }
        if !handled {
            extras.push(arg.clone());
        }
        index += 1;
    }
    let mut required: Vec<&str> = Vec::new();
    if output.is_none() {
        required.push("--output");
    }
    if team.is_none() {
        required.push("--team");
    }
    if session_dir.is_none() {
        required.push("--session-dir");
    }
    if !required.is_empty() {
        return argparse_error(
            INGEST_MUSE_USAGE,
            "wrkviz ingest-muse",
            &format!(
                "the following arguments are required: {}",
                required.join(", ")
            ),
        );
    }
    if !extras.is_empty() || !sep_extras.is_empty() {
        let mut reported = extras.clone();
        if !sep_extras.is_empty() {
            reported.push("--".to_string());
            reported.extend(sep_extras.iter().cloned());
        }
        eprintln!(
            "usage: wrkviz [-h] [--version] [--userguide]\n              {{quickstart,ingest-project,ingest,ingest-claude,ingest-claude-history,ingest-muse,ingest-orc,summarize,build,extract-transcripts,export,refresh,github-metadata,refresh-claude,refresh-orc,serve,inspect,gc,migrate-build-state,migrate-snapshots,audit-glossary,audit-losslessness,query,responsiveness,discover-agents}}\n              ...\nwrkviz: error: unrecognized arguments: {}",
            reported.join(" ")
        );
        return 2;
    }
    let output = output.unwrap();
    let team_slug = team.unwrap();
    let session_dir = session_dir.unwrap();
    let archive = cli_path(&output);
    let started = crate::pipeline::utc_now();
    let mut command: Vec<String> = vec!["wrkviz".to_string(), "ingest-muse".to_string()];
    command.extend(args.iter().cloned());
    let outcome: Result<crate::ingest::IngestReport, String> = (|| {
        let timezone_explicit = args
            .iter()
            .any(|item| item == "--timezone" || item.starts_with("--timezone="));
        let (parsed_projects, parsed_hosts) =
            crate::identity::parse_identity_overrides(&projects, &hosts)?;
        let overrides = crate::identity::IdentityOverrides {
            projects: parsed_projects,
            hosts: parsed_hosts,
            display_timezone_source: if timezone_explicit {
                "explicit".to_string()
            } else {
                "default".to_string()
            },
        };
        let date_window = crate::window::parse_date_window(
            start_date.as_deref(),
            end_date.as_deref(),
            &timezone,
            start_time.as_deref(),
            end_time.as_deref(),
        )?;
        let (_, report) = crate::ingest::ingest_muse(
            &archive,
            &cli_path(&session_dir),
            &team_slug,
            &timezone,
            date_window.as_ref(),
            &overrides,
            snapshot_root.as_deref().map(cli_path).as_deref(),
        )?;
        Ok(report)
    })();
    let report = match outcome {
        Ok(report) => report,
        Err(error) => {
            if let Ok(run_path) = crate::pipeline::record_run(
                &archive,
                &crate::pipeline::RunReceipt {
                    command: &command,
                    started_at: &started,
                    status: "failed",
                    team_slug: &team_slug,
                    error: Some(&error),
                    team_slugs: &[],
                    mechanical: None,
                    ingest: None,
                },
            ) {
                eprintln!("run metadata: {}", run_path.display());
            }
            eprintln!("wrkviz: {error}");
            return 2;
        }
    };
    println!(
        "ingest: {} agents, {} messages/events, {} outer tools, {} edges from {} source files ({:.1} MiB); {} archive files changed",
        report.agents,
        report.events,
        report.tool_calls,
        report.edges,
        report.sources,
        report.source_bytes as f64 / (1024.0 * 1024.0),
        report.files_changed
    );
    if report.snapshot_root_established {
        let where_ = crate::json::resolve_path(&archive.join(&report.snapshot_root));
        if report.snapshot_root_layout == "in-archive-legacy" {
            eprintln!(
                "wrkviz: {}: this archive keeps its vendor source snapshots inside itself, at {}; they are ingest input rather than published output, and they are reachable over HTTP from `make serve` until they are moved. `wrkviz migrate-snapshots --output {}` says what moving them would do",
                report.team_slug,
                where_.display(),
                archive.display()
            );
        } else {
            eprintln!(
                "wrkviz: {}: vendor source snapshots are being kept in {}, outside the published archive, because they are ingest input and are usually the largest thing here. The archive records the location; back it up with the archive if you want incremental ingest to keep working",
                report.team_slug,
                where_.display()
            );
        }
    }
    if report.history_unmatched_prompts != 0 || report.history_covered_prompts != 0 {
        let mut parts: Vec<String> = Vec::new();
        if report.history_unmatched_prompts != 0 {
            parts.push(format!(
                "{} prompt(s) typed outside the project pattern",
                report.history_unmatched_prompts
            ));
        }
        if report.history_covered_prompts != 0 {
            parts.push(format!(
                "{} prompt(s) from {} session(s) whose full transcript is archived as its own team",
                report.history_covered_prompts, report.history_covered_sessions
            ));
        }
        eprintln!(
            "wrkviz: {}: left out {}",
            report.team_slug,
            parts.join("; ")
        );
    }
    if report.retired_message_projections != 0 {
        eprintln!(
            "wrkviz: {}: removed {} retired raw/messages/<thread-id>.json file(s) ({:.1} MiB); the tool no longer writes that per-thread projection, which nothing read and which raw/team.json still fully describes",
            report.team_slug,
            report.retired_message_projections,
            report.retired_message_projection_bytes as f64 / (1024.0 * 1024.0)
        );
    }
    if report.newly_promoted_task_notes != 0 {
        eprintln!(
            "wrkviz: {}: promoted {} task note(s) into raw/task-notes.jsonl ({} total); the archive now keeps this text under version control instead of only inside gitignored source_snapshots/",
            report.team_slug, report.newly_promoted_task_notes, report.task_notes
        );
        if report.task_notes_upstream_deleted != 0 {
            eprintln!(
                "wrkviz: {}: {} of those notes no longer exist in the upstream task table; for those this archive is the only copy",
                report.team_slug, report.task_notes_upstream_deleted
            );
        }
    }
    for shard in &report.damaged_payload_shards {
        eprintln!("wrkviz: {}: payload store: {shard}", report.team_slug);
    }
    if !report.pruned_payload_shards.is_empty() {
        eprintln!(
            "wrkviz: {}: {} payload shard(s) recorded by the previous ingest are no longer in the tree; the manifest no longer claims them",
            report.team_slug,
            report.pruned_payload_shards.len()
        );
    }
    if report.newly_stored_tool_payloads != 0 {
        eprintln!(
            "wrkviz: {}: stored {} tool payload(s) ({:.1} MiB) into gitignored teams/{}/payloads/ ({} total); the archive now keeps command arguments and output instead of discarding them and deferring to the vendor logs",
            report.team_slug,
            report.newly_stored_tool_payloads,
            report.newly_stored_tool_payload_bytes as f64 / (1024.0 * 1024.0),
            report.team_slug,
            report.tool_payloads
        );
    }
    let ingest_json = report.to_json();
    match crate::pipeline::record_run(
        &archive,
        &crate::pipeline::RunReceipt {
            command: &command,
            started_at: &started,
            status: "completed",
            team_slug: &team_slug,
            error: None,
            team_slugs: &[],
            mechanical: None,
            ingest: Some(ingest_json),
        },
    ) {
        Ok(run_path) => println!("run metadata: {}", run_path.display()),
        Err(error) => {
            eprintln!("wrkviz: {error}");
            return 2;
        }
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
            // argparse's negative-number matcher takes `-1`
            // and `-.5` as values, never as options.
            let negative_number = value.len() > 1 && value.as_bytes()[1].is_ascii_digit()
                || (value.len() > 2
                    && value.starts_with("-.")
                    && value.as_bytes()[2].is_ascii_digit());
            value.len() > 1 && value.starts_with('-') && !negative_number
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

/// The Python edition's summarize help, verbatim.
pub const SUMMARIZE_HELP: &str = r##"usage: wrkviz summarize [-h] --output OUTPUT --team TEAM
                        [--backend {codex,claude,heuristic}] [--model MODEL]
                        [--reasoning-effort REASONING_EFFORT]
                        [--service-tier SERVICE_TIER]
                        [--summary-workers SUMMARY_WORKERS]
                        [--summary-batch-size SUMMARY_BATCH_SIZE]
                        [--name-batch-size NAME_BATCH_SIZE]
                        [--phase-minutes PHASE_MINUTES]
                        [--context-chars CONTEXT_CHARS]
                        [--transcript-chars TRANSCRIPT_CHARS]
                        [--codex-command CODEX_COMMAND]
                        [--claude-command CLAUDE_COMMAND]
                        [--summary-start-date SUMMARY_START_DATE | --summary-start-time SUMMARY_START_TIME]
                        [--summary-end-date SUMMARY_END_DATE | --summary-end-time SUMMARY_END_TIME]
                        [--summary-timezone SUMMARY_TIMEZONE]
                        [--rollup-kind {hourly,daily,weekly,monthly,quarterly}]
                        [--budget-usd USD] [--price RATES] [--plan]

fill structured summary cache misses

options:
  -h, --help            show this help message and exit
  --output OUTPUT       version-controllable archive directory
  --team TEAM           stable team slug, for example example-team
  --backend {codex,claude,heuristic}
                        summary backend (heuristic is deterministic/offline)
  --model MODEL         backend model name; required for Claude, otherwise
                        defaults to 'gpt-5.5' (or WRKVIZ_MODEL)
  --reasoning-effort REASONING_EFFORT
                        backend reasoning effort, recorded in cache provenance
                        (default: medium). A model/provider failure aborts
                        instead of selecting another model or backend
  --service-tier SERVICE_TIER
                        Codex service tier; Fast uses 'priority', omission
                        means 'default', and the effective value is recorded
                        in cache and run provenance
  --summary-workers SUMMARY_WORKERS
  --summary-batch-size SUMMARY_BATCH_SIZE
  --name-batch-size NAME_BATCH_SIZE
                        agents per hindsight-naming model call
  --phase-minutes PHASE_MINUTES
  --context-chars CONTEXT_CHARS
  --transcript-chars TRANSCRIPT_CHARS
  --codex-command CODEX_COMMAND
                        Codex executable (primarily for testing/wrappers)
  --claude-command CLAUDE_COMMAND
                        Claude executable (primarily for testing/wrappers)
  --summary-start-date SUMMARY_START_DATE
                        first local date to summarize without truncating
                        archived data
  --summary-start-time SUMMARY_START_TIME
                        first RFC3339 instant to summarize, inclusive
  --summary-end-date SUMMARY_END_DATE
                        local date boundary where summarization stops,
                        exclusive
  --summary-end-time SUMMARY_END_TIME
                        RFC3339 instant where summarization stops, exclusive
  --summary-timezone SUMMARY_TIMEZONE
                        IANA timezone for summary date bounds (defaults to the
                        archived team timezone)
  --rollup-kind {hourly,daily,weekly,monthly,quarterly}
                        calendar summary level to generate; repeat as needed
                        (default: daily, weekly, monthly, quarterly)
  --budget-usd USD      spending cap for this run in US dollars; requires
                        --price. Before any model call the whole remaining
                        tree (phase summaries, agent names, overview, every
                        rollup) is estimated, and the run refuses to start,
                        spending nothing, when the estimate is over the cap.
                        If actual spend still reaches the cap, no new model
                        call starts, calls already running finish and are
                        cached, the run exits 3, and the team's
                        summary_data/summarize_state.json marks it partial;
                        rerun to resume (default: no cap)
  --price RATES         model prices in US dollars per million tokens, as inpu
                        t=N,output=N[,cached_input=N][,cache_write_input=N]; a
                        cache rate defaults to the input rate. Used to price
                        the plan and to enforce --budget-usd, e.g. --price inp
                        ut=2,output=10,cached_input=0.2,cache_write_input=2.5
                        (default: none; the plan then shows tokens only)
  --plan                print what the run would generate and its estimated
                        tokens and cost (with --price), then exit without any
                        model call; exits 3 when over --budget-usd
"##;

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

const SUMMARIZE_USAGE: &str = "usage: wrkviz summarize [-h] --output OUTPUT --team TEAM\n                        [--backend {codex,claude,heuristic}] [--model MODEL]\n                        [--reasoning-effort REASONING_EFFORT]\n                        [--service-tier SERVICE_TIER]\n                        [--summary-workers SUMMARY_WORKERS]\n                        [--summary-batch-size SUMMARY_BATCH_SIZE]\n                        [--name-batch-size NAME_BATCH_SIZE]\n                        [--phase-minutes PHASE_MINUTES]\n                        [--context-chars CONTEXT_CHARS]\n                        [--transcript-chars TRANSCRIPT_CHARS]\n                        [--codex-command CODEX_COMMAND]\n                        [--claude-command CLAUDE_COMMAND]\n                        [--summary-start-date SUMMARY_START_DATE | --summary-start-time SUMMARY_START_TIME]\n                        [--summary-end-date SUMMARY_END_DATE | --summary-end-time SUMMARY_END_TIME]\n                        [--summary-timezone SUMMARY_TIMEZONE]\n                        [--rollup-kind {hourly,daily,weekly,monthly,quarterly}]\n                        [--budget-usd USD] [--price RATES] [--plan]";

fn run_summarize(args: &[String]) -> u8 {
    crate::summarize_core::install_interrupt_handler();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print!("{SUMMARIZE_HELP}");
        return 0;
    }
    const VALUE_OPTIONS: &[&str] = &[
        "--output",
        "--team",
        "--backend",
        "--model",
        "--reasoning-effort",
        "--service-tier",
        "--summary-workers",
        "--summary-batch-size",
        "--name-batch-size",
        "--phase-minutes",
        "--context-chars",
        "--transcript-chars",
        "--codex-command",
        "--claude-command",
        "--summary-start-date",
        "--summary-start-time",
        "--summary-end-date",
        "--summary-end-time",
        "--summary-timezone",
        "--rollup-kind",
        "--budget-usd",
        "--price",
    ];
    let mut values: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    let mut plan = false;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        let (head, explicit) = match arg.split_once('=') {
            Some((head, value)) => (head, Some(value.to_string())),
            None => (arg.as_str(), None),
        };
        let mut candidates: Vec<&str> = VALUE_OPTIONS
            .iter()
            .copied()
            .filter(|name| {
                head == *name
                    || (head.len() > 2 && head.starts_with("--") && name.starts_with(head))
            })
            .collect();
        if head == "--plan"
            || (head.len() > 2 && head.starts_with("--") && "--plan".starts_with(head))
        {
            candidates.push("--plan");
        }
        if candidates.len() > 1 && !candidates.contains(&head) {
            return argparse_error(
                SUMMARIZE_USAGE,
                "wrkviz summarize",
                &format!(
                    "ambiguous option: {head} could match {}",
                    candidates.join(", ")
                ),
            );
        }
        let plan_selected = candidates.contains(&"--plan") && !VALUE_OPTIONS.contains(&head);
        if plan_selected {
            if let Some(value) = &explicit {
                return argparse_error(
                    SUMMARIZE_USAGE,
                    "wrkviz summarize",
                    &format!("argument --plan: ignored explicit argument '{value}'"),
                );
            }
            plan = true;
            index += 1;
            continue;
        }
        let matched: Vec<&str> = candidates
            .into_iter()
            .filter(|name| *name != "--plan")
            .collect();
        let Some(name) = matched.first() else {
            return argparse_error(
                SUMMARIZE_USAGE,
                "wrkviz summarize",
                &format!("unrecognized arguments: {arg}"),
            );
        };
        let value = match explicit {
            Some(value) => value,
            None => {
                index += 1;
                match args.get(index) {
                    Some(value) => value.clone(),
                    None => {
                        return argparse_error(
                            SUMMARIZE_USAGE,
                            "wrkviz summarize",
                            &format!("argument {name}: expected one argument"),
                        )
                    }
                }
            }
        };
        values.entry(name.to_string()).or_default().push(value);
        index += 1;
    }
    let get = |name: &str| values.get(name).and_then(|items| items.last()).cloned();
    let missing: Vec<&str> = ["--output", "--team"]
        .into_iter()
        .filter(|name| get(name).is_none())
        .collect();
    if !missing.is_empty() {
        return argparse_error(
            SUMMARIZE_USAGE,
            "wrkviz summarize",
            &format!(
                "the following arguments are required: {}",
                missing.join(", ")
            ),
        );
    }
    if get("--summary-start-date").is_some() && get("--summary-start-time").is_some() {
        return argparse_error(
            SUMMARIZE_USAGE,
            "wrkviz summarize",
            "argument --summary-start-time: not allowed with argument --summary-start-date",
        );
    }
    if get("--summary-end-date").is_some() && get("--summary-end-time").is_some() {
        return argparse_error(
            SUMMARIZE_USAGE,
            "wrkviz summarize",
            "argument --summary-end-time: not allowed with argument --summary-end-date",
        );
    }
    let parse_int = |name: &str, default: i64| -> Result<i64, u8> {
        match get(name) {
            None => Ok(default),
            Some(raw) => raw.parse::<i64>().map_err(|_| {
                argparse_error(
                    SUMMARIZE_USAGE,
                    "wrkviz summarize",
                    &format!(
                        "argument {name}: invalid int value: {}",
                        crate::json::py_repr_str(&raw)
                    ),
                )
            }),
        }
    };
    let summary_workers = match parse_int("--summary-workers", 3) {
        Ok(value) => value,
        Err(code) => return code,
    };
    let summary_batch_size = match parse_int("--summary-batch-size", 6) {
        Ok(value) => value,
        Err(code) => return code,
    };
    let name_batch_size = match parse_int("--name-batch-size", 12) {
        Ok(value) => value,
        Err(code) => return code,
    };
    let phase_minutes = match parse_int("--phase-minutes", 30) {
        Ok(value) => value,
        Err(code) => return code,
    };
    let context_chars = match parse_int("--context-chars", 16000) {
        Ok(value) => value,
        Err(code) => return code,
    };
    let transcript_chars = match parse_int("--transcript-chars", 30000) {
        Ok(value) => value,
        Err(code) => return code,
    };
    let budget_usd: Option<f64> = match get("--budget-usd") {
        None => None,
        Some(raw) => match raw.parse::<f64>() {
            Ok(value) => Some(value),
            Err(_) => {
                return argparse_error(
                    SUMMARIZE_USAGE,
                    "wrkviz summarize",
                    &format!(
                        "argument --budget-usd: invalid float value: {}",
                        crate::json::py_repr_str(&raw)
                    ),
                )
            }
        },
    };
    let backend = get("--backend").unwrap_or_else(|| "codex".to_string());
    if !matches!(backend.as_str(), "codex" | "claude" | "heuristic") {
        return argparse_error(
            SUMMARIZE_USAGE,
            "wrkviz summarize",
            &format!(
                "argument --backend: invalid choice: {} (choose from 'codex', 'claude', 'heuristic')",
                crate::json::py_repr_str(&backend)
            ),
        );
    }
    let mut rollup_kinds: Vec<String> = values.get("--rollup-kind").cloned().unwrap_or_default();
    for kind in &rollup_kinds {
        if !matches!(
            kind.as_str(),
            "hourly" | "daily" | "weekly" | "monthly" | "quarterly"
        ) {
            return argparse_error(
                SUMMARIZE_USAGE,
                "wrkviz summarize",
                &format!(
                    "argument --rollup-kind: invalid choice: {} (choose from 'hourly', 'daily', 'weekly', 'monthly', 'quarterly')",
                    crate::json::py_repr_str(kind)
                ),
            );
        }
    }
    if rollup_kinds.is_empty() {
        rollup_kinds = ["daily", "weekly", "monthly", "quarterly"]
            .iter()
            .map(|item| item.to_string())
            .collect();
    }

    let output = get("--output").unwrap();
    let team_slug = get("--team").unwrap();
    let archive = cli_path(&output);
    let started = crate::pipeline::utc_now();
    let mut command: Vec<String> = vec!["wrkviz".to_string(), "summarize".to_string()];
    command.extend(args.iter().cloned());

    let default_model = std::env::var("WRKVIZ_MODEL").unwrap_or_else(|_| "gpt-5.5".to_string());
    let default_effort =
        std::env::var("WRKVIZ_REASONING_EFFORT").unwrap_or_else(|_| "medium".to_string());
    let reasoning_effort = Some(get("--reasoning-effort").unwrap_or(default_effort));
    let service_tier = get("--service-tier");

    let outcome: Result<Option<crate::summarize_pipeline::SummarizeReport>, (String, bool)> =
        (|| {
            let model = match get("--model") {
                None => {
                    if backend == "claude" {
                        return Err((
                            "--model is required when --backend=claude".to_string(),
                            false,
                        ));
                    }
                    default_model.clone()
                }
                Some(raw) => {
                    let model = crate::summarize_core::py_trim(&raw).to_string();
                    if model.is_empty() {
                        return Err(("--model must not be empty".to_string(), false));
                    }
                    model
                }
            };
            let has_date_bound =
                get("--summary-start-date").is_some() || get("--summary-end-date").is_some();
            let summary_timezone = match get("--summary-timezone") {
                Some(value) => value,
                None => {
                    if has_date_bound {
                        crate::pipeline::load_archived_team(&archive, &team_slug)
                            .map_err(|error| (error, false))?
                            .display_timezone
                    } else {
                        "UTC".to_string()
                    }
                }
            };
            let summary_window = crate::window::parse_date_window(
                get("--summary-start-date").as_deref(),
                get("--summary-end-date").as_deref(),
                &summary_timezone,
                get("--summary-start-time").as_deref(),
                get("--summary-end-time").as_deref(),
            )
            .map_err(|error| (error, false))?;
            if let Some(budget) = budget_usd {
                if budget.is_nan() || budget < 0.0 {
                    return Err((
                        "--budget-usd must be a non-negative number".to_string(),
                        false,
                    ));
                }
            }
            let prices = match get("--price") {
                None => None,
                Some(raw) => Some(
                    crate::summary_budget::Prices::parse(&raw).map_err(|error| (error, false))?,
                ),
            };
            if budget_usd.is_some() && prices.is_none() && backend != "heuristic" {
                return Err((
                    "--budget-usd needs --price to turn token estimates into dollars".to_string(),
                    false,
                ));
            }
            let options = crate::summarize_pipeline::SummarizeOptions {
                max_workers: summary_workers.max(0) as usize,
                batch_size: summary_batch_size.max(0) as usize,
                name_batch_size: name_batch_size.max(0) as usize,
                phase_minutes,
                context_chars,
                transcript_chars,
                codex_command: vec![get("--codex-command").unwrap_or_else(|| "codex".to_string())],
                claude_command: vec![
                    get("--claude-command").unwrap_or_else(|| "claude".to_string())
                ],
                reasoning_effort: reasoning_effort.clone(),
                service_tier: service_tier.clone(),
                summary_window,
                rollup_kinds: rollup_kinds.clone(),
                budget_usd,
                prices,
            };
            if plan {
                let plan = crate::summarize_pipeline::plan_summaries(
                    &archive, &team_slug, &backend, &model, &options,
                )
                .map_err(|error| (error, false))?;
                for line in plan.lines() {
                    println!("{line}");
                }
                if plan.over_budget() {
                    // _PlanOverBudget: exit 3 with no run record.
                    return Err((String::new(), true));
                }
                return Ok(None);
            }
            let report = crate::summarize_pipeline::summarize_archive(
                &archive,
                &team_slug,
                &backend,
                &model,
                &options,
                &|plan| {
                    for line in plan.lines() {
                        println!("{line}");
                    }
                },
            )
            .map_err(|error| {
                let is_budget = error.is_budget();
                (error.message().to_string(), is_budget)
            })?;
            Ok(Some(report))
        })();

    match outcome {
        Ok(None) => {
            // --plan records no run.
            0
        }
        Ok(Some(report)) => {
            print_summaries(&report);
            let summaries_json = report.to_json_obj();
            match crate::pipeline::record_run_summaries(
                &archive,
                &crate::pipeline::RunReceipt {
                    command: &command,
                    started_at: &started,
                    status: "completed",
                    team_slug: &team_slug,
                    error: None,
                    team_slugs: &[],
                    mechanical: None,
                    ingest: None,
                },
                Some(summaries_json),
            ) {
                Ok(run_path) => {
                    println!("run metadata: {}", run_path.display());
                    0
                }
                Err(error) => {
                    eprintln!("wrkviz: {error}");
                    2
                }
            }
        }
        Err((error, is_budget)) => {
            if crate::summarize_core::interrupted() {
                let interruption = "interrupted by user";
                if let Ok(run_path) = crate::pipeline::record_run_summaries(
                    &archive,
                    &crate::pipeline::RunReceipt {
                        command: &command,
                        started_at: &started,
                        status: "interrupted",
                        team_slug: &team_slug,
                        error: Some(interruption),
                        team_slugs: &[],
                        mechanical: None,
                        ingest: None,
                    },
                    None,
                ) {
                    eprintln!("run metadata: {}", run_path.display());
                }
                eprintln!("wrkviz: {interruption}");
                return 130;
            }
            if plan && error.is_empty() && is_budget {
                return 3;
            }
            if let Ok(run_path) = crate::pipeline::record_run_summaries(
                &archive,
                &crate::pipeline::RunReceipt {
                    command: &command,
                    started_at: &started,
                    status: "failed",
                    team_slug: &team_slug,
                    error: Some(&error),
                    team_slugs: &[],
                    mechanical: None,
                    ingest: None,
                },
                None,
            ) {
                eprintln!("run metadata: {}", run_path.display());
            }
            eprintln!("wrkviz: {error}");
            if is_budget {
                3
            } else {
                2
            }
        }
    }
}

fn print_summaries(report: &crate::summarize_pipeline::SummarizeReport) {
    let tier = report.service_tier.as_deref().unwrap_or("unspecified");
    println!(
        "summarize ({} / {} / tier={tier}): {} phases + {} hindsight agent names + {} calendar periods × 2 summary audiences; cache {} hit / {} miss in {} backend batch(es); {} project overview + {} glossary definitions; catalog {} immutable artifacts",
        report.backend,
        report.model,
        report.phases,
        report.agent_names,
        report.rollups,
        report.cache_hits,
        report.cache_misses,
        report.backend_batches,
        report.project_overviews,
        report.glossary_definitions,
        report.catalog_artifacts
    );
    if report.carried_forward_rollups != 0 {
        println!(
            "rollups carried forward: {} (what they summarize is unchanged; only earlier periods they read as context changed)",
            report.carried_forward_rollups
        );
    }
    println!(
        "{}",
        usage_line(
            "tokens newly spent",
            &report.newly_spent_usage,
            report.newly_spent_unknown_receipts
        )
    );
    println!(
        "{}",
        usage_line(
            "tokens behind returned cached artifacts",
            &report.artifact_generation_usage,
            report.artifact_generation_unknown_receipts
        )
    );
    if report.unknown_legacy_artifacts != 0 {
        println!(
            "token accounting: {} legacy artifact(s) have no usage receipt",
            report.unknown_legacy_artifacts
        );
    }
}

fn usage_line(
    label: &str,
    usage: &crate::token_usage::TokenUsage,
    unknown_receipts: i64,
) -> String {
    format!(
        "{label}: input={}, cached_input={}, cache_write_input={}, output={}, reasoning_output={}, total={}; unknown_receipts={unknown_receipts}",
        usage.input_tokens,
        usage.cached_input_tokens,
        usage.cache_write_input_tokens,
        usage.output_tokens,
        usage.reasoning_output_tokens,
        usage.total_tokens()
    )
}
