//! Command-line interface for `herdr-run`.
//!
//! The surface is a plain multiplexed CLI: `herdr-run [GLOBAL OPTIONS] <subcommand> [OPTIONS]`.
//! Options that identify the invocation and its configuration are global and go before the
//! subcommand; options that only mean something to one subcommand go after it. Neither level
//! documents nor accepts the other's options, so `--help` at each level describes exactly the
//! options that level takes.

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use serde_json::{json, Map, Value};

use crate::allowlist::{admit, render, Admission};
use crate::audit;
use crate::client::{bounded_output, HerdrClient};
use crate::config::{load_config, Config, MAX_TIMEOUT_SECONDS};
use crate::error::{ErrorKind, HerdrRunError, Result, EXIT_BUSY};
use crate::readiness::{assess, infer_prompt_tail};
use crate::runner::{execute, result_json, write_meta, RunResult};
use crate::session::{resolve_target, tab_label_for};
use crate::sweep::sweep;

/// One subcommand and the single line describing it in `herdr-run --help`.
struct Subcommand {
    name: &'static str,
    summary: &'static str,
}

/// Every subcommand, in the order `herdr-run --help` lists them.
///
/// Listed by what a reader is most likely to want first rather than alphabetically: the action,
/// then the two questions asked about it, then setup, then the reports.
const SUBCOMMANDS: &[Subcommand] = &[
    Subcommand {
        name: "run",
        summary: "run one allowlisted command in a pane and return its result",
    },
    Subcommand {
        name: "check",
        summary: "say whether a command would be admitted; touch no pane",
    },
    Subcommand {
        name: "init",
        summary: "write an annotated .herdr-run.yaml in this directory",
    },
    Subcommand {
        name: "config",
        summary: "print the fully resolved configuration as JSON",
    },
    Subcommand {
        name: "target",
        summary: "resolve this agent's pane and print its ids and readiness",
    },
    Subcommand {
        name: "reap",
        summary: "report which command tabs are provably finished with",
    },
    Subcommand {
        name: "net-doctor",
        summary: "smoke-test one scenario: a caller whose own network is blocked",
    },
    Subcommand {
        name: "userguide",
        summary: "print the complete reference",
    },
];

/// Options accepted only before the subcommand.
#[derive(Clone, Debug, Default, PartialEq)]
struct Globals {
    config: Option<PathBuf>,
    agent: Option<String>,
    json: bool,
}

/// Options accepted only after `run`.
#[derive(Clone, Debug, Default, PartialEq)]
struct RunOptions {
    command: String,
    cwd: Option<String>,
    timeout: Option<f64>,
    wait_ready: Option<f64>,
    no_cache: bool,
    dry_run: bool,
}

/// The selected subcommand together with its own options.
#[derive(Clone, Debug, PartialEq)]
enum Selected {
    Run(RunOptions),
    Check(String),
    Init { force: bool },
    Config,
    Target { no_cache: bool },
    Reap,
    NetDoctor,
    Userguide,
}

/// One fully parsed invocation.
#[derive(Clone, Debug, PartialEq)]
struct Invocation {
    globals: Globals,
    selected: Selected,
}

enum ParseResult {
    Invocation(Box<Invocation>),
    Exit(i32),
}

/// Run the CLI over an argument iterator and return the desired process status.
pub fn main<I>(arguments: I) -> i32
where
    I: IntoIterator<Item = OsString>,
{
    let parsed = match parse(arguments) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("usage: {}", usage_line());
            eprintln!("herdr-run: error: {message}");
            return 2;
        }
    };
    let invocation = match parsed {
        ParseResult::Invocation(invocation) => *invocation,
        ParseResult::Exit(code) => return code,
    };
    if invocation.selected == Selected::Userguide {
        // Printed before any configuration is read: the guide is exactly what somebody with a
        // broken configuration file needs, and refusing to show it would be perverse.
        print!("{}", crate::USER_GUIDE);
        return 0;
    }
    match dispatch(invocation) {
        Ok(code) => code,
        Err(error) => {
            let _ = writeln!(io::stderr(), "herdr-run: {error}");
            error.exit_code()
        }
    }
}

fn find_subcommand(name: &str) -> Option<&'static Subcommand> {
    SUBCOMMANDS.iter().find(|entry| entry.name == name)
}

fn subcommand_names() -> String {
    let mut names: Vec<&str> = SUBCOMMANDS.iter().map(|entry| entry.name).collect();
    names.sort_unstable();
    names.join(", ")
}

/// Return the subcommands that own `name`, or an empty slice if no subcommand does.
///
/// Knowing the owner is what turns "unrecognized argument" into a usable message. A caller who
/// writes `herdr-run --cwd /tmp run ...` has the right option and the wrong level, and being told
/// which level it belongs to is the whole difference between a typo and a dead end.
fn local_option_owners(name: &str) -> &'static [&'static str] {
    match name {
        "--cwd" | "--timeout" | "--wait-ready" | "--dry-run" => &["run"],
        "--no-cache" => &["run", "target"],
        "--force" => &["init"],
        _ => &[],
    }
}

fn is_global_option(name: &str) -> bool {
    matches!(
        name,
        "--help" | "-h" | "--version" | "--json" | "--config" | "--agent"
    )
}

fn join_quoted(values: &[&str]) -> String {
    values
        .iter()
        .map(|value| format!("'{value}'"))
        .collect::<Vec<_>>()
        .join(" or ")
}

/// Split `--name=value` into its parts; a bare `--name` yields no inline value.
fn split_option(token: &str) -> (&str, Option<&str>) {
    match token.split_once('=') {
        Some((name, value)) => (name, Some(value)),
        None => (token, None),
    }
}

fn option_value(
    raw: &[String],
    index: &mut usize,
    name: &str,
    inline: Option<&str>,
) -> std::result::Result<String, String> {
    if let Some(value) = inline {
        return Ok(value.to_owned());
    }
    let value = raw
        .get(*index + 1)
        .ok_or_else(|| format!("argument {name}: expected one value"))?;
    if value.starts_with('-') && value != "-" {
        return Err(format!("argument {name}: expected one value"));
    }
    *index += 1;
    Ok(value.clone())
}

