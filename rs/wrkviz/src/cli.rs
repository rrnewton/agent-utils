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

const QUERY_USAGE: &str = "usage: wrkviz query [-h] --output OUTPUT [--format {json,jsonl,markdown,text}]\n                    {list,show,search,prompts,messages,stats} ...";
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
        other => {
            eprintln!(
                "wrkviz: the Rust edition has not ported '{other}' yet; the Python edition remains the oracle for it"
            );
            2
        }
    }
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
    if let Some(value) = arg.strip_prefix(&format!("{name}=")) {
        return Ok(Some(value.to_string()));
    }
    // argparse also accepts an unambiguous prefix of a long option.
    if arg == name || (arg.len() > 2 && arg.starts_with("--") && name.starts_with(arg.as_str())) {
        *index += 1;
        return match args.get(*index) {
            Some(value) => Ok(Some(value.clone())),
            None => Err((
                name.to_string(),
                format!("argument {name}: expected one argument"),
            )),
        };
    }
    Ok(None)
}

fn is_option_token(arg: &str, names: &[&str]) -> bool {
    names.iter().any(|name| {
        arg == *name
            || arg.starts_with(&format!("{name}="))
            || (arg.len() > 2 && arg.starts_with("--") && name.starts_with(arg))
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
                            &format!(
                                "argument --transcript: ignored explicit argument '{value}'"
                            ),
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
    if reference.starts_with("message:") || reference.starts_with("tool:") {
        eprintln!(
            "wrkviz: the Rust edition has not ported 'query show' for transcript search references yet; the Python edition remains the oracle for it"
        );
        return 2;
    }
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