fn parse_seconds(option: &str, value: &str) -> std::result::Result<f64, String> {
    let parsed = value
        .parse::<f64>()
        .map_err(|_| format!("argument {option}: invalid numeric value: {value:?}"))?;
    if !parsed.is_finite() || parsed < 0.0 {
        return Err(format!(
            "argument {option}: must be a finite non-negative number"
        ));
    }
    if parsed > MAX_TIMEOUT_SECONDS {
        return Err(format!(
            "argument {option}: must not exceed {MAX_TIMEOUT_SECONDS:.0} seconds"
        ));
    }
    Ok(parsed)
}

fn parse<I>(arguments: I) -> std::result::Result<ParseResult, String>
where
    I: IntoIterator<Item = OsString>,
{
    let raw = arguments
        .into_iter()
        .map(|value| {
            value
                .into_string()
                .map_err(|_| "arguments must be valid UTF-8".to_owned())
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let mut globals = Globals::default();
    let mut index = 0;
    let mut chosen: Option<&'static Subcommand> = None;
    while index < raw.len() {
        let token = raw[index].clone();
        if !token.starts_with('-') {
            match find_subcommand(&token) {
                Some(subcommand) => {
                    chosen = Some(subcommand);
                    index += 1;
                    break;
                }
                None => return Err(unknown_subcommand(&token, &raw[index + 1..])),
            }
        }
        if token == "--" {
            return Err(format!(
                "expected a subcommand before '--'. Subcommands: {}",
                subcommand_names()
            ));
        }
        match token.as_str() {
            "--help" | "-h" => {
                print_help();
                return Ok(ParseResult::Exit(0));
            }
            "--version" => {
                println!("herdr-run {}", env!("CARGO_PKG_VERSION"));
                return Ok(ParseResult::Exit(0));
            }
            "--json" => globals.json = true,
            _ => {
                let (name, inline) = split_option(&token);
                match name {
                    "--config" => {
                        globals.config =
                            Some(PathBuf::from(option_value(&raw, &mut index, name, inline)?));
                    }
                    "--agent" => {
                        globals.agent = Some(option_value(&raw, &mut index, name, inline)?);
                    }
                    "--json" | "--help" | "--version" => {
                        return Err(format!("argument {name}: takes no value"));
                    }
                    other => return Err(misplaced_global(other)),
                }
            }
        }
        index += 1;
    }

    let Some(subcommand) = chosen else {
        // No subcommand at all is not an error: it is somebody asking what this command is.
        print_help();
        return Ok(ParseResult::Exit(0));
    };
    let rest = &raw[index..];
    let selected = match parse_subcommand(subcommand.name, rest)? {
        Some(selected) => selected,
        None => return Ok(ParseResult::Exit(0)),
    };
    Ok(ParseResult::Invocation(Box::new(Invocation {
        globals,
        selected,
    })))
}

/// Parse one subcommand's own arguments; `None` means its `--help` was printed.
fn parse_subcommand(name: &str, rest: &[String]) -> std::result::Result<Option<Selected>, String> {
    match name {
        "run" => parse_run(rest),
        "check" => {
            let Some(positional) = parse_bare(name, rest)? else {
                return Ok(None);
            };
            match positional.len() {
                0 => Err("check: needs a command to check".to_owned()),
                1 => Ok(Some(Selected::Check(positional[0].clone()))),
                _ => Err(one_argument_error("check", &positional)),
            }
        }
        "config" | "reap" | "net-doctor" | "userguide" => {
            let Some(positional) = parse_bare(name, rest)? else {
                return Ok(None);
            };
            if !positional.is_empty() {
                return Err(format!(
                    "{name}: takes no positional arguments; got {}",
                    positional.len()
                ));
            }
            Ok(Some(match name {
                "config" => Selected::Config,
                "reap" => Selected::Reap,
                "net-doctor" => Selected::NetDoctor,
                _ => Selected::Userguide,
            }))
        }
        "init" => {
            let mut force = false;
            let mut index = 0;
            while index < rest.len() {
                let token = &rest[index];
                if token == "--help" || token == "-h" {
                    print_subcommand_help(name);
                    return Ok(None);
                }
                if token == "--force" {
                    force = true;
                    index += 1;
                    continue;
                }
                return Err(local_argument_error(name, token));
            }
            Ok(Some(Selected::Init { force }))
        }
        "target" => {
            let mut no_cache = false;
            let mut index = 0;
            while index < rest.len() {
                let token = &rest[index];
                if token == "--help" || token == "-h" {
                    print_subcommand_help(name);
                    return Ok(None);
                }
                if token == "--no-cache" {
                    no_cache = true;
                    index += 1;
                    continue;
                }
                return Err(local_argument_error(name, token));
            }
            Ok(Some(Selected::Target { no_cache }))
        }
        _ => Err(format!("unknown subcommand '{name}'")),
    }
}

/// Parse a subcommand that takes no options, returning its positional arguments.
fn parse_bare(name: &str, rest: &[String]) -> std::result::Result<Option<Vec<String>>, String> {
    let mut positional = Vec::new();
    let mut options = true;
    for token in rest {
        if options && token == "--" {
            options = false;
            continue;
        }
        if options && (token == "--help" || token == "-h") {
            print_subcommand_help(name);
            return Ok(None);
        }
        if options && token.starts_with('-') {
            return Err(local_argument_error(name, token));
        }
        positional.push(token.clone());
    }
    Ok(Some(positional))
}

fn parse_run(rest: &[String]) -> std::result::Result<Option<Selected>, String> {
    let mut options = RunOptions::default();
    let mut positional = Vec::new();
    let mut parsing = true;
    let mut index = 0;
    while index < rest.len() {
        let token = rest[index].clone();
        if parsing && token == "--" {
            parsing = false;
            index += 1;
            continue;
        }
        if !parsing || !token.starts_with('-') {
            positional.push(token);
            index += 1;
            continue;
        }
        match token.as_str() {
            "--help" | "-h" => {
                print_subcommand_help("run");
                return Ok(None);
            }
            "--no-cache" => options.no_cache = true,
            "--dry-run" => options.dry_run = true,
            _ => {
                let (name, inline) = split_option(&token);
                match name {
                    "--cwd" => {
                        options.cwd = Some(option_value(rest, &mut index, name, inline)?);
                    }
                    "--timeout" => {
                        let value = option_value(rest, &mut index, name, inline)?;
                        options.timeout = Some(parse_seconds(name, &value)?);
                    }
                    "--wait-ready" => {
                        let value = option_value(rest, &mut index, name, inline)?;
                        options.wait_ready = Some(parse_seconds(name, &value)?);
                    }
                    other => return Err(local_argument_error("run", other)),
                }
            }
        }
        index += 1;
    }
    match positional.len() {
        0 => Err("run: needs a command to run".to_owned()),
        1 => {
            options.command = positional[0].clone();
            Ok(Some(Selected::Run(options)))
        }
        _ => Err(one_argument_error("run", &positional)),
    }
}

fn one_argument_error(subcommand: &str, positional: &[String]) -> String {
    let joined = positional.join(" ");
    format!(
        "{subcommand}: pass the command as ONE quoted argument, not as separate words.\n  you wrote:  herdr-run {subcommand} {joined}\n  instead:    herdr-run {subcommand} '{joined}'\nRe-joining loose words would silently change the quoting of your arguments."
    )
}

/// Describe an option that appeared after the subcommand but does not belong to it.
fn local_argument_error(subcommand: &str, token: &str) -> String {
    let (name, _) = split_option(token);
    if is_global_option(name) && name != "--help" && name != "-h" {
        return format!(
            "argument {name}: this is a GLOBAL option; put it before the subcommand:\n  herdr-run {name} ... {subcommand} ..."
        );
    }
    let owners = local_option_owners(name);
    if !owners.is_empty() && !owners.contains(&subcommand) {
        return format!(
            "argument {name}: this is a {} option, not a '{subcommand}' option",
            join_quoted(owners)
        );
    }
    format!("{subcommand}: unrecognized arguments: {token}")
}

/// Describe an option that appeared before the subcommand but belongs to one of them.
fn misplaced_global(name: &str) -> String {
    let owners = local_option_owners(name);
    if owners.is_empty() {
        return format!("unrecognized arguments: {name}");
    }
    format!(
        "argument {name}: this is a {} option; put it AFTER the subcommand:\n  herdr-run {} {name} ...",
        join_quoted(owners),
        owners[0]
    )
}

/// Describe a first positional that is not a subcommand, naming the replacement for the old form.
///
/// `herdr-run <agent> '<command>'` and `herdr-run '<command>'` used to run a command with no
/// subcommand at all. Removing that is the point of this surface, so the removal has to be said
/// out loud, with the exact line to type instead — an "unknown subcommand" alone would leave a
/// caller guessing that the tool had been withdrawn.
fn unknown_subcommand(token: &str, following: &[String]) -> String {
    let mut message = format!("unknown subcommand '{token}'");
    let replacement = if following.len() == 1 && !following[0].starts_with('-') {
        Some(format!("herdr-run --agent {token} run '{}'", following[0]))
    } else if following.is_empty() && token.contains(' ') {
        Some(format!("herdr-run run '{token}'"))
    } else {
        None
    };
    if let Some(replacement) = replacement {
        message.push_str(
            "\nRunning a command without a subcommand is no longer accepted. Use 'run':\n    ",
        );
        message.push_str(&replacement);
    }
    message.push_str(&format!("\nSubcommands: {}", subcommand_names()));
    message
}

fn dispatch(invocation: Invocation) -> Result<i32> {
    let start = std::env::current_dir().map_err(|error| {
        HerdrRunError::config(format!("cannot determine current directory: {error}"))
    })?;
    if let Selected::Init { force } = invocation.selected {
        // Deliberately before load_config: the reason to reach for `init` is often that discovery
        // found nothing, or found something broken, and refusing to write a fresh template because
        // the old one will not parse would be exactly the wrong moment to be strict.
        return command_init(&start, force, invocation.globals.json);
    }
    let config = load_config(invocation.globals.config.as_deref(), &start)?;
    let agent = invocation
        .globals
        .agent
        .clone()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(default_agent);
    let json = invocation.globals.json;
    match invocation.selected {
        Selected::Run(options) => run_command(&config, &agent, json, &options),
        Selected::Check(command) => command_check(&config, &command, json),
        Selected::Init { .. } => unreachable!("init is handled before configuration is loaded"),
        Selected::Config => command_config(&config, &agent),
        Selected::Target { no_cache } => command_target(&config, &agent, !no_cache),
        Selected::Reap => command_reap(&config),
        Selected::NetDoctor => command_net_doctor(&config, &agent),
        Selected::Userguide => Ok(0),
    }
}

fn default_agent() -> String {
    ["HERDR_RUN_AGENT", "DG_AGENT_NAME", "ORC_AGENT_NAME"]
        .iter()
        .find_map(|key| {
            std::env::var(key)
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| "unknown-agent".to_owned())
}

fn command_init(directory: &Path, force: bool, json_output: bool) -> Result<i32> {
    let path = crate::init::write_config_template(directory, force)?;
    if json_output {
        print_json(&json!({"created": true, "path": path.display().to_string()}))?;
    } else {
        println!("wrote {}", path.display());
        println!(
            "Every knob is in that file, set to the value in force today. The allowlist near the\ntop is a HUMAN-ONLY knob: an agent that can widen its own allowlist does not have one."
        );
    }
    Ok(0)
}

fn command_config(config: &Config, agent: &str) -> Result<i32> {
    let document = json!({
        "allow": config.allow,
        "allow_subcommand": config.allow_subcommand,
        "broker": config.broker,
        "cwd": config.cwd,
        "deny_anywhere": config.deny_anywhere,
        "deny_global": config.deny_global,
        "deny_subcommand": config.deny_subcommand,
        "probe_remote": config.probe_remote,
        "project_root": config.project_root,
        "prompt_tail": config.prompt_tail,
        "prefixes": config.prefixes,
        "max_panes": config.max_panes,
        "readiness": config.readiness,
        "retention_days": config.retention_days,
        "ready_timeout_seconds": config.ready_timeout_seconds,
        "shells": config.shells,
        "source": config.source_path.as_deref().unwrap_or("(built-in defaults)"),
        "spool_dir": config.spool_dir,
        "tab_label": tab_label_for(config, agent)?,
        "tab_name": config.tab_name,
        "timeout_seconds": config.timeout_seconds,
        "value_options": config.value_options,
        "workspace": config.workspace,
    });
    print_json(&document)?;
    Ok(0)
}

fn command_check(config: &Config, command: &str, json_output: bool) -> Result<i32> {
    match admit(command, config) {
        Ok(admission) => {
            if json_output {
                print_json(&json!({
                    "program": admission.program,
                    "rendered": admission.rendered(),
                    "subcommand": admission.subcommand,
                    "verdict": "allowed",
                }))?;
            } else {
                println!(
                    "ALLOWED: program={} subcommand={}",
                    admission.program,
                    admission.subcommand.as_deref().unwrap_or("-")
                );
                println!("rendered: {}", admission.rendered());
            }
            Ok(0)
        }
        Err(error) if error.kind() == ErrorKind::Refused => {
            if json_output {
                print_json(&json!({
                    "reason": error.message(),
                    "verdict": "refused",
                }))?;
            } else {
                eprintln!("REFUSED: {error}");
            }
            Ok(error.exit_code())
        }
        Err(error) => Err(error),
    }
}

fn command_target(config: &Config, agent: &str, use_cache: bool) -> Result<i32> {
    let client = HerdrClient::new(&config.broker)?;
    let target = resolve_target(&client, config, agent, use_cache)?;
    let prompt_tail = infer_prompt_tail(config, None);
    let readiness = assess(&client, &target.pane_id, config, prompt_tail.as_deref(), 4)?;
    let document = json!({
        "created": target.created,
        "from_cache": target.from_cache,
        "pane_id": target.pane_id,
        "prompt_tail": prompt_tail,
        "readiness": readiness.describe(),
        "ready": readiness.ready,
        "tab": {"id": target.tab_id, "label": target.tab_label},
        "workspace": {"id": target.workspace_id, "label": target.workspace_label},
    });
    print_json(&document)?;
    Ok(if readiness.ready { 0 } else { EXIT_BUSY })
}

fn run_command(config: &Config, agent: &str, json_output: bool, args: &RunOptions) -> Result<i32> {
    let command = args.command.as_str();
    let log = audit::audit_path(
        Path::new(&config.project_root),
        Path::new(&config.spool_dir),
    );
    let _ = audit::warn_if_spool_is_tracked(
        Path::new(&config.project_root),
        Path::new(&config.spool_dir),
    );
    let admission = match admit(command, config) {
        Ok(admission) => admission,
        Err(error) if error.kind() == ErrorKind::Refused => {
            record_audit(&log, agent, command, "REFUSED", error.message(), Map::new());
            eprintln!("herdr-run: REFUSED: {error}");
            return Ok(error.exit_code());
        }
        Err(error) => return Err(error),
    };
    if args.dry_run {
        record_audit(
            &log,
            agent,
            command,
            "DRY-RUN",
            admission.rendered(),
            Map::new(),
        );
        if json_output {
            print_json(&json!({
                "program": admission.program,
                "rendered": admission.rendered(),
                "verdict": "allowed",
            }))?;
        } else {
            println!("{}", admission.rendered());
        }
        return Ok(0);
    }

    let mut admitted_fields = Map::new();
    admitted_fields.insert("rendered".to_owned(), json!(admission.rendered()));
    record_audit(
        &log,
        agent,
        command,
        "ADMITTED",
        "policy accepted command; target resolution and launch have not yet completed",
        admitted_fields,
    );

    let client = match HerdrClient::new(&config.broker) {
        Ok(client) => client,
        Err(error) => return audited_error(&log, agent, command, error),
    };
    let cwd = match resolved_cwd(config, args.cwd.as_deref()) {
        Ok(cwd) => cwd,
        Err(error) => return audited_error(&log, agent, command, error),
    };
    let ready_timeout = args.wait_ready.unwrap_or(config.ready_timeout_seconds);
    let timeout = args.timeout.unwrap_or(config.timeout_seconds);
    let target = match resolve_target(&client, config, agent, !args.no_cache) {
        Ok(target) => target,
        Err(error) => return audited_error(&log, agent, command, error),
    };
    let result = match execute(
        &client,
        config,
        &target,
        &admission,
        agent,
        &cwd,
        ready_timeout,
        timeout,
    ) {
        Ok(result) => result,
        Err(error) if error.kind() == ErrorKind::Timeout => {
            record_audit(
                &log,
                agent,
                command,
                "RUNTIMEOUT",
                error.message(),
                Map::new(),
            );
            io::stdout()
                .write_all(error.partial_stdout().as_bytes())
                .map_err(|write_error| {
                    HerdrRunError::new(
                        ErrorKind::Other,
                        format!("cannot write partial stdout: {write_error}"),
                    )
                })?;
            io::stderr()
                .write_all(error.partial_stderr().as_bytes())
                .map_err(|write_error| {
                    HerdrRunError::new(
                        ErrorKind::Other,
                        format!("cannot write partial stderr: {write_error}"),
                    )
                })?;
            eprintln!("herdr-run: {error}");
            return Ok(error.exit_code());
        }
        Err(error) => return audited_error(&log, agent, command, error),
    };
    let (meta, meta_error) = write_meta_best_effort(&result, &admission, config, agent);
    let mut fields = Map::new();
    fields.insert("exit_code".to_owned(), json!(result.exit_code));
    fields.insert("meta".to_owned(), json!(meta.as_ref()));
    if let Some(error) = meta_error {
        fields.insert("meta_error".to_owned(), json!(error));
    }
    fields.insert("pane_id".to_owned(), json!(result.target.pane_id));
    fields.insert("rendered".to_owned(), json!(admission.rendered()));
    fields.insert("run_id".to_owned(), json!(result.run_id));
    fields.insert("tab".to_owned(), json!(result.target.tab_label));
    record_audit(
        &log,
        agent,
        command,
        "RAN",
        &format!(
            "exit {} in {:.1}s",
            result.exit_code, result.duration_seconds
        ),
        fields,
    );
    if json_output {
        print_json(&result_json(&result, meta.as_deref()))?;
    } else {
        io::stdout().write_all(&result.stdout).map_err(|error| {
            HerdrRunError::new(ErrorKind::Other, format!("cannot write stdout: {error}"))
        })?;
        io::stderr().write_all(&result.stderr).map_err(|error| {
            HerdrRunError::new(ErrorKind::Other, format!("cannot write stderr: {error}"))
        })?;
    }
    Ok(result.exit_code)
}

fn audited_error(log: &Path, agent: &str, command: &str, error: HerdrRunError) -> Result<i32> {
    record_audit(
        log,
        agent,
        command,
        error_verdict(error.kind()),
        error.message(),
        Map::new(),
    );
    eprintln!("herdr-run: {error}");
    Ok(error.exit_code())
}

fn record_audit(
    log: &Path,
    agent: &str,
    command: &str,
    verdict: &str,
    detail: &str,
    fields: Map<String, Value>,
) {
    if !audit::record(log, agent, command, verdict, detail, fields) {
        eprintln!(
            "herdr-run: WARNING: could not append audit record to {}",
            log.display()
        );
    }
}

fn write_meta_best_effort(
    result: &RunResult,
    admission: &Admission,
    config: &Config,
    agent: &str,
) -> (Option<PathBuf>, Option<String>) {
    match write_meta(result, admission, config, agent) {
        Ok(path) => (Some(path), None),
        Err(error) => {
            eprintln!("herdr-run: WARNING: {}", error.message());
            (None, Some(error.message().to_owned()))
        }
    }
}

/// Report which command tabs are PROVABLY finished with. Closes nothing.
///
/// Report-only is the point, not a limitation. The expensive mistake is closing a tab whose agent
/// is merely thinking, and a reaper that is wrong once in that direction is switched off for good;
/// so the first version of this has to be checkable against a known-good population before anyone
/// lets it act. Every declined pane carries its reason, and every verdict is counted including the
/// zeros, because "reaped 0 because nothing was stale" and "reaped 0 because the detector is inert"
/// are otherwise the same output.
///
/// `candidate_source` states the bound on what could POSSIBLY have been considered. The candidate
/// set is the panes named by surviving run records, and herdr-run prunes a run record
/// `retention_days` after the run finished — so the oldest leaked tabs, which are exactly the ones
/// the pane cap exists to bound, drop out of this report while still counting against `max_panes`.
/// Printing the window is the difference between a report an operator can reason about and a count
/// that quietly means something narrower than it says.
/// The bound on `reap`'s candidate set, printed with every report.
///
/// A constant rather than an inline literal because the wording is part of the command's stable
/// output and is compared verbatim by the differential harness.
const CANDIDATE_SOURCE_NOTE: &str = concat!(
    "candidates are the panes named by surviving run records; run records are pruned ",
    "retention_days after the run finished, so a tab whose agent last ran longer ago than that ",
    "is not considered here and must be closed by hand"
);

fn command_reap(config: &Config) -> Result<i32> {
    let client = HerdrClient::new(&config.broker)?;
    let plan = sweep(&client, config, Path::new("/proc"));
    let entry = |decision: &crate::reap::ReapDecision, with_verdict: bool| {
        let mut object = Map::new();
        object.insert("pane_id".to_owned(), json!(decision.pane_id));
        object.insert("reason".to_owned(), json!(decision.reason));
        object.insert("tab_id".to_owned(), json!(decision.tab_id));
        object.insert("tab_label".to_owned(), json!(decision.tab_label));
        if with_verdict {
            object.insert("verdict".to_owned(), json!(decision.verdict.as_str()));
        }
        Value::Object(object)
    };
    let document = json!({
        "candidate_source": {
            "note": CANDIDATE_SOURCE_NOTE,
            "retention_days": config.retention_days,
            "spool_dir": config.spool_dir,
        },
        "counts": plan.counts(),
        "declined": plan.declined().into_iter().map(|d| entry(d, true)).collect::<Vec<_>>(),
        "reapable": plan.reapable().into_iter().map(|d| entry(d, false)).collect::<Vec<_>>(),
        "workspace": config.workspace,
    });
    print_json(&document)?;
    Ok(0)
}

/// The scope disclaimer `net-doctor` prints before it does anything.
///
/// It goes FIRST, not in the verdict, because a diagnostic that only says what it covered once it
/// has already failed reads as an excuse. `net-doctor` answers exactly one question — does routing
/// a network command through a pane get past a block on the caller's own network — and a reader
/// whose interest in this command is something else should be able to stop at line one.
const NET_DOCTOR_SCOPE: &str = concat!(
    "This is a smoke test for ONE scenario, not a health check of this command as a whole:\n",
    "a caller whose own network access is blocked, running a network command through a pane\n",
    "that is not blocked. It runs the same probe twice — directly, then through the pane —\n",
    "and compares. If you route commands through a pane for some other reason, the verdict\n",
    "below says nothing about that reason.\n",
);

fn command_net_doctor(config: &Config, agent: &str) -> Result<i32> {
    let mut argv = vec![
        "git".to_owned(),
        "ls-remote".to_owned(),
        config.probe_remote.clone(),
        "HEAD".to_owned(),
    ];
    if config.prefixes.iter().any(|prefix| prefix == "with-proxy") {
        argv.insert(0, "with-proxy".to_owned());
    }
    let probe = render(&argv)?;
    println!(
        "herdr-run net-doctor  (agent={agent}, config={})\n",
        config.source_path.as_deref().unwrap_or("defaults")
    );
    println!("{NET_DOCTOR_SCOPE}");
    let log = audit::audit_path(
        Path::new(&config.project_root),
        Path::new(&config.spool_dir),
    );
    let _ = audit::warn_if_spool_is_tracked(
        Path::new(&config.project_root),
        Path::new(&config.spool_dir),
    );
    let admission = match admit(&probe, config) {
        Ok(admission) => admission,
        Err(error) => {
            let fields = net_doctor_error_fields(None);
            record_audit(
                &log,
                agent,
                &probe,
                error_verdict(error.kind()),
                error.message(),
                fields,
            );
            return Err(error);
        }
    };
    let mut admitted_fields = Map::new();
    admitted_fields.insert("net_doctor".to_owned(), json!(true));
    admitted_fields.insert("rendered".to_owned(), json!(admission.rendered()));
    record_audit(
        &log,
        agent,
        &probe,
        "ADMITTED",
        "net-doctor probe accepted; target resolution and launch have not yet completed",
        admitted_fields,
    );
    let mut direct_command = Command::new("/bin/bash");
    direct_command.args(["-lc", &probe]);
    let direct = match bounded_output(&mut direct_command, std::time::Duration::from_secs(120)) {
        Ok(output) => output,
        Err(error) => {
            let error = HerdrRunError::unavailable(format!("cannot run the direct probe: {error}"));
            record_audit(
                &log,
                agent,
                &probe,
                error_verdict(error.kind()),
                error.message(),
                net_doctor_error_fields(None),
            );
            return Err(error);
        }
    };
    let direct_code = direct.status.code().unwrap_or(1);
    let direct_ok = direct_code == 0;
    println!("[direct  ] {probe}");
    println!(
        "           rc={direct_code} {}",
        if direct_ok { "SUCCEEDED" } else { "failed" }
    );
    if !direct_ok {
        let stderr = String::from_utf8_lossy(&direct.stderr);
        println!("           {}", stderr.lines().last().unwrap_or(""));
    }

    // Every failure after the direct half of the diagnosis is a net-doctor result, not a raw
    // wrapper exit. Keep construction, target/cwd resolution, and execution in one result so they
    // all produce the same `[via pane] FAILED` line, final verdict, exit 1, and audit metadata.
    let outside: Result<_> = (|| {
        let client = HerdrClient::new(&config.broker)?;
        let target = resolve_target(&client, config, agent, true)?;
        let cwd = resolved_cwd(config, None)?;
        let result = execute(
            &client,
            config,
            &target,
            &admission,
            agent,
            &cwd,
            config.ready_timeout_seconds.max(30.0),
            config.timeout_seconds.min(180.0),
        )?;
        Ok((target, result))
    })();
    let pane_code = match outside {
        Ok((target, result)) => {
            let (meta, meta_error) = write_meta_best_effort(&result, &admission, config, agent);
            let mut fields = net_doctor_error_fields(Some(direct_code));
            fields.insert("exit_code".to_owned(), json!(result.exit_code));
            fields.insert("run_id".to_owned(), json!(result.run_id));
            fields.insert("pane_id".to_owned(), json!(result.target.pane_id));
            fields.insert("meta".to_owned(), json!(meta.as_ref()));
            if let Some(error) = meta_error {
                fields.insert("meta_error".to_owned(), json!(error));
            }
            record_audit(
                &log,
                agent,
                &probe,
                "RAN",
                &format!("net-doctor pane probe exited {}", result.exit_code),
                fields,
            );
            let head = result
                .stdout_text()
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(80)
                .collect::<String>();
            println!(
                "[via pane] {probe}   (pane {}, tab {})",
                target.pane_id, target.tab_label
            );
            println!(
                "           rc={} {}  {head}",
                result.exit_code,
                if result.exit_code == 0 {
                    "SUCCEEDED"
                } else {
                    "failed"
                }
            );
            result.exit_code
        }
        Err(error) => {
            let fields = net_doctor_error_fields(Some(direct_code));
            record_audit(
                &log,
                agent,
                &probe,
                error_verdict(error.kind()),
                error.message(),
                fields,
            );
            println!("[via pane] FAILED: {error}");
            -1
        }
    };
    println!();
    if pane_code == 0 && !direct_ok {
        println!("VERDICT: this scenario works — the probe is blocked directly and succeeds through the pane.");
        Ok(0)
    } else if pane_code == 0 {
        println!("VERDICT: the pane works, but so does running the probe directly. For THIS scenario the pane is buying you nothing; prefer running the command directly. Other reasons to route a command through a pane are untouched by that.");
        Ok(0)
    } else {
        println!("VERDICT: the pane path is NOT working. Most likely the Herdr server was started from inside a confined process, so its panes inherit the confinement. Stop it ('herdr server stop') and let herdr-run restart it via systemd-run, or start it from an unconfined shell.");
        Ok(1)
    }
}

fn error_verdict(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::Config => "CONFIGERROR",
        ErrorKind::Refused => "REFUSED",
        ErrorKind::Unavailable => "HERDRUNAVAILABLE",
        ErrorKind::Busy => "PANEBUSY",
        ErrorKind::Timeout => "RUNTIMEOUT",
        ErrorKind::Other => "HERDRRUNERROR",
    }
}

fn net_doctor_error_fields(direct_exit_code: Option<i32>) -> Map<String, Value> {
    let mut fields = Map::new();
    fields.insert("net_doctor".to_owned(), json!(true));
    if let Some(code) = direct_exit_code {
        fields.insert("direct_exit_code".to_owned(), json!(code));
    }
    fields
}

fn resolved_cwd(config: &Config, override_cwd: Option<&str>) -> Result<PathBuf> {
    resolved_cwd_with(config, override_cwd, std::env::current_dir)
}

fn resolved_cwd_with<F>(
    config: &Config,
    override_cwd: Option<&str>,
    current_dir: F,
) -> Result<PathBuf>
where
    F: FnOnce() -> io::Result<PathBuf>,
{
    let Some(raw) = override_cwd.or(config.cwd.as_deref()) else {
        return current_dir().map_err(|error| {
            HerdrRunError::config(format!("cannot determine current directory: {error}"))
        });
    };
    let path = Path::new(raw);
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        Path::new(&config.project_root).join(path)
    };
    Ok(lexical_normalize(&joined))
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => output.push(prefix.as_os_str()),
            Component::RootDir => output.push(Path::new("/")),
            Component::CurDir => {}
            Component::ParentDir => {
                output.pop();
            }
            Component::Normal(part) => output.push(part),
        }
    }
    output
}

fn print_json(value: &Value) -> Result<()> {
    let text = serde_json::to_string_pretty(value).map_err(|error| {
        HerdrRunError::new(ErrorKind::Other, format!("cannot encode JSON: {error}"))
    })?;
    println!("{text}");
    Ok(())
}

fn usage_line() -> &'static str {
    "herdr-run [-h] [--version] [--config PATH] [--agent NAME] [--json] <subcommand> [OPTIONS]"
}

fn print_help() {
    println!("usage: {}", usage_line());
    println!(
        "\nRun an allowlisted command in a terminal pane that is not subject to whatever\nconstrains the caller, and get its real stdout, stderr, and exit code back."
    );
    println!("\nsubcommands:");
    for subcommand in SUBCOMMANDS {
        println!("  {:<11} {}", subcommand.name, subcommand.summary);
    }
    println!(
        "\nglobal options (before the subcommand):\n  -h, --help            show this help message and exit\n  --version             show version and exit\n  --config PATH         explicit configuration file\n  --agent NAME          the agent this invocation speaks for; names its tab\n  --json                emit machine-readable output where a subcommand has it"
    );
    println!(
        "\nEach subcommand has its own options: run 'herdr-run <subcommand> --help'.\nOptions are not shared between the two levels — '--cwd' is a 'run' option and\ngoes after 'run'; '--agent' is global and goes before it."
    );
    println!(
        "\nDocumentation:\n  userguide   the complete reference: configuration, exit codes, readiness,\n              retention, the pane cap, and the trust model"
    );
}

fn print_subcommand_help(name: &str) {
    match name {
        "run" => {
            println!("usage: herdr-run [GLOBAL OPTIONS] run [OPTIONS] '<command>'");
            println!(
                "\nRun one allowlisted command in this agent's pane and return its stdout, stderr,\nand exit code. The command is ONE argument: quote it."
            );
            println!("\npositional arguments:\n  <command>             the whole command line, as a single quoted argument");
            println!(
                "\noptions:\n  -h, --help            show this help message and exit\n  --cwd PATH            working directory for the command\n  --timeout SECONDS     how long to wait for the command to finish\n  --wait-ready SECONDS  how long to wait for the pane to go idle\n  --no-cache            ignore the session cache and re-resolve from labels\n  --dry-run             admit and render the command; execute nothing"
            );
            println!("\nExample:\n  herdr-run --agent release-agent run 'git push origin HEAD'");
        }
        "check" => {
            println!("usage: herdr-run [GLOBAL OPTIONS] check '<command>'");
            println!(
                "\nDecide whether a command would be admitted by the policy in effect. Touches no\npane and executes nothing. Exits 0 when allowed and 77 when refused."
            );
            println!("\npositional arguments:\n  <command>             the whole command line, as a single quoted argument");
            println!("\noptions:\n  -h, --help            show this help message and exit");
        }
        "init" => {
            println!("usage: herdr-run [GLOBAL OPTIONS] init [OPTIONS]");
            println!(
                "\nWrite an annotated .herdr-run.yaml into the current directory, the way 'git init'\nwrites into the current directory. Every knob is present and set to the value in\nforce today, so adopting the file changes nothing until you edit it. Refuses to\noverwrite an existing configuration."
            );
            println!(
                "\noptions:\n  -h, --help            show this help message and exit\n  --force               overwrite an existing configuration file"
            );
        }
        "config" => {
            println!("usage: herdr-run [GLOBAL OPTIONS] config");
            println!(
                "\nPrint the fully resolved configuration as JSON: every value in effect, the file\nit came from, and the tab label it renders for this agent."
            );
            println!("\noptions:\n  -h, --help            show this help message and exit");
        }
        "target" => {
            println!("usage: herdr-run [GLOBAL OPTIONS] target [OPTIONS]");
            println!(
                "\nResolve this agent's pane, creating the workspace, tab, and pane if they are\nmissing, and print their ids together with the readiness verdict."
            );
            println!(
                "\noptions:\n  -h, --help            show this help message and exit\n  --no-cache            ignore the session cache and re-resolve from labels"
            );
        }
        "reap" => {
            println!("usage: herdr-run [GLOBAL OPTIONS] reap");
            println!(
                "\nReport which command tabs are provably finished with, and why every other one\nwas declined. Closes nothing."
            );
            println!("\noptions:\n  -h, --help            show this help message and exit");
        }
        "net-doctor" => {
            println!("usage: herdr-run [GLOBAL OPTIONS] net-doctor");
            println!(
                "\nSmoke-test one narrow scenario: a caller whose own network access is blocked,\nreaching the network through a pane that is not blocked. It runs one probe\ndirectly and again through the pane and compares the two. This is not a health\ncheck of the command as a whole."
            );
            println!("\noptions:\n  -h, --help            show this help message and exit");
        }
        "userguide" => {
            println!("usage: herdr-run [GLOBAL OPTIONS] userguide");
            println!(
                "\nPrint the complete reference: configuration, exit codes, readiness, retention,\nthe pane cap, and the trust model."
            );
            println!("\noptions:\n  -h, --help            show this help message and exit");
        }
        other => println!("usage: herdr-run [GLOBAL OPTIONS] {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(arguments: &[&str]) -> std::result::Result<Invocation, String> {
        match parse(arguments.iter().map(OsString::from))? {
            ParseResult::Invocation(invocation) => Ok(*invocation),
            ParseResult::Exit(code) => Err(format!("unexpected early exit {code}")),
        }
    }

    #[test]
    fn global_options_are_accepted_before_the_subcommand() {
        let invocation = parse_args(&[
            "--config",
            "/tmp/c.yaml",
            "--agent",
            "release-agent",
            "--json",
            "run",
            "git status",
        ])
        .expect("parse");
        assert_eq!(
            invocation.globals,
            Globals {
                config: Some(PathBuf::from("/tmp/c.yaml")),
                agent: Some("release-agent".to_owned()),
                json: true,
            }
        );
        assert_eq!(
            invocation.selected,
            Selected::Run(RunOptions {
                command: "git status".to_owned(),
                ..RunOptions::default()
            })
        );
    }

    #[test]
    fn run_options_are_accepted_after_the_subcommand() {
        let invocation = parse_args(&[
            "run",
            "--cwd",
            "/tmp",
            "--timeout=12.5",
            "--wait-ready",
            "3",
            "--no-cache",
            "--dry-run",
            "git status",
        ])
        .expect("parse");
        assert_eq!(
            invocation.selected,
            Selected::Run(RunOptions {
                command: "git status".to_owned(),
                cwd: Some("/tmp".to_owned()),
                timeout: Some(12.5),
                wait_ready: Some(3.0),
                no_cache: true,
                dry_run: true,
            })
        );
    }

    /// The two levels must not leak into each other, in either direction.
    ///
    /// This is the defect the surface exists to fix, so it is pinned as behaviour rather than left
    /// to the help text: an accepted `herdr-run --cwd /tmp run ...` would quietly restore the very
    /// mixing that made the old surface unreadable.
    #[test]
    fn an_option_at_the_wrong_level_is_refused_and_says_which_level_it_belongs_to() {
        for option in [
            "--cwd",
            "--timeout",
            "--wait-ready",
            "--no-cache",
            "--dry-run",
        ] {
            let error = parse_args(&[option, "1", "run", "git status"]).expect_err(option);
            assert!(
                error.contains("put it AFTER the subcommand"),
                "{option}: {error}"
            );
        }
        for option in ["--config", "--agent"] {
            let error = parse_args(&["run", option, "value", "git status"]).expect_err(option);
            assert!(
                error.contains("this is a GLOBAL option"),
                "{option}: {error}"
            );
        }
        let error = parse_args(&["--json", "check", "--dry-run", "git status"])
            .expect_err("check --dry-run");
        assert!(
            error.contains("this is a 'run' option, not a 'check' option"),
            "{error}"
        );
        let error = parse_args(&["target", "--dry-run"]).expect_err("target --dry-run");
        assert!(
            error.contains("this is a 'run' option, not a 'target' option"),
            "{error}"
        );
        // --no-cache belongs to two subcommands, and both must be named.
        let error = parse_args(&["--no-cache", "target"]).expect_err("global --no-cache");
        assert!(error.contains("'run' or 'target'"), "{error}");
    }

    /// The bare form is gone, and its removal names the exact replacement.
    #[test]
    fn the_bare_command_form_is_refused_with_the_run_replacement() {
        let error = parse_args(&["git status"]).expect_err("bare command");
        assert!(error.contains("unknown subcommand 'git status'"), "{error}");
        assert!(error.contains("herdr-run run 'git status'"), "{error}");

        let error = parse_args(&["release-agent", "git status"]).expect_err("bare agent form");
        assert!(
            error.contains("herdr-run --agent release-agent run 'git status'"),
            "{error}"
        );

        let error = parse_args(&["stauts"]).expect_err("typo");
        assert!(error.contains("unknown subcommand 'stauts'"), "{error}");
        assert!(error.contains("Subcommands: "), "{error}");
        // A single word is a mistyped subcommand, not a command line; suggesting `run 'stauts'`
        // would send the reader off to debug the wrong thing.
        assert!(!error.contains("run 'stauts'"), "{error}");
    }

    #[test]
    fn run_needs_exactly_one_quoted_command() {
        let error = parse_args(&["run"]).expect_err("no command");
        assert!(error.contains("needs a command to run"), "{error}");
        let error = parse_args(&["run", "git", "status"]).expect_err("loose words");
        assert!(error.contains("ONE quoted argument"), "{error}");
        assert!(error.contains("herdr-run run 'git status'"), "{error}");
    }

    #[test]
    fn a_double_dash_hands_the_rest_to_the_command() {
        let invocation = parse_args(&["run", "--dry-run", "--", "git --help"]).expect("parse");
        assert_eq!(
            invocation.selected,
            Selected::Run(RunOptions {
                command: "git --help".to_owned(),
                dry_run: true,
                ..RunOptions::default()
            })
        );
        let invocation = parse_args(&["check", "--", "--version"]).expect("parse");
        assert_eq!(invocation.selected, Selected::Check("--version".to_owned()));
    }

    #[test]
    fn invalid_cli_timeouts_are_rejected() {
        for value in ["nan", "inf", "-1", "1e300"] {
            assert!(parse_args(&["run", "--timeout", value, "git status"]).is_err());
        }
    }

    #[test]
    fn option_values_cannot_be_stolen_by_help_or_version() {
        assert!(parse_args(&["--agent", "--help", "run", "x"]).is_err());
        assert!(parse_args(&["run", "--cwd", "--version", "x"]).is_err());
    }

    #[test]
    fn every_subcommand_is_reachable_and_uniquely_named() {
        assert_eq!(
            subcommand_names(),
            "check, config, init, net-doctor, reap, run, target, userguide"
        );
        for subcommand in SUBCOMMANDS {
            assert!(
                find_subcommand(subcommand.name).is_some(),
                "{}",
                subcommand.name
            );
            assert!(!subcommand.summary.is_empty(), "{}", subcommand.name);
        }
    }

    #[test]
    fn default_command_cwd_is_the_callers_current_directory() {
        assert_eq!(
            resolved_cwd(&Config::default(), None).unwrap(),
            std::env::current_dir().unwrap()
        );
    }

    #[test]
    fn vanished_caller_cwd_is_a_typed_config_error() {
        let error = resolved_cwd_with(&Config::default(), None, || {
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                "caller directory was removed",
            ))
        })
        .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Config);
        assert_eq!(error.exit_code(), 78);
        assert!(error
            .message()
            .contains("cannot determine current directory"));
    }

    #[test]
    fn explicit_empty_cwd_means_the_project_root() {
        let config = Config {
            project_root: "/project".to_owned(),
            cwd: Some("configured".to_owned()),
            ..Config::default()
        };
        assert_eq!(
            resolved_cwd(&config, Some("")).unwrap(),
            Path::new("/project")
        );
    }

    #[test]
    fn net_doctor_failures_have_one_verdict_and_metadata_shape() {
        for kind in [
            ErrorKind::Config,
            ErrorKind::Unavailable,
            ErrorKind::Busy,
            ErrorKind::Timeout,
            ErrorKind::Other,
        ] {
            assert!(!error_verdict(kind).is_empty());
            let fields = net_doctor_error_fields(Some(17));
            assert_eq!(fields.get("net_doctor"), Some(&json!(true)));
            assert_eq!(fields.get("direct_exit_code"), Some(&json!(17)));
        }

        let refusal = net_doctor_error_fields(None);
        assert_eq!(refusal.get("net_doctor"), Some(&json!(true)));
        assert!(!refusal.contains_key("direct_exit_code"));
        assert_eq!(error_verdict(ErrorKind::Refused), "REFUSED");
    }

    /// The scope note must lead, and must not be phrased as being about agents alone.
    #[test]
    fn net_doctor_announces_its_narrow_scope_before_probing() {
        assert!(NET_DOCTOR_SCOPE.starts_with("This is a smoke test for ONE scenario"));
        assert!(NET_DOCTOR_SCOPE.contains("not a health check of this command as a whole"));
    }
}
